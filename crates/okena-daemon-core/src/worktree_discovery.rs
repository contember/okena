//! Track worktrees that were created outside Okena.
//!
//! A worktree made from a terminal, by a script or by Claude Code (`claude -w`)
//! used to stay invisible until the user ticked it in the worktree popover.
//! This scan adds it under its parent project instead. Two things keep that
//! from flooding the sidebar, which is what got the old GUI-side auto-add
//! removed: `worktree.auto_track_ignore` (by default Claude Code's
//! short-lived subagent worktrees) and the dismissals a user leaves by
//! untracking a row, which the next scan must not undo.
//!
//! The counterpart that drops rows whose checkout disappeared is
//! `worktree_stale_sweep`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use okena_hooks::{HookMonitor, HookRunner};
use okena_workspace::actions::worktree::{DiscoveredWorktree, linked_checkout_root};
use okena_workspace::persistence::AppSettings;
use okena_workspace::state::{
    ClaimedWorktreeRoot, PhysicalPathIdentity, ProjectData, WindowId, Workspace,
};
use parking_lot::Mutex;
use tokio::sync::watch;

use crate::workspace_cx::DaemonWorkspaceCx;

/// Each pass opens every parent repository with gix and reads its worktree
/// list: no process spawns, cheap enough to notice a new worktree while the
/// agent that made it is still starting up.
const SCAN_INTERVAL: Duration = Duration::from_secs(10);

pub async fn run_worktree_discovery(
    workspace: Arc<Mutex<Workspace>>,
    workspace_tick: watch::Sender<u64>,
    hook_runner: Option<HookRunner>,
    hook_monitor: Option<HookMonitor>,
    settings: Arc<Mutex<AppSettings>>,
) {
    loop {
        tokio::time::sleep(SCAN_INTERVAL).await;
        scan_once(
            &workspace,
            &workspace_tick,
            &hook_runner,
            &hook_monitor,
            &settings,
        )
        .await;
    }
}

/// Run one scan. A worktree it adds gets no terminal: turning discovery on in
/// a repository with dozens of worktrees must not start dozens of shells, and
/// a shell sitting in a checkout gets in the way of the tool that made it
/// removing it again. The row shows Start Terminal until the user opens one.
async fn scan_once(
    workspace: &Arc<Mutex<Workspace>>,
    workspace_tick: &watch::Sender<u64>,
    hook_runner: &Option<HookRunner>,
    hook_monitor: &Option<HookMonitor>,
    settings: &Arc<Mutex<AppSettings>>,
) {
    let config = settings.lock().worktree.clone();
    // Opt-in. Switched off, a pass touches neither the workspace nor the
    // filesystem; dismissals of deleted checkouts wait for it to come back on.
    if !config.auto_track {
        return;
    }
    let input = ScanInput::snapshot(&workspace.lock(), config.auto_track_ignore);

    // Opening repositories and resolving paths is filesystem work; keep it off
    // the reactor and outside the workspace lock.
    let Ok(plan) = tokio::task::spawn_blocking(move || plan_scan(input)).await else {
        log::warn!("worktree discovery task panicked");
        return;
    };

    let mut cx = DaemonWorkspaceCx::new(workspace_tick, hook_runner, hook_monitor);
    let mut ws = workspace.lock();
    // A candidate that keeps failing would otherwise rewrite workspace.json
    // and broadcast a snapshot on every pass.
    if apply_plan(&mut ws, plan) {
        ws.notify_data(&mut cx);
    }
}

/// Apply `plan` under the workspace lock, comparing only what the plan
/// resolved. Returns whether the workspace changed.
///
/// A plan made from a workspace that has changed since (a project added,
/// deleted, moved, reordered or started closing, a worktree untracked) is
/// dropped whole, and the next pass plans against the new state. That keeps
/// every decision the plan made, including the path claims of Okena's own
/// creates and closes, valid at the moment it is applied.
fn apply_plan(workspace: &mut Workspace, plan: ScanPlan) -> bool {
    if plan.discovered.is_empty() && plan.gone_dismissals.is_empty() {
        return false;
    }
    if WorkspaceShape::of(workspace) != plan.shape {
        log::debug!("worktree discovery: the workspace changed during the scan");
        return false;
    }
    let mut changed = workspace.forget_dismissed_worktrees(&plan.gone_dismissals);
    for found in plan.discovered {
        log::info!(
            "worktree discovery: tracking {} ({})",
            found.worktree.worktree_path,
            found.worktree.branch
        );
        let id = workspace.insert_discovered_worktree(
            found.worktree,
            &found.parent_id,
            WindowId::Main,
            false,
        );
        workspace.add_to_worktree_ids(&found.parent_id, &id);
        changed = true;
    }
    changed
}

