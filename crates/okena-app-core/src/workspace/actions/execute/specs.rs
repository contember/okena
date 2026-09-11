//! Engineering-harness OpenSpec actions.
//!
//! okena works with OpenSpec (<https://github.com/Fission-AI/OpenSpec>) the way
//! the `openspec` CLI does — stores registered on this machine, roots found in
//! projects, and folders from settings (<https://openspec.dev/docs/stores>) —
//! but reads and writes the files itself through `okena-openspec`. Browsing
//! works on a machine that has never installed the CLI, and an agent drafting a
//! change is still free to use it.
//!
//! Reads are scoped to roots discovery found and path-checked against them:
//! these actions are reachable by any client and by agents through okena's MCP
//! server, so they must not become a way to read arbitrary files.

use super::ActionResult;
use crate::workspace::persistence::AppSettings;
use crate::workspace::state::{ProjectData, WindowId, Workspace};
use okena_core::specs::{SpecRoot, SpecRootKind, SpecStores, change_slug};
use okena_openspec::discover::{self, ProjectSource, Sources};
use okena_openspec::files::{self, DEFAULT_SCHEMA};
use okena_openspec::{OpenSpecDirs, registry, setup, tree};
use okena_terminal::TerminalsRegistry;
use okena_terminal::backend::TerminalBackend;
use okena_workspace::context::WorkspaceCx;
use std::path::{Path, PathBuf};

/// OpenSpec's machine directories, with overrides from settings.
fn dirs(settings: &AppSettings) -> OpenSpecDirs {
    let specs = &settings.harness.specs;
    OpenSpecDirs::detect(specs.data_dir.as_deref(), specs.config_dir.as_deref())
}

fn sources(projects: &[ProjectData], settings: &AppSettings) -> Sources {
    let specs = &settings.harness.specs;
    let projects = if specs.projects {
        projects
            .iter()
            // A worktree is a second checkout of a repo already listed, and a
            // session is rooted at a spec root or above several repos — neither
            // is a root of its own.
            .filter(|p| p.worktree_info.is_none() && !p.is_spec_session() && !p.is_agent_session())
            .map(|p| ProjectSource {
                name: p.name.clone(),
                path: p.path.clone(),
            })
            .collect()
    } else {
        Vec::new()
    };
    Sources {
        registry: specs.registry,
        projects,
        folders: settings.harness.spec_folders(),
    }
}

fn snapshot(projects: &[ProjectData], settings: &AppSettings) -> SpecStores {
    discover::discover(&dirs(settings), &sources(projects, settings))
}

fn to_result(value: serde_json::Result<serde_json::Value>, what: &str) -> ActionResult {
    match value {
        Ok(v) => ActionResult::Ok(Some(v)),
        Err(e) => ActionResult::Err(format!("could not serialize {what}: {e}")),
    }
}

/// Every root okena can see.
pub(super) fn stores(ws: &Workspace, settings: &AppSettings) -> ActionResult {
    to_result(
        serde_json::to_value(snapshot(&ws.data.projects, settings)),
        "spec stores",
    )
}

/// A root the client named, checked against what discovery found — never a
/// path taken on trust.
fn resolve_root(
    projects: &[ProjectData],
    settings: &AppSettings,
    key: Option<&str>,
) -> Result<SpecRoot, String> {
    let stores = snapshot(projects, settings);
    match key.map(str::trim).filter(|k| !k.is_empty()) {
        Some(key) => stores.root(key).cloned().ok_or_else(|| {
            format!(
                "unknown spec root `{key}` — it is no longer discovered; refresh the Specs view"
            )
        }),
        None => stores.default_root().cloned().ok_or_else(|| {
            "no OpenSpec roots found — register a store or add a folder in Settings → Specs"
                .to_string()
        }),
    }
}

pub(super) fn tree(ws: &Workspace, settings: &AppSettings, root: Option<String>) -> ActionResult {
    tree_for(&ws.data.projects, settings, root)
}

