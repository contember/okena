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
use okena_app_core::workspace::actions::execute::track_discovered_worktree;
use okena_hooks::{HookMonitor, HookRunner};
use okena_workspace::persistence::{AppSettings, worktree_checkout_path};
use okena_workspace::state::{PhysicalPathIdentity, WindowId, Workspace};
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

    // Opening repositories and statting paths is filesystem work; keep it off
    // the reactor.
    let Ok(plan) = tokio::task::spawn_blocking(move || plan_scan(input)).await else {
        log::warn!("worktree discovery task panicked");
        return;
    };
    if plan.discovered.is_empty() && plan.gone_dismissals.is_empty() {
        return;
    }

    let mut cx = DaemonWorkspaceCx::new(workspace_tick, hook_runner, hook_monitor);
    let mut ws = workspace.lock();
    let mut changed = ws.forget_dismissed_worktrees(&plan.gone_dismissals);
    for found in plan.discovered {
        // The plan was made without the lock. Anything that started in the
        // meantime wins: a delete of the parent, or the user untracking this
        // very worktree. A checkout Okena is busy removing or creating is
        // refused by the path claim checks inside `track_discovered_worktree`.
        if ws.is_project_closing(&found.parent_id)
            || ws.is_worktree_dismissed(Path::new(&found.worktree_path))
        {
            continue;
        }
        match track_discovered_worktree(
            &mut ws,
            WindowId::Main,
            &found.parent_id,
            &found.worktree_path,
            &found.branch,
            false,
        ) {
            Ok(_) => {
                log::info!(
                    "worktree discovery: tracking {} ({})",
                    found.worktree_path,
                    found.branch
                );
                changed = true;
            }
            // Parent gone, tracked by a project the snapshot didn't cover, or
            // a path claim the workspace refuses. Either way not ours to add.
            Err(error) => log::debug!(
                "worktree discovery: skipping {}: {error}",
                found.worktree_path
            ),
        }
    }
    // A candidate that keeps failing would otherwise rewrite workspace.json
    // and broadcast a snapshot on every pass.
    if changed {
        ws.notify_data(&mut cx);
    }
}

/// Everything a scan needs from the workspace, copied out under the lock.
struct ScanInput {
    /// `(project id, project path)` of each project whose repository is scanned.
    parents: Vec<(String, String)>,
    /// Project paths and worktree checkout roots already in the workspace.
    tracked: Vec<PathBuf>,
    dismissed: Vec<String>,
    ignore: Vec<String>,
}

impl ScanInput {
    fn snapshot(workspace: &Workspace, ignore: Vec<String>) -> Self {
        let data = workspace.data();
        let parents = data
            .projects
            .iter()
            .filter(|project| project.worktree_info.is_none())
            .filter(|project| project.connection_id.is_none())
            .filter(|project| !workspace.is_project_closing(&project.id))
            .filter(|project| !workspace.is_creating_project(&project.id))
            .map(|project| (project.id.clone(), project.path.clone()))
            .collect();
        let tracked = data
            .projects
            .iter()
            .flat_map(|project| {
                [
                    PathBuf::from(&project.path),
                    worktree_checkout_path(project).to_path_buf(),
                ]
            })
            .collect();
        Self {
            parents,
            tracked,
            dismissed: data.dismissed_worktree_paths.clone(),
            ignore,
        }
    }
}

#[derive(Debug, PartialEq)]
struct Discovered {
    parent_id: String,
    worktree_path: String,
    branch: String,
}

#[derive(Debug, Default)]
struct ScanPlan {
    discovered: Vec<Discovered>,
    /// Dismissed checkouts that no longer exist.
    gone_dismissals: Vec<String>,
}