/// The parts of the workspace a plan is made from.
#[derive(Debug, PartialEq)]
struct WorkspaceShape {
    projects: Vec<ProjectShape>,
    project_order: Vec<String>,
    folders: Vec<(String, Vec<String>)>,
    dismissed: Vec<String>,
}

#[derive(Debug, PartialEq)]
struct ProjectShape {
    id: String,
    path: String,
    /// `Some` for a worktree row: the checkout it recorded, empty on rows
    /// written before that was persisted.
    recorded_checkout: Option<String>,
    remote: bool,
    /// A create or close is in flight, which claims its worktree root.
    busy: bool,
}

impl WorkspaceShape {
    fn of(workspace: &Workspace) -> Self {
        let data = workspace.data();
        Self {
            projects: data
                .projects
                .iter()
                .map(|project| ProjectShape {
                    id: project.id.clone(),
                    path: project.path.clone(),
                    recorded_checkout: project
                        .worktree_info
                        .as_ref()
                        .map(|metadata| metadata.worktree_path.clone()),
                    remote: project.connection_id.is_some(),
                    busy: workspace.is_creating_project(&project.id)
                        || workspace.is_project_closing(&project.id),
                })
                .collect(),
            project_order: data.project_order.clone(),
            folders: data
                .folders
                .iter()
                .map(|folder| (folder.id.clone(), folder.project_ids.clone()))
                .collect(),
            dismissed: data.dismissed_worktree_paths.clone(),
        }
    }

    /// Projects as the sidebar lists them: `project_order`, with each folder
    /// expanded in place. A project missing from both comes last.
    fn in_sidebar_order(&self) -> Vec<&ProjectShape> {
        let mut ordered = Vec::new();
        let mut placed = HashSet::new();
        for entry in &self.project_order {
            let ids = match self
                .folders
                .iter()
                .find(|(folder_id, _)| folder_id == entry)
            {
                Some((_, project_ids)) => project_ids.as_slice(),
                None => std::slice::from_ref(entry),
            };
            for id in ids {
                if let Some(project) = self.projects.iter().find(|project| &project.id == id)
                    && placed.insert(id.as_str())
                {
                    ordered.push(project);
                }
            }
        }
        ordered.extend(
            self.projects
                .iter()
                .filter(|project| !placed.contains(project.id.as_str())),
        );
        ordered
    }
}

/// Everything a scan needs from the workspace, copied out under the lock.
struct ScanInput {
    shape: WorkspaceShape,
    /// Projects whose create or close is in flight, to resolve their claims.
    busy: Vec<ProjectData>,
    ignore: Vec<String>,
}

impl ScanInput {
    fn snapshot(workspace: &Workspace, ignore: Vec<String>) -> Self {
        let shape = WorkspaceShape::of(workspace);
        let busy = workspace
            .data()
            .projects
            .iter()
            .zip(&shape.projects)
            .filter(|(_, project_shape)| project_shape.busy)
            .map(|(project, _)| project.clone())
            .collect();
        Self {
            shape,
            busy,
            ignore,
        }
    }
}

#[derive(Debug, PartialEq)]
struct Discovered {
    parent_id: String,
    worktree: DiscoveredWorktree,
}

#[derive(Debug)]
struct ScanPlan {
    shape: WorkspaceShape,
    discovered: Vec<Discovered>,
    /// Dismissed checkouts that no longer exist.
    gone_dismissals: Vec<String>,
}