fn tree_for(
    projects: &[ProjectData],
    settings: &AppSettings,
    root: Option<String>,
) -> ActionResult {
    let root = match resolve_root(projects, settings, root.as_deref()) {
        Ok(r) => r,
        Err(e) => return ActionResult::Err(e),
    };
    let mut t = tree::read_tree(Path::new(&root.path));
    t.root_key = root.key.clone();
    // Only a registered store can be selected with `--store`; a folder that
    // merely holds store metadata cannot.
    if root.kind == SpecRootKind::Store {
        t.store_id = root.store_id.clone();
    }
    to_result(serde_json::to_value(&t), "spec tree")
}

/// Largest document this action will return, in bytes.
///
/// Specs are prose; anything past this is not a spec, and streaming a huge file
/// through a JSON action response would stall the client for no benefit.
const MAX_DOC_BYTES: u64 = 2 * 1024 * 1024;

pub(super) fn read(
    ws: &Workspace,
    settings: &AppSettings,
    root: Option<String>,
    path: String,
) -> ActionResult {
    read_for(&ws.data.projects, settings, root, path)
}

fn read_for(
    projects: &[ProjectData],
    settings: &AppSettings,
    root: Option<String>,
    path: String,
) -> ActionResult {
    let root = match resolve_root(projects, settings, root.as_deref()) {
        Ok(r) => r,
        Err(e) => return ActionResult::Err(e),
    };
    let real = match tree::resolve_document(Path::new(&root.path), &path) {
        Ok(p) => p,
        Err(e) => return ActionResult::Err(e),
    };
    if let Ok(m) = real.metadata()
        && m.len() > MAX_DOC_BYTES
    {
        return ActionResult::Err(format!(
            "document is too large to display ({} KB)",
            m.len() / 1024
        ));
    }
    match std::fs::read_to_string(&real) {
        Ok(content) => ActionResult::Ok(Some(serde_json::json!({
            "root": root.key,
            "path": path,
            "content": content,
        }))),
        Err(e) => ActionResult::Err(format!("could not read {path}: {e}")),
    }
}

// ─── Store management ────────────────────────────────────────────────────────

pub(super) fn register_store(
    settings: &AppSettings,
    path: String,
    id: Option<String>,
) -> ActionResult {
    match registry::register(&dirs(settings), &path, id.as_deref()) {
        Ok(r) => ActionResult::Ok(Some(serde_json::json!({
            "id": r.id,
            "root": r.root.to_string_lossy(),
            "metadata_created": r.metadata_created,
            "already_registered": r.already_registered,
        }))),
        Err(e) => ActionResult::Err(e.to_string()),
    }
}

pub(super) fn unregister_store(settings: &AppSettings, id: String) -> ActionResult {
    match registry::unregister(&dirs(settings), &id) {
        Ok(left) => ActionResult::Ok(Some(serde_json::json!({
            "id": id,
            "left_on_disk": left.to_string_lossy(),
        }))),
        Err(e) => ActionResult::Err(e.to_string()),
    }
}

pub(super) fn setup_store(
    settings: &AppSettings,
    id: String,
    path: String,
    remote: Option<String>,
    init_git: bool,
) -> ActionResult {
    let request = setup::SetupRequest {
        id,
        path,
        remote,
        init_git,
    };
    match setup::setup_store(&dirs(settings), &request) {
        Ok(out) => ActionResult::Ok(Some(serde_json::json!({
            "id": out.id,
            "root": out.root.to_string_lossy(),
            "created": out.created,
            "git_initialized": out.git_initialized,
            "committed": out.committed,
            "already_registered": out.already_registered,
        }))),
        Err(e) => ActionResult::Err(e.to_string()),
    }
}