fn plan_scan(input: ScanInput) -> ScanPlan {
    let mut plan = ScanPlan {
        gone_dismissals: input
            .dismissed
            .iter()
            .filter(|path| checkout_is_gone(Path::new(path)))
            .cloned()
            .collect(),
        ..ScanPlan::default()
    };
    if input.parents.is_empty() {
        return plan;
    }

    let ignore = build_ignore_set(&input.ignore);
    let tracked: Vec<PhysicalPathIdentity> = input
        .tracked
        .iter()
        .map(|path| Workspace::physical_path_identity(path))
        .collect();
    let dismissed: Vec<PhysicalPathIdentity> = input
        .dismissed
        .iter()
        .map(|path| Workspace::physical_path_identity(Path::new(path)))
        .collect();
    let is_known = |path: &Path| {
        let identity = Workspace::physical_path_identity(path);
        tracked.contains(&identity) || dismissed.contains(&identity)
    };

    let mut scanned_roots = HashSet::new();
    for (parent_id, parent_path) in &input.parents {
        let Some(git_root) = okena_git::repository::get_repo_root(Path::new(parent_path)) else {
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
        // workspace order is the parent, matching what a manual add picks.
        if !scanned_roots.insert(git_root.clone()) {
            continue;
        }

        for (worktree_path, branch) in okena_git::repository::list_linked_git_worktrees(&git_root) {
            let path = Path::new(&worktree_path);
            if !path.is_dir() || is_ignored(&ignore, &git_root, path) || is_known(path) {
                continue;
            }
            plan.discovered.push(Discovered {
                parent_id: parent_id.clone(),
                worktree_path,
                branch,
            });
        }
    }
    plan
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
    use std::process::Command;

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

        fn input(&self) -> ScanInput {
            ScanInput {
                parents: vec![("parent".into(), self.root.to_string_lossy().into_owned())],
                tracked: vec![self.root.clone()],
                dismissed: Vec::new(),
                ignore: vec![".claude/worktrees/agent-*".into()],
            }
        }
    }

    fn discovered_paths(plan: &ScanPlan) -> Vec<&str> {
        plan.discovered
            .iter()
            .map(|found| found.worktree_path.as_str())
            .collect()
    }

    #[test]
    fn a_claude_session_worktree_is_found_and_its_subagent_ones_are_not() {
        let repo = repo();
        let session = repo.worktree(".claude/worktrees/fix-login", "worktree-fix-login");
        repo.worktree(".claude/worktrees/agent-a1b2c3", "worktree-agent-a1b2c3");

        let plan = plan_scan(repo.input());

        assert_eq!(
            plan.discovered,
            vec![Discovered {
                parent_id: "parent".into(),
                worktree_path: session,
                branch: "worktree-fix-login".into(),
            }]
        );
    }

    #[test]
    fn a_tracked_or_dismissed_worktree_is_left_alone() {
        let repo = repo();
        let tracked = repo.worktree("wt/tracked", "tracked");
        let dismissed = repo.worktree("wt/dismissed", "dismissed");
        let new = repo.worktree("wt/new", "new");

        let mut input = repo.input();
        input.tracked.push(PathBuf::from(&tracked));
        input.dismissed.push(dismissed);
        let plan = plan_scan(input);

        assert_eq!(discovered_paths(&plan), vec![new.as_str()]);
        assert!(plan.gone_dismissals.is_empty());
    }

    #[test]
    fn a_dismissal_is_forgotten_once_its_checkout_is_gone() {
        let repo = repo();
        let mut input = repo.input();
        std::fs::create_dir_all(repo.root.join("wt")).unwrap();
        let gone = repo.root.join("wt/removed").to_string_lossy().into_owned();
        input.dismissed.push(gone.clone());

        assert_eq!(plan_scan(input).gone_dismissals, vec![gone]);
    }

    #[test]
    fn a_dismissal_on_an_unreachable_volume_is_kept() {
        let repo = repo();
        let mut input = repo.input();
        // Neither the checkout nor its parent: an unmounted drive or share.
        let unreachable = repo
            .root
            .join("unmounted/repo-wt/feature")
            .to_string_lossy()
            .into_owned();
        input.dismissed.push(unreachable);

        assert!(plan_scan(input).gone_dismissals.is_empty());
    }

    #[test]
    fn a_monorepo_scans_its_repository_once() {
        let repo = repo();
        let new = repo.worktree("wt/new", "new");
        std::fs::create_dir_all(repo.root.join("packages/app")).unwrap();

        let mut input = repo.input();
        input.parents.push((
            "package".into(),
            repo.root
                .join("packages/app")
                .to_string_lossy()
                .into_owned(),
        ));
        let plan = plan_scan(input);

        assert_eq!(discovered_paths(&plan), vec![new.as_str()]);
        assert_eq!(plan.discovered[0].parent_id, "parent");
    }

    #[test]
    fn a_linked_worktree_project_does_not_parent_the_main_checkout() {
        let repo = repo();
        let linked = repo.worktree("wt/linked", "linked");

        let input = ScanInput {
            parents: vec![("linked".into(), linked.clone())],
            tracked: vec![PathBuf::from(&linked)],
            dismissed: Vec::new(),
            ignore: Vec::new(),
        };

        assert!(plan_scan(input).discovered.is_empty());
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
        let ignore = build_ignore_set(&[".claude/worktrees/agent-*".into()]);
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