fn plan_scan(input: ScanInput) -> ScanPlan {
    let gone_dismissals = input
        .shape
        .dismissed
        .iter()
        .filter(|path| checkout_is_gone(Path::new(path)))
        .cloned()
        .collect();
    let discovered = discover(&input);
    ScanPlan {
        shape: input.shape,
        discovered,
        gone_dismissals,
    }
}

fn discover(input: &ScanInput) -> Vec<Discovered> {
    let shape = &input.shape;
    let parents: Vec<&ProjectShape> = shape
        .in_sidebar_order()
        .into_iter()
        .filter(|project| project.recorded_checkout.is_none() && !project.remote && !project.busy)
        .collect();
    if parents.is_empty() {
        return Vec::new();
    }

    let ignore = build_ignore_set(&input.ignore);
    let claims: Vec<ClaimedWorktreeRoot> = input
        .busy
        .iter()
        .filter_map(ClaimedWorktreeRoot::resolve)
        .collect();
    let known = known_paths(shape);
    let is_known = |identity: &PhysicalPathIdentity| known.contains(identity);

    let mut discovered = Vec::new();
    let mut scanned_roots = HashSet::new();
    for parent in parents {
        let parent_path = Path::new(&parent.path);
        let Some(git_root) = okena_git::repository::get_repo_root(parent_path) else {
            continue;
        };
        // A linked worktree added as a plain project shares its repository
        // with the main checkout. Scanning from it would file the main
        // checkout and its siblings under the wrong parent.
        if git_root.join(".git").is_file() {
            continue;
        }
        let git_root = std::fs::canonicalize(&git_root).unwrap_or(git_root);
        // Two projects in one repository (monorepo packages): the first in
        // sidebar order is the parent.
        if !scanned_roots.insert(git_root.clone()) {
            continue;
        }

        let mut placement = None;
        for (worktree_path, branch) in okena_git::repository::list_linked_git_worktrees(&git_root) {
            let path = Path::new(&worktree_path);
            if !path.is_dir() || is_ignored(&ignore, &git_root, path) {
                continue;
            }
            let worktree_identity = Workspace::physical_path_identity(path);
            if is_known(&worktree_identity) {
                continue;
            }
            let (repo_root, subdir) = placement
                .get_or_insert_with(|| okena_git::resolve_git_root_and_subdir(parent_path));
            let worktree = DiscoveredWorktree::new(&worktree_path, &branch, repo_root, subdir);
            let project_identity =
                Workspace::physical_path_identity(Path::new(&worktree.project_path));
            if is_known(&project_identity) {
                continue;
            }
            if let Err(error) = claims.iter().try_for_each(|claim| {
                claim.check_worktree_target(&worktree_identity)?;
                claim.check_project_path(&project_identity)
            }) {
                log::debug!("worktree discovery: skipping {worktree_path}: {error}");
                continue;
            }
            discovered.push(Discovered {
                parent_id: parent.id.clone(),
                worktree,
            });
        }
    }
    discovered
}

/// Paths a discovered worktree must not land on: every project path, the
/// linked checkout each local project lives in (its recorded one, or the one
/// a plain project's path sits inside), and every dismissal.
fn known_paths(shape: &WorkspaceShape) -> Vec<PhysicalPathIdentity> {
    let project_paths = shape.projects.iter().flat_map(|project| {
        let checkout = (!project.remote)
            .then(|| linked_checkout_root(&project.path, project.recorded_checkout.as_deref()))
            .flatten();
        std::iter::once(project.path.clone()).chain(checkout)
    });
    project_paths
        .chain(shape.dismissed.iter().cloned())
        .map(|path| Workspace::physical_path_identity(Path::new(&path)))
        .collect()
}

/// A dismissed checkout counts as gone only when its directory is missing
/// from a parent that is still there. A checkout on an unmounted volume or a
/// disconnected share has no parent either, and one we may not read answers
/// with another error; dropping those dismissals would bring the worktree
/// back as soon as it is reachable again.
fn checkout_is_gone(path: &Path) -> bool {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            path.parent().is_some_and(Path::is_dir)
        }
        _ => false,
    }
}