pub(super) fn set_default_store(settings: &AppSettings, id: Option<String>) -> ActionResult {
    let id = id.map(|i| i.trim().to_string()).filter(|i| !i.is_empty());
    match setup::set_default_store(&dirs(settings), id.as_deref()) {
        Ok(()) => ActionResult::Ok(Some(serde_json::json!({ "default_store": id }))),
        Err(e) => ActionResult::Err(e.to_string()),
    }
}

// ─── Drafting a change ───────────────────────────────────────────────────────

/// The proposal stub okena writes when scaffolding a change.
///
/// Deliberately thin. Its job is to record the idea verbatim so the change is
/// browsable and nothing is lost if the agent is closed straight away — the
/// thinking is the agent's, and pre-filling headings it may not want would
/// fight OpenSpec's "fluid not rigid" stance.
fn proposal_stub(idea: &str) -> String {
    format!("# {idea}\n\n## Why\n\n{idea}\n\n## What Changes\n\n_Drafting._\n")
}

/// Today's local date as `YYYY-MM-DD` — what `openspec new change` records
/// (`formatLocalDate`). Falls back to UTC where the local offset cannot be
/// determined safely.
fn today() -> String {
    let now = time::OffsetDateTime::now_local().unwrap_or_else(|_| time::OffsetDateTime::now_utc());
    date_string(now.date())
}

fn date_string(date: time::Date) -> String {
    format!(
        "{:04}-{:02}-{:02}",
        date.year(),
        u8::from(date.month()),
        date.day()
    )
}

/// Brief an agent to fill in a scaffolded change.
///
/// States the conventions inline rather than assuming the agent knows
/// OpenSpec: most models have not read it, and a wrong guess produces a
/// plausible-looking tree in the wrong shape.
fn brief(idea: &str, change: &str, change_dir: &str, root: &SpecRoot) -> String {
    let mut out = format!(
        "Draft an OpenSpec change for this idea: {idea}\n\n\
         The change directory already exists at `{change_dir}` with its \
         `.openspec.yaml` and a stub `proposal.md` holding the idea. Work only \
         inside that directory.\n\n\
         Follow OpenSpec conventions (https://github.com/Fission-AI/OpenSpec):\n\
         - `proposal.md` — why this change, and what changes.\n\
         - `design.md` — the technical approach, when the change needs one.\n\
         - `tasks.md` — an implementation checklist.\n\
         - `specs/<capability>/spec.md` — delta specs for the requirements this \
         change adds, modifies or removes.\n\n\
         Read the existing `openspec/specs/` before proposing. Prefer plain \
         Markdown and keep it short. Ask me about anything ambiguous rather \
         than inventing requirements."
    );
    match (root.kind, root.store_id.as_deref()) {
        (SpecRootKind::Store, Some(id)) => out.push_str(&format!(
            "\n\nThis is the OpenSpec store `{id}`. If the `openspec` CLI is \
             installed you may use it; pass `--store {id}` so every command \
             targets this store, e.g. `openspec status --change {change} --store {id}`. \
             Do not install the CLI if it is not."
        )),
        _ => out.push_str(
            "\n\nIf the `openspec` CLI is installed you may use it; do not install \
             it if it is not.",
        ),
    }
    let references: Vec<_> = root
        .references
        .iter()
        .filter_map(|r| r.root.as_ref().map(|path| (r.id.as_str(), path.as_str())))
        .collect();
    if !references.is_empty() {
        out.push_str(
            "\n\nReferenced stores — read-only upstream context. Fetch what you \
             need and cite what you use:",
        );
        for (id, path) in references {
            out.push_str(&format!(
                "\n- `{id}` at `{path}` (e.g. `openspec show <spec-id> --type spec --store {id}`)"
            ));
        }
    }
    out
}