/// `*` stays within a path segment so `agent-*` can't swallow a whole subtree.
/// A pattern that doesn't parse is dropped with a warning rather than
/// disabling discovery. An absolute pattern is also added in its canonical
/// spelling, see [`canonical_pattern`].
fn build_ignore_set(patterns: &[String]) -> GlobSet {
    let mut builder = GlobSetBuilder::new();
    let spellings = patterns
        .iter()
        .flat_map(|pattern| std::iter::once(pattern.clone()).chain(canonical_pattern(pattern)));
    for pattern in spellings {
        match GlobBuilder::new(&pattern).literal_separator(true).build() {
            Ok(glob) => {
                builder.add(glob);
            }
            Err(error) => {
                log::warn!("worktree.auto_track_ignore: bad pattern {pattern:?}: {error}")
            }
        }
    }
    builder.build().unwrap_or_else(|error| {
        log::warn!("worktree.auto_track_ignore: {error}");
        GlobSet::empty()
    })
}

/// An absolute pattern with its literal leading directories resolved, or
/// `None` when that changes nothing. Git may record a worktree under its real
/// path while the user writes the pattern through a symlink or an alias
/// (`/tmp` is `/private/tmp` on macOS), and the two must still meet.
fn canonical_pattern(pattern: &str) -> Option<String> {
    let path = Path::new(pattern);
    if !path.is_absolute() {
        return None;
    }
    let mut prefix = PathBuf::new();
    let mut rest = Vec::new();
    for component in path.components() {
        let text = component.as_os_str().to_string_lossy();
        if rest.is_empty() && !text.contains(['*', '?', '[', '{']) {
            prefix.push(component);
        } else {
            rest.push(text.into_owned());
        }
    }
    let resolved = canonical(&prefix);
    if resolved == prefix {
        return None;
    }
    let mut out = resolved.to_string_lossy().into_owned();
    for segment in rest {
        out.push(std::path::MAIN_SEPARATOR);
        out.push_str(&segment);
    }
    Some(out)
}

/// Match a checkout inside the repository by its path relative to the root
/// (`.claude/worktrees/agent-1`), and one outside it by its absolute path.
/// Both the path as Git reports it and its canonical spelling are tried, so a
/// symlink on either side doesn't hide a match.
fn is_ignored(ignore: &GlobSet, git_root: &Path, worktree: &Path) -> bool {
    let roots = [git_root.to_path_buf(), canonical(git_root)];
    [worktree.to_path_buf(), canonical(worktree)]
        .iter()
        .any(
            |path| match roots.iter().find_map(|root| path.strip_prefix(root).ok()) {
                Some(relative) => ignore.is_match(relative),
                None => ignore.is_match(path),
            },
        )
}

/// `path` with symlinks resolved, or unchanged when it can't be resolved.
fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path)
        .map(without_verbatim_prefix)
        .unwrap_or_else(|_| path.to_path_buf())
}

/// `canonicalize` on Windows answers `\\?\C:\...`, which no pattern written
/// as `C:\...` matches (and whose `?` is a glob metacharacter).
fn without_verbatim_prefix(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        let text = path.to_string_lossy();
        if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{rest}"));
        }
        if let Some(rest) = text.strip_prefix(r"\\?\") {
            return PathBuf::from(rest);
        }
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{default_settings, empty_workspace_data, plain_project};
    use okena_state::{FolderData, WorktreeMetadata};
    use std::process::Command;

    const SUBAGENT_IGNORE: &str = ".claude/worktrees/agent-*";

    struct Repo {
        _dir: tempfile::TempDir,
        root: PathBuf,
    }

    fn git(cwd: &Path, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn repo() -> Repo {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap().join("repo");
        std::fs::create_dir_all(&root).unwrap();
        git(&root, &["init", "-q", "-b", "main"]);
        git(
            &root,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "init",
            ],
        );
        Repo { _dir: dir, root }
    }

    impl Repo {
        fn worktree(&self, relative: &str, branch: &str) -> String {
            let path = self.root.join(relative);
            git(
                &self.root,
                &[
                    "worktree",
                    "add",
                    "-q",
                    "-b",
                    branch,
                    path.to_str().unwrap(),
                ],
            );
            path.to_string_lossy().into_owned()
        }

        fn path(&self, relative: &str) -> String {
            self.root.join(relative).to_string_lossy().into_owned()
        }

        /// A workspace holding the main checkout as its only project.
        fn workspace(&self) -> Workspace {
            workspace_with(vec![plain_project("parent", &self.path(""))])
        }
    }

    fn workspace_with(projects: Vec<ProjectData>) -> Workspace {
        let mut data = empty_workspace_data();
        data.project_order = projects.iter().map(|project| project.id.clone()).collect();
        data.projects = projects;
        Workspace::new(data)
    }

    fn worktree_row(id: &str, parent_id: &str, path: &str) -> ProjectData {
        ProjectData {
            worktree_info: Some(WorktreeMetadata {
                parent_project_id: parent_id.to_string(),
                color_override: None,
                main_repo_path: String::new(),
                worktree_path: path.to_string(),
                branch_name: String::new(),
            }),
            ..plain_project(id, path)
        }
    }

    fn plan(workspace: &Workspace) -> ScanPlan {
        plan_scan(ScanInput::snapshot(
            workspace,
            vec![SUBAGENT_IGNORE.to_string()],
        ))
    }

    fn discovered_paths(plan: &ScanPlan) -> Vec<&str> {
        plan.discovered
            .iter()
            .map(|found| found.worktree.worktree_path.as_str())
            .collect()
    }

    fn child_rows(workspace: &Workspace) -> Vec<&ProjectData> {
        workspace
            .data()
            .projects
            .iter()
            .filter(|project| project.worktree_info.is_some())
            .collect()
    }

    #[test]
    fn a_claude_session_worktree_is_found_and_its_subagent_ones_are_not() {
        let repo = repo();
        let session = repo.worktree(".claude/worktrees/fix-login", "worktree-fix-login");
        repo.worktree(".claude/worktrees/agent-a1b2c3", "worktree-agent-a1b2c3");

        let plan = plan(&repo.workspace());

        assert_eq!(discovered_paths(&plan), vec![session.as_str()]);
        assert_eq!(plan.discovered[0].parent_id, "parent");
        assert_eq!(plan.discovered[0].worktree.branch, "worktree-fix-login");
    }

    #[test]
    fn a_tracked_or_dismissed_worktree_is_left_alone() {
        let repo = repo();
        let tracked = repo.worktree("wt/tracked", "tracked");
        let dismissed = repo.worktree("wt/dismissed", "dismissed");
        let new = repo.worktree("wt/new", "new");
        let mut workspace = workspace_with(vec![
            plain_project("parent", &repo.path("")),
            worktree_row("tracked", "parent", &tracked),
        ]);
        workspace.dismiss_worktree(&dismissed);

        let plan = plan(&workspace);

        assert_eq!(discovered_paths(&plan), vec![new.as_str()]);
        assert!(plan.gone_dismissals.is_empty());
    }

    #[test]
    fn a_dismissal_is_forgotten_once_its_checkout_is_gone() {
        let repo = repo();
        std::fs::create_dir_all(repo.root.join("wt")).unwrap();
        let gone = repo.path("wt/removed");
        let mut workspace = repo.workspace();
        workspace.dismiss_worktree(&gone);

        assert_eq!(plan(&workspace).gone_dismissals, vec![gone]);
    }

    #[test]
    fn a_dismissal_on_an_unreachable_volume_is_kept() {
        let repo = repo();
        let mut workspace = repo.workspace();
        // Neither the checkout nor its parent: an unmounted drive or share.
        workspace.dismiss_worktree(&repo.path("unmounted/repo-wt/feature"));

        assert!(plan(&workspace).gone_dismissals.is_empty());
    }

    #[test]
    fn a_monorepo_worktree_goes_under_the_first_project_in_sidebar_order() {
        let repo = repo();
        std::fs::create_dir_all(repo.root.join("packages/app")).unwrap();
        let new = repo.worktree("wt/new", "new");
        // Added first, but listed after the folder that holds the root.
        let package = plain_project("package", &repo.path("packages/app"));
        let root = plain_project("root", &repo.path(""));
        let mut data = empty_workspace_data();
        data.projects = vec![package, root];
        data.folders = vec![FolderData {
            id: "folder".to_string(),
            name: "folder".to_string(),
            project_ids: vec!["root".to_string()],
            folder_color: Default::default(),
        }];
        data.project_order = vec!["folder".to_string(), "package".to_string()];

        let plan = plan(&Workspace::new(data));

        assert_eq!(discovered_paths(&plan), vec![new.as_str()]);
        assert_eq!(plan.discovered[0].parent_id, "root");
        assert_eq!(plan.discovered[0].worktree.project_path, new);
    }

    #[test]
    fn a_linked_worktree_project_does_not_parent_the_main_checkout() {
        let repo = repo();
        let linked = repo.worktree("wt/linked", "linked");

        let plan = plan(&workspace_with(vec![plain_project("linked", &linked)]));

        assert!(plan.discovered.is_empty());
    }

    #[test]
    fn a_plain_project_inside_a_linked_worktree_keeps_it_from_being_added_again() {
        let repo = repo();
        let linked = repo.worktree("wt/linked", "linked");
        let package = Path::new(&linked).join("packages/app");
        std::fs::create_dir_all(&package).unwrap();
        let workspace = workspace_with(vec![
            plain_project("parent", &repo.path("")),
            plain_project("package", &package.to_string_lossy()),
        ]);

        assert!(plan(&workspace).discovered.is_empty());
    }

    #[test]
    fn a_worktree_inside_a_root_okena_is_creating_is_skipped() {
        let repo = repo();
        let nested = repo.worktree("wt/outer/inner", "inner");
        let mut workspace = workspace_with(vec![
            plain_project("parent", &repo.path("")),
            worktree_row("outer", "parent", &repo.path("wt/outer")),
        ]);
        workspace.mark_creating_project("outer");

        let plan = plan(&workspace);

        assert!(
            !discovered_paths(&plan).contains(&nested.as_str()),
            "{:?}",
            plan.discovered
        );
    }

    #[test]
    fn applying_a_plan_adds_a_bookmark_under_the_parent() {
        let repo = repo();
        let new = repo.worktree("wt/new", "new");
        let mut workspace = repo.workspace();

        let plan = plan(&workspace);
        assert!(apply_plan(&mut workspace, plan));

        let rows = child_rows(&workspace);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].path, new);
        assert!(rows[0].layout.is_none(), "no pane waiting for a PTY");
        let parent = workspace.project("parent").unwrap();
        assert_eq!(parent.worktree_ids, vec![rows[0].id.clone()]);
    }

    #[test]
    fn a_parent_deleted_after_planning_gets_no_rows() {
        let repo = repo();
        repo.worktree("wt/new", "new");
        let mut workspace = repo.workspace();

        let plan = plan(&workspace);
        let (tick, _tick_rx) = watch::channel(0);
        workspace.delete_project(
            &mut okena_workspace::focus::FocusManager::new(),
            "parent",
            &Default::default(),
            &mut DaemonWorkspaceCx::new(&tick, &None, &None),
        );

        assert!(!apply_plan(&mut workspace, plan));
        assert!(workspace.data().projects.is_empty());
    }

    #[test]
    fn a_worktree_untracked_after_planning_stays_untracked() {
        let repo = repo();
        let new = repo.worktree("wt/new", "new");
        let mut workspace = repo.workspace();

        let plan = plan(&workspace);
        workspace.dismiss_worktree(&new);

        assert!(!apply_plan(&mut workspace, plan));
        assert!(child_rows(&workspace).is_empty());
    }

    #[test]
    fn a_worktree_okena_starts_creating_after_planning_is_left_to_it() {
        let repo = repo();
        let new = repo.worktree("wt/new", "new");
        let mut data = repo.workspace().data().clone();
        let plan = plan(&Workspace::new(data.clone()));
        data.projects.push(worktree_row("creating", "parent", &new));
        let mut workspace = Workspace::new(data);
        workspace.mark_creating_project("creating");

        assert!(!apply_plan(&mut workspace, plan));
        assert_eq!(child_rows(&workspace).len(), 1);
    }

    struct Scan {
        workspace: Arc<Mutex<Workspace>>,
        settings: Arc<Mutex<AppSettings>>,
        tick: watch::Sender<u64>,
        tick_rx: watch::Receiver<u64>,
    }

    impl Scan {
        fn new(workspace: Workspace, auto_track: bool) -> Self {
            let mut settings = default_settings();
            settings.worktree.auto_track = auto_track;
            let (tick, tick_rx) = watch::channel(0);
            Self {
                workspace: Arc::new(Mutex::new(workspace)),
                settings: Arc::new(Mutex::new(settings)),
                tick,
                tick_rx,
            }
        }

        async fn run(&self) {
            scan_once(&self.workspace, &self.tick, &None, &None, &self.settings).await;
        }

        fn notified(&self) -> bool {
            self.tick_rx.has_changed().unwrap()
        }
    }

    #[tokio::test]
    async fn a_scan_with_discovery_off_changes_nothing() {
        let repo = repo();
        repo.worktree("wt/new", "new");
        let gone = repo.path("wt/removed");
        let mut workspace = repo.workspace();
        workspace.dismiss_worktree(&gone);
        let scan = Scan::new(workspace, false);

        scan.run().await;

        let workspace = scan.workspace.lock();
        assert!(child_rows(&workspace).is_empty());
        assert_eq!(workspace.data().dismissed_worktree_paths, vec![gone]);
        assert!(!scan.notified());
    }

    #[tokio::test]
    async fn a_scan_that_finds_nothing_new_does_not_notify() {
        let repo = repo();
        let tracked = repo.worktree("wt/tracked", "tracked");
        let scan = Scan::new(
            workspace_with(vec![
                plain_project("parent", &repo.path("")),
                worktree_row("tracked", "parent", &tracked),
            ]),
            true,
        );

        scan.run().await;

        assert_eq!(child_rows(&scan.workspace.lock()).len(), 1);
        assert!(!scan.notified());
    }

    #[tokio::test]
    async fn a_scan_that_adds_a_worktree_notifies() {
        let repo = repo();
        repo.worktree("wt/new", "new");
        let scan = Scan::new(repo.workspace(), true);

        scan.run().await;

        assert_eq!(child_rows(&scan.workspace.lock()).len(), 1);
        assert!(scan.notified());
    }

    #[test]
    fn an_ignore_pattern_outside_the_repository_matches_the_absolute_path() {
        let ignore = build_ignore_set(&["/tmp/scratch/**".into()]);
        assert!(is_ignored(
            &ignore,
            Path::new("/repo"),
            Path::new("/tmp/scratch/a/b")
        ));
        assert!(!is_ignored(
            &ignore,
            Path::new("/repo"),
            Path::new("/tmp/other")
        ));
    }

    #[cfg(unix)]
    #[test]
    fn an_absolute_pattern_matches_through_a_symlink_on_either_side() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        std::fs::create_dir_all(real.join("scratch/a")).unwrap();
        let alias = dir.path().join("alias");
        std::os::unix::fs::symlink(&real, &alias).unwrap();
        let repo = Path::new("/repo");

        // Pattern through the alias, worktree reported by its real path.
        let ignore = build_ignore_set(&[format!("{}/scratch/**", alias.display())]);
        assert!(is_ignored(&ignore, repo, &real.join("scratch/a")));

        // Pattern by the real path, worktree reported through the alias.
        let ignore = build_ignore_set(&[format!("{}/scratch/**", real.display())]);
        assert!(is_ignored(&ignore, repo, &alias.join("scratch/a")));
    }

    #[test]
    fn a_star_does_not_cross_a_path_separator() {
        let ignore = build_ignore_set(&[SUBAGENT_IGNORE.into()]);
        assert!(is_ignored(
            &ignore,
            Path::new("/repo"),
            Path::new("/repo/.claude/worktrees/agent-1")
        ));
        assert!(!is_ignored(
            &ignore,
            Path::new("/repo"),
            Path::new("/repo/.claude/worktrees/agent-1/nested")
        ));
    }

    #[test]
    fn a_bad_pattern_does_not_disable_the_others() {
        let ignore = build_ignore_set(&["[".into(), "skip-*".into()]);
        assert!(is_ignored(
            &ignore,
            Path::new("/repo"),
            Path::new("/repo/skip-me")
        ));
    }
}