/// How to hand `command` an opening prompt.
///
/// Agents disagree here, so this is a small per-agent table like the MCP-flag
/// one. Note the difference in kind: `claude` takes a positional prompt and
/// stays interactive, while `copilot`'s only prompt flag is non-interactive and
/// exits when it is done — a drafted spec either way, but only one of them
/// leaves you in a conversation.
pub(super) fn prompt_args(command: &str, prompt: &str) -> Vec<String> {
    let program = Path::new(command)
        .file_stem()
        .map(|s| s.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    match program.as_str() {
        "copilot" => vec!["--prompt".into(), prompt.into()],
        // `claude` and, by convention, anything else: a positional prompt.
        _ => vec![prompt.into()],
    }
}

/// Shell for an agent session opened with `prompt` — a spec draft here, and a
/// knowledge draft in `knowledge.rs`.
pub(super) fn spec_agent_shell(
    settings: &AppSettings,
    override_command: Option<&str>,
    prompt: &str,
) -> Option<okena_terminal::shell_config::ShellType> {
    // An explicit empty string means "scaffold only, no agent", even when a
    // default agent is configured — same contract as starting work on a task.
    let command = match override_command {
        Some(c) => c,
        None => settings.harness.agent_command.as_deref().unwrap_or(""),
    }
    .trim()
    .to_string();
    if command.is_empty() {
        return None;
    }
    let mut args = prompt_args(&command, prompt);
    args.extend(super::agent_mcp::injection_args(&command, settings));
    Some(okena_terminal::shell_config::ShellType::Custom {
        path: command,
        args,
    })
}

/// Create `openspec/changes/<slug>/` the way `openspec new change` does, plus
/// the proposal stub. Returns the change directory.
fn scaffold_change(root: &SpecRoot, slug: &str, idea: &str) -> Result<PathBuf, String> {
    let change_dir = Path::new(&root.path)
        .join("openspec")
        .join("changes")
        .join(slug);
    // Refuse rather than merge into an existing change: the user asked to start
    // something new, and writing a fresh stub over a change already being
    // worked on would destroy it.
    if change_dir.exists() {
        return Err(format!(
            "a change named `{slug}` already exists — open it, or reword the idea"
        ));
    }
    std::fs::create_dir_all(&change_dir)
        .map_err(|e| format!("could not create the change directory: {e}"))?;
    let schema = root.schema.as_deref().unwrap_or(DEFAULT_SCHEMA);
    std::fs::write(
        change_dir.join(files::CHANGE_METADATA_FILE),
        files::change_metadata_yaml(schema, &today()),
    )
    .map_err(|e| format!("could not write {}: {e}", files::CHANGE_METADATA_FILE))?;
    std::fs::write(change_dir.join("proposal.md"), proposal_stub(idea))
        .map_err(|e| format!("could not write proposal.md: {e}"))?;
    Ok(change_dir)
}

/// Scaffold a change directory and open an agent session to fill it in.
#[allow(clippy::too_many_arguments)]
pub(super) fn draft_change(
    ws: &mut Workspace,
    window_id: WindowId,
    idea: String,
    name: Option<String>,
    agent_command: Option<String>,
    root: Option<String>,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
    settings: &AppSettings,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    let idea = idea.trim().to_string();
    if idea.is_empty() {
        return ActionResult::Err("describe the change in a sentence first".into());
    }
    let root = match resolve_root(&ws.data.projects, settings, root.as_deref()) {
        Ok(r) => r,
        Err(e) => return ActionResult::Err(e),
    };
    if !root.healthy {
        let why = root
            .status
            .first()
            .map(|d| d.message.clone())
            .unwrap_or_else(|| "it is not a usable OpenSpec root".into());
        return ActionResult::Err(format!("can't draft in `{}`: {why}", root.name));
    }
    // An explicit name wins; otherwise derive one from the prompt. Slugged
    // either way, since a name typed by hand is no more filesystem-safe than a
    // sentence.
    let slug = match name.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
        Some(n) => change_slug(n),
        None => change_slug(&idea),
    };
    if slug.is_empty() {
        return ActionResult::Err(
            "that name has no letters or numbers to name a change after".into(),
        );
    }
    let change_dir = match scaffold_change(&root, &slug, &idea) {
        Ok(d) => d,
        Err(e) => return ActionResult::Err(e),
    };
    let root_path = PathBuf::from(&root.path);
    let change_rel = tree::rel(&root_path, &change_dir);

    // ── Agent session ────────────────────────────────────────────────────────
    //
    // Rooted at the OpenSpec root, not the change directory: OpenSpec asks an
    // author to read the existing `openspec/specs/` before proposing, and an
    // agent confined to the new directory cannot.
    let mut session: Option<serde_json::Value> = None;
    let name = format!("{slug} (spec)");
    match ws.add_project(
        name.clone(),
        root.path.clone(),
        true,
        &settings.hooks,
        window_id,
        cx,
    ) {
        Ok(project_id) => {
            // Mark it before spawning, so the session is recognizable as a
            // spec session from the first snapshot the client sees.
            if let Some(p) = ws.data.projects.iter_mut().find(|p| p.id == project_id) {
                p.spec_change = Some(slug.clone());
            }
            // Set before spawning: the terminal reads the project's default
            // shell as it starts.
            if let Some(shell) = spec_agent_shell(
                settings,
                agent_command.as_deref(),
                &brief(&idea, &slug, &change_rel, &root),
            ) && let Some(p) = ws.data.projects.iter_mut().find(|p| p.id == project_id)
            {
                p.default_shell = Some(shell);
            }
            if let ActionResult::Err(e) = super::spawn_uninitialized_terminals(
                ws,
                &project_id,
                backend,
                terminals,
                settings,
                None,
                cx,
            ) {
                log::warn!("[specs] spec session terminal failed to spawn: {e}");
            }
            session = Some(serde_json::json!({
                "project_id": project_id,
                "name": name,
            }));
        }
        // The change directory is real and browsable even without a session, so
        // report the failure rather than failing the whole call.
        Err(e) => log::warn!("[specs] could not create spec session project: {e}"),
    }

    ws.notify_data(cx);

    ActionResult::Ok(Some(serde_json::json!({
        "root": root.key,
        "change": slug,
        "path": change_rel,
        "session": session,
    })))
}

#[cfg(test)]
mod tests {
    use super::{
        ActionResult, brief, date_string, prompt_args, proposal_stub, read_for, resolve_root,
        scaffold_change, tree_for,
    };
    use crate::workspace::persistence::AppSettings;
    use okena_core::specs::{SpecRootKind, SpecTree};
    use std::path::{Path, PathBuf};

    fn tmpdir(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "okena-specs-{tag}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(path: &Path, content: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    /// Settings whose OpenSpec directories live in `sandbox`, so no test ever
    /// reads — or writes — the developer's real store registry.
    fn sandboxed(sandbox: &Path) -> AppSettings {
        let mut settings = AppSettings::default();
        settings.harness.specs.data_dir = Some(sandbox.join("data").to_string_lossy().into());
        settings.harness.specs.config_dir = Some(sandbox.join("config").to_string_lossy().into());
        settings
    }

    fn populated_root(root: &Path) {
        let os = root.join("openspec");
        write(&os.join("config.yaml"), "schema: spec-driven\n");
        write(&os.join("specs/auth/spec.md"), "# Auth");
        write(&os.join("changes/add-login/proposal.md"), "# Why");
        write(&os.join("changes/add-login/specs/login/spec.md"), "# Login");
        write(
            &os.join("changes/archive/2026-01-01-old-thing/proposal.md"),
            "# Old",
        );
    }

    fn tree_of(settings: &AppSettings, root: Option<String>) -> SpecTree {
        let ActionResult::Ok(Some(v)) = tree_for(&[], settings, root) else {
            panic!("expected a spec tree");
        };
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn the_legacy_spec_repo_setting_still_opens_as_a_folder_root() {
        // Existing installs set `spec_repo`; they must keep working unchanged.
        let sandbox = tmpdir("legacy");
        let repo = sandbox.join("specs");
        populated_root(&repo);
        let mut settings = sandboxed(&sandbox);
        settings.harness.spec_repo = Some(repo.to_string_lossy().into_owned());

        let t = tree_of(&settings, None);
        assert!(t.initialized);
        assert!(t.root_key.starts_with("path:"));
        assert!(t.store_id.is_none());
        assert_eq!(t.specs[0].name, "auth");
        assert_eq!(t.changes[0].name, "add-login");
        assert_eq!(t.changes[0].specs[0].name, "login");
        assert_eq!(t.archived[0].name, "2026-01-01-old-thing");
        std::fs::remove_dir_all(&sandbox).ok();
    }

    #[test]
    fn the_default_store_opens_by_default_and_carries_its_id() {
        let sandbox = tmpdir("store");
        let settings = sandboxed(&sandbox);
        let dirs = super::dirs(&settings);
        let store = sandbox.join("stores/team-plans");
        populated_root(&store);
        write(
            &store.join(".openspec-store/store.yaml"),
            "version: 1\nid: team-plans\n",
        );
        okena_openspec::registry::register(&dirs, &store.to_string_lossy(), None).unwrap();
        okena_openspec::setup::set_default_store(&dirs, Some("team-plans")).unwrap();

        let t = tree_of(&settings, None);
        assert_eq!(t.root_key, "store:team-plans");
        assert_eq!(t.store_id.as_deref(), Some("team-plans"));
        std::fs::remove_dir_all(&sandbox).ok();
    }

    #[test]
    fn nothing_configured_says_where_to_configure_it() {
        let sandbox = tmpdir("empty");
        let ActionResult::Err(e) = tree_for(&[], &sandboxed(&sandbox), None) else {
            panic!("expected an error");
        };
        assert!(e.contains("Settings → Specs"), "unhelpful: {e}");
        std::fs::remove_dir_all(&sandbox).ok();
    }

    #[test]
    fn a_root_key_the_daemon_did_not_discover_is_refused() {
        // The key names a root; it must not be a way to name any directory.
        let sandbox = tmpdir("unknown-key");
        let secret = sandbox.join("elsewhere/openspec/secret.md");
        write(&secret, "SECRET");
        let key = format!("path:{}", sandbox.join("elsewhere").to_string_lossy());
        let settings = sandboxed(&sandbox);
        assert!(resolve_root(&[], &settings, Some(&key)).is_err());
        assert!(matches!(
            read_for(&[], &settings, Some(key), "openspec/secret.md".into()),
            ActionResult::Err(_)
        ));
        std::fs::remove_dir_all(&sandbox).ok();
    }

    #[test]
    fn reading_outside_the_root_is_refused_through_the_action() {
        let sandbox = tmpdir("escape");
        let repo = sandbox.join("specs");
        populated_root(&repo);
        write(&sandbox.join("outside.md"), "SECRET");
        let mut settings = sandboxed(&sandbox);
        settings.harness.specs.folders = vec![repo.to_string_lossy().into_owned()];

        let ActionResult::Ok(Some(v)) =
            read_for(&[], &settings, None, "openspec/specs/auth/spec.md".into())
        else {
            panic!("expected content");
        };
        assert_eq!(v["content"], "# Auth");
        assert!(matches!(
            read_for(&[], &settings, None, "openspec/../../outside.md".into()),
            ActionResult::Err(_)
        ));
        std::fs::remove_dir_all(&sandbox).ok();
    }

    #[test]
    fn project_discovery_can_be_turned_off() {
        let sandbox = tmpdir("projects");
        let repo = sandbox.join("app");
        populated_root(&repo);
        let project: crate::workspace::state::ProjectData =
            serde_json::from_value(serde_json::json!({
                "id": "p1", "name": "app", "path": repo.to_string_lossy(),
            }))
            .unwrap();
        let mut settings = sandboxed(&sandbox);
        let found = resolve_root(std::slice::from_ref(&project), &settings, None).unwrap();
        assert_eq!(found.kind, SpecRootKind::Project);

        settings.harness.specs.projects = false;
        assert!(resolve_root(std::slice::from_ref(&project), &settings, None).is_err());
        std::fs::remove_dir_all(&sandbox).ok();
    }

    #[test]
    fn scaffolding_writes_what_openspec_new_change_writes_and_refuses_to_overwrite() {
        let sandbox = tmpdir("scaffold");
        let repo = sandbox.join("specs");
        populated_root(&repo);
        let mut settings = sandboxed(&sandbox);
        settings.harness.specs.folders = vec![repo.to_string_lossy().into_owned()];
        let root = resolve_root(&[], &settings, None).unwrap();

        let dir = scaffold_change(&root, "add-sso", "Add SSO").unwrap();
        let meta = std::fs::read_to_string(dir.join(".openspec.yaml")).unwrap();
        assert!(meta.starts_with("schema: spec-driven\ncreated: "), "{meta}");
        assert!(dir.join("proposal.md").is_file());
        assert!(scaffold_change(&root, "add-sso", "again").is_err());
        std::fs::remove_dir_all(&sandbox).ok();
    }

    #[test]
    fn dates_are_zero_padded_like_openspec_writes_them() {
        let date = time::Date::from_calendar_date(2026, time::Month::September, 1).unwrap();
        assert_eq!(date_string(date), "2026-09-01");
    }

    #[test]
    fn copilot_and_claude_take_prompts_differently() {
        assert_eq!(prompt_args("claude", "hi"), ["hi"]);
        assert_eq!(
            prompt_args("/usr/local/bin/copilot", "hi"),
            ["--prompt", "hi"]
        );
        // An unknown agent gets the positional form rather than nothing, so the
        // prompt is never silently dropped.
        assert_eq!(prompt_args("aider", "hi"), ["hi"]);
    }

    #[test]
    fn the_stub_keeps_the_idea_verbatim() {
        let s = proposal_stub("Add login with Google");
        assert!(s.contains("Add login with Google"));
    }

    fn root_of(kind: SpecRootKind, store_id: Option<&str>) -> okena_core::specs::SpecRoot {
        okena_core::specs::SpecRoot {
            key: "k".into(),
            kind,
            name: "n".into(),
            path: "/r".into(),
            store_id: store_id.map(str::to_string),
            remote: None,
            schema: None,
            healthy: true,
            is_default: false,
            references: vec![okena_core::specs::SpecReference {
                id: "design-system".into(),
                remote: None,
                root: Some("/stores/design-system".into()),
                status: Vec::new(),
            }],
            used_by: Vec::new(),
            status: Vec::new(),
        }
    }

    #[test]
    fn the_brief_names_the_directory_and_the_artifacts() {
        let b = brief(
            "Add login",
            "add-login",
            "openspec/changes/add-login",
            &root_of(SpecRootKind::Folder, None),
        );
        assert!(b.contains("openspec/changes/add-login"));
        for f in okena_core::specs::CHANGE_ARTIFACTS {
            assert!(b.contains(f), "brief never mentions {f}");
        }
        // A folder cannot be selected with --store; only its references can.
        assert!(!b.contains("--change add-login --store"), "{b}");
    }

    #[test]
    fn a_store_brief_targets_the_store_and_lists_its_references() {
        // Without `--store`, an agent's CLI calls resolve against its working
        // directory — not necessarily this store.
        let b = brief(
            "Add login",
            "add-login",
            "openspec/changes/add-login",
            &root_of(SpecRootKind::Store, Some("team-plans")),
        );
        assert!(b.contains("--change add-login --store team-plans"));
        assert!(b.contains("`design-system` at `/stores/design-system`"));
    }
}
