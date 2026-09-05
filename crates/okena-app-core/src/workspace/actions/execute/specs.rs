//! Engineering-harness OpenSpec actions.
//!
//! okena reads the OpenSpec layout (<https://github.com/Fission-AI/OpenSpec>)
//! straight off disk instead of shelling out to the `openspec` CLI. The
//! convention is plain Markdown in a git repo, so browsing works on a machine
//! that has never installed the CLI, while an agent drafting a change is free
//! to use the CLI itself.
//!
//! Everything here is scoped to `settings.harness.spec_repo`. Reads are
//! path-checked against that root: this action is reachable by any client and
//! by agents through okena's MCP server, so it must not become a way to read
//! arbitrary files.

use super::ActionResult;
use crate::workspace::persistence::AppSettings;
use crate::workspace::state::{WindowId, Workspace};
use okena_core::specs::{CHANGE_ARTIFACTS, SpecChange, SpecDoc, SpecTree, change_slug};
use okena_terminal::TerminalsRegistry;
use okena_terminal::backend::TerminalBackend;
use okena_workspace::context::WorkspaceCx;
use std::path::{Path, PathBuf};

/// The configured spec repository, or an explanation of why there isn't one.
fn spec_root(settings: &AppSettings) -> Result<PathBuf, String> {
    let configured = settings
        .harness
        .spec_repo
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            "no spec repository configured — set it in Settings → Harness".to_string()
        })?;
    let path = PathBuf::from(shellexpand_home(configured));
    if !path.is_dir() {
        return Err(format!("spec repository not found: {}", path.display()));
    }
    Ok(path)
}

/// Expand a leading `~`.
///
/// Settings are hand-edited as often as they are set through the UI, and
/// `~/p/specs` is the form a person naturally types.
fn shellexpand_home(p: &str) -> String {
    if let Some(rest) = p.strip_prefix("~/")
        && let Some(home) = dirs::home_dir()
    {
        return home.join(rest).to_string_lossy().into_owned();
    }
    p.to_string()
}

/// Path relative to `root`, in the forward-slash form the wire types use.
fn rel(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// Markdown documents directly inside `dir`, sorted by name.
///
/// Non-Markdown files are skipped: OpenSpec is a Markdown convention, and
/// listing stray files would make the tree noisier without making it truer.
fn markdown_docs(root: &Path, dir: &Path) -> Vec<SpecDoc> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut docs: Vec<SpecDoc> = entries
        .flatten()
        .filter(|e| e.path().is_file())
        .filter(|e| {
            e.path()
                .extension()
                .is_some_and(|x| x.eq_ignore_ascii_case("md"))
        })
        .map(|e| SpecDoc {
            path: rel(root, &e.path()),
            name: e.file_name().to_string_lossy().into_owned(),
        })
        .collect();
    docs.sort_by(|a, b| a.name.cmp(&b.name));
    docs
}

/// Read one change directory.
fn read_change(root: &Path, dir: &Path, archived: bool) -> SpecChange {
    let name = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    // Listed in convention order (why → how → work) rather than alphabetically,
    // because that is the order a reader wants them in. Missing files are
    // simply absent: OpenSpec is explicitly "fluid not rigid", so a change with
    // only a proposal is normal and must not look broken.
    let artifacts = CHANGE_ARTIFACTS
        .iter()
        .map(|f| dir.join(f))
        .filter(|p| p.is_file())
        .map(|p| SpecDoc {
            path: rel(root, &p),
            name: p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
        })
        .collect();
    SpecChange {
        name,
        path: rel(root, dir),
        artifacts,
        specs: markdown_docs(root, &dir.join("specs")),
        archived,
    }
}

/// Change directories inside `dir`, newest first.
///
/// Newest-first because the change you want is nearly always the one you just
/// made. `archive` is skipped — it is read separately as its own list.
fn read_changes(root: &Path, dir: &Path, archived: bool) -> Vec<SpecChange> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut dirs: Vec<(std::time::SystemTime, PathBuf)> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .filter(|p| p.file_name().is_some_and(|n| n != "archive"))
        .map(|p| {
            let modified = p
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(std::time::UNIX_EPOCH);
            (modified, p)
        })
        .collect();
    dirs.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    dirs.into_iter()
        .map(|(_, p)| read_change(root, &p, archived))
        .collect()
}

/// Build the OpenSpec tree for the configured repository.
pub(super) fn tree(settings: &AppSettings) -> ActionResult {
    let root = match spec_root(settings) {
        Ok(r) => r,
        Err(e) => return ActionResult::Err(e),
    };
    let openspec = root.join("openspec");
    // An uninitialized repo is a normal state, not an error: it is what every
    // spec repo looks like before the first change. The UI offers to start one.
    let mut out = SpecTree {
        root: root.to_string_lossy().into_owned(),
        initialized: openspec.is_dir(),
        ..Default::default()
    };
    if out.initialized {
        out.specs = markdown_docs(&root, &openspec.join("specs"));
        let changes = openspec.join("changes");
        out.changes = read_changes(&root, &changes, false);
        out.archived = read_changes(&root, &changes.join("archive"), true);
    }
    match serde_json::to_value(&out) {
        Ok(v) => ActionResult::Ok(Some(v)),
        Err(e) => ActionResult::Err(format!("could not serialize spec tree: {e}")),
    }
}

/// Largest document this action will return, in bytes.
///
/// Specs are prose; anything past this is not a spec, and streaming a huge file
/// through a JSON action response would stall the client for no benefit.
const MAX_DOC_BYTES: u64 = 2 * 1024 * 1024;

/// Resolve a client-supplied relative path inside `root`.
///
/// Canonicalizes both sides so `..` and symlinks cannot escape: the check has
/// to be on the resolved path, since `openspec/../../.ssh/id_rsa` is a perfectly
/// ordinary-looking string.
fn resolve_in_root(root: &Path, path: &str) -> Result<PathBuf, String> {
    let candidate = root.join(path);
    let real = candidate
        .canonicalize()
        .map_err(|_| format!("no such document: {path}"))?;
    let real_root = root
        .canonicalize()
        .map_err(|e| format!("spec repository is unreadable: {e}"))?;
    if !real.starts_with(&real_root) {
        return Err("path is outside the spec repository".into());
    }
    if !real.is_file() {
        return Err(format!("not a file: {path}"));
    }
    Ok(real)
}

/// Read one document from the spec repository.
pub(super) fn read(settings: &AppSettings, path: String) -> ActionResult {
    let root = match spec_root(settings) {
        Ok(r) => r,
        Err(e) => return ActionResult::Err(e),
    };
    let real = match resolve_in_root(&root, &path) {
        Ok(p) => p,
        Err(e) => return ActionResult::Err(e),
    };
    match real.metadata() {
        Ok(m) if m.len() > MAX_DOC_BYTES => {
            return ActionResult::Err(format!(
                "document is too large to display ({} KB)",
                m.len() / 1024
            ));
        }
        _ => {}
    }
    match std::fs::read_to_string(&real) {
        Ok(content) => ActionResult::Ok(Some(serde_json::json!({
            "path": path,
            "content": content,
        }))),
        Err(e) => ActionResult::Err(format!("could not read {path}: {e}")),
    }
}

/// The proposal stub okena writes when scaffolding a change.
///
/// Deliberately thin. Its job is to record the idea verbatim so the change is
/// browsable and nothing is lost if the agent is closed straight away — the
/// thinking is the agent's, and pre-filling headings it may not want would
/// fight OpenSpec's "fluid not rigid" stance.
fn proposal_stub(idea: &str) -> String {
    format!("# {idea}\n\n## Why\n\n{idea}\n\n## What Changes\n\n_Drafting._\n")
}

/// Brief an agent to fill in a scaffolded change.
///
/// States the conventions inline rather than assuming the agent knows
/// OpenSpec: most models have not read it, and a wrong guess produces a
/// plausible-looking tree in the wrong shape.
fn brief(idea: &str, change_dir: &str) -> String {
    format!(
        "Draft an OpenSpec change for this idea: {idea}\n\n\
         The change directory already exists at `{change_dir}` with a stub \
         `proposal.md` holding the idea. Work only inside that directory.\n\n\
         Follow OpenSpec conventions (https://github.com/Fission-AI/OpenSpec):\n\
         - `proposal.md` — why this change, and what changes.\n\
         - `design.md` — the technical approach, when the change needs one.\n\
         - `tasks.md` — an implementation checklist.\n\
         - `specs/` — the requirements this change introduces or amends.\n\n\
         Prefer plain Markdown and keep it short. Ask me about anything \
         ambiguous rather than inventing requirements. If the `openspec` CLI \
         is installed you may use it; do not install it if it is not."
    )
}

/// How to hand `command` an opening prompt.
///
/// Agents disagree here, so this is a small per-agent table like the MCP-flag
/// one. Note the difference in kind: `claude` takes a positional prompt and
/// stays interactive, while `copilot`'s only prompt flag is non-interactive and
/// exits when it is done — a drafted spec either way, but only one of them
/// leaves you in a conversation.
fn prompt_args(command: &str, prompt: &str) -> Vec<String> {
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

/// Shell for a spec-drafting agent session.
fn spec_agent_shell(
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

/// Scaffold a change directory and open an agent session to fill it in.
#[allow(clippy::too_many_arguments)]
pub(super) fn draft_change(
    ws: &mut Workspace,
    window_id: WindowId,
    idea: String,
    name: Option<String>,
    agent_command: Option<String>,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
    settings: &AppSettings,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    let idea = idea.trim().to_string();
    if idea.is_empty() {
        return ActionResult::Err("describe the change in a sentence first".into());
    }
    let root = match spec_root(settings) {
        Ok(r) => r,
        Err(e) => return ActionResult::Err(e),
    };
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

    let change_dir = root.join("openspec").join("changes").join(&slug);
    // Refuse rather than merge into an existing change: the user asked to start
    // something new, and writing a fresh stub over a change already being
    // worked on would destroy it.
    if change_dir.exists() {
        return ActionResult::Err(format!(
            "a change named `{slug}` already exists — open it, or reword the idea"
        ));
    }
    if let Err(e) = std::fs::create_dir_all(change_dir.join("specs")) {
        return ActionResult::Err(format!("could not create the change directory: {e}"));
    }
    if let Err(e) = std::fs::write(change_dir.join("proposal.md"), proposal_stub(&idea)) {
        return ActionResult::Err(format!("could not write proposal.md: {e}"));
    }
    let change_rel = rel(&root, &change_dir);

    // ── Agent session ────────────────────────────────────────────────────────
    //
    // Rooted at the repository, not the change directory: OpenSpec asks an
    // author to read the existing `openspec/specs/` before proposing, and an
    // agent confined to the new directory cannot.
    let mut session: Option<serde_json::Value> = None;
    let name = format!("{slug} (spec)");
    match ws.add_project(
        name.clone(),
        root.to_string_lossy().into_owned(),
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
                &brief(&idea, &change_rel),
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
        "change": slug,
        "path": change_rel,
        "session": session,
    })))
}

#[cfg(test)]
mod tests {
    use super::{brief, prompt_args, proposal_stub, read_change, rel, resolve_in_root};
    use std::path::Path;

    fn tmpdir(tag: &str) -> std::path::PathBuf {
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

    #[test]
    fn rel_uses_forward_slashes() {
        let root = Path::new("/a/b");
        assert_eq!(rel(root, Path::new("/a/b/c/d.md")), "c/d.md");
    }

    #[test]
    fn change_lists_only_artifacts_that_exist_in_convention_order() {
        let root = tmpdir("artifacts");
        let dir = root.join("openspec/changes/add-login");
        std::fs::create_dir_all(dir.join("specs")).unwrap();
        // Written out of order, and with `design.md` absent, which is the
        // normal shape of a change that hasn't needed a design yet.
        std::fs::write(dir.join("tasks.md"), "x").unwrap();
        std::fs::write(dir.join("proposal.md"), "x").unwrap();
        std::fs::write(dir.join("specs/auth.md"), "x").unwrap();

        let c = read_change(&root, &dir, false);
        assert_eq!(c.name, "add-login");
        assert_eq!(
            c.artifacts.iter().map(|d| &d.name).collect::<Vec<_>>(),
            ["proposal.md", "tasks.md"]
        );
        assert_eq!(c.specs.len(), 1);
        assert_eq!(c.specs[0].path, "openspec/changes/add-login/specs/auth.md");
        assert!(!c.archived);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn traversal_out_of_the_repo_is_refused() {
        let root = tmpdir("traversal");
        std::fs::create_dir_all(root.join("openspec")).unwrap();
        let outside = root
            .parent()
            .unwrap()
            .join(format!("okena-specs-secret-{}.md", std::process::id()));
        std::fs::write(&outside, "secret").unwrap();

        let escape = format!(
            "openspec/../../{}",
            outside.file_name().unwrap().to_string_lossy()
        );
        let err = resolve_in_root(&root, &escape).unwrap_err();
        assert!(
            err.contains("outside") || err.contains("no such document"),
            "unexpected: {err}"
        );
        std::fs::remove_file(&outside).ok();
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_real_document_resolves() {
        let root = tmpdir("resolve");
        std::fs::create_dir_all(root.join("openspec/specs")).unwrap();
        std::fs::write(root.join("openspec/specs/auth.md"), "# Auth").unwrap();
        let got = resolve_in_root(&root, "openspec/specs/auth.md").unwrap();
        assert_eq!(std::fs::read_to_string(got).unwrap(), "# Auth");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_directory_is_not_readable_as_a_document() {
        let root = tmpdir("isdir");
        std::fs::create_dir_all(root.join("openspec/specs")).unwrap();
        assert!(resolve_in_root(&root, "openspec/specs").is_err());
        std::fs::remove_dir_all(&root).ok();
    }

    /// Build a repo with one active change, one archived change and a stable
    /// spec, then read it back through the real action.
    fn tree_of(root: &std::path::Path) -> okena_core::specs::SpecTree {
        let mut settings = crate::workspace::persistence::AppSettings::default();
        settings.harness.spec_repo = Some(root.to_string_lossy().into_owned());
        let super::ActionResult::Ok(Some(v)) = super::tree(&settings) else {
            panic!("expected a spec tree");
        };
        serde_json::from_value(v).unwrap()
    }

    fn populated_repo(tag: &str) -> std::path::PathBuf {
        let root = tmpdir(tag);
        let os = root.join("openspec");
        std::fs::create_dir_all(os.join("specs")).unwrap();
        std::fs::write(os.join("specs/auth.md"), "# Auth").unwrap();
        std::fs::create_dir_all(os.join("changes/add-login/specs")).unwrap();
        std::fs::write(os.join("changes/add-login/proposal.md"), "# Why").unwrap();
        std::fs::write(os.join("changes/add-login/specs/login.md"), "# Login").unwrap();
        std::fs::create_dir_all(os.join("changes/archive/old-thing")).unwrap();
        std::fs::write(os.join("changes/archive/old-thing/proposal.md"), "# Old").unwrap();
        root
    }

    #[test]
    fn the_archive_is_read_separately_and_never_as_an_active_change() {
        // `archive` is a directory sitting among the change directories, so the
        // obvious readdir lists it as a change named "archive" holding nothing.
        let root = populated_repo("tree");
        let t = tree_of(&root);

        assert!(t.initialized);
        assert_eq!(
            t.changes.iter().map(|c| &c.name).collect::<Vec<_>>(),
            ["add-login"]
        );
        assert_eq!(
            t.archived.iter().map(|c| &c.name).collect::<Vec<_>>(),
            ["old-thing"]
        );
        assert!(t.archived[0].archived);
        assert_eq!(t.specs.len(), 1, "stable specs");
        assert_eq!(t.changes[0].specs.len(), 1, "the change's own specs");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_repo_without_openspec_reads_as_uninitialized_not_as_an_error() {
        // This is every spec repo before its first change; failing here would
        // make a brand-new repo look broken.
        let root = tmpdir("empty");
        let t = tree_of(&root);
        assert!(!t.initialized);
        assert!(t.changes.is_empty() && t.specs.is_empty());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn an_unset_repository_is_reported_as_unset() {
        let settings = crate::workspace::persistence::AppSettings::default();
        let super::ActionResult::Err(e) = super::tree(&settings) else {
            panic!("expected an error");
        };
        assert!(e.contains("no spec repository"), "unhelpful: {e}");
    }

    #[test]
    fn a_whitespace_only_repository_setting_counts_as_unset() {
        // Otherwise it becomes a lookup for a directory named " ", reported as
        // "not found" — which sends the user hunting for a path they never set.
        let mut settings = crate::workspace::persistence::AppSettings::default();
        settings.harness.spec_repo = Some("   ".into());
        let super::ActionResult::Err(e) = super::tree(&settings) else {
            panic!("expected an error");
        };
        assert!(e.contains("no spec repository"), "unhelpful: {e}");
    }

    #[test]
    fn a_document_reads_back_through_the_action() {
        let root = populated_repo("read");
        let mut settings = crate::workspace::persistence::AppSettings::default();
        settings.harness.spec_repo = Some(root.to_string_lossy().into_owned());
        let super::ActionResult::Ok(Some(v)) =
            super::read(&settings, "openspec/specs/auth.md".into())
        else {
            panic!("expected content");
        };
        assert_eq!(v.get("content").unwrap().as_str().unwrap(), "# Auth");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn reading_outside_the_repository_is_refused_through_the_action() {
        // The end-to-end version of the traversal guard: this action is
        // reachable by any client and by agents through okena's MCP server.
        let root = populated_repo("escape");
        let secret = root
            .parent()
            .unwrap()
            .join(format!("okena-specs-outside-{}.md", std::process::id()));
        std::fs::write(&secret, "SECRET").unwrap();
        let mut settings = crate::workspace::persistence::AppSettings::default();
        settings.harness.spec_repo = Some(root.to_string_lossy().into_owned());

        let path = format!(
            "openspec/../../{}",
            secret.file_name().unwrap().to_string_lossy()
        );
        let result = super::read(&settings, path);
        assert!(
            matches!(result, super::ActionResult::Err(_)),
            "traversal was not refused"
        );
        std::fs::remove_file(&secret).ok();
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn an_explicit_name_beats_slugging_the_prompt() {
        // The prompt is a paragraph now; slugging it gives a truncated,
        // unreadable directory, and the directory is the change's identity.
        let prompt = "Let users sign in with Google, alongside the existing \
                      email flow, without breaking saved sessions";
        assert_eq!(
            okena_core::specs::change_slug("add-login"),
            "add-login",
            "an explicit name passes through"
        );
        let derived = okena_core::specs::change_slug(prompt);
        assert!(derived.len() <= 48);
        assert_ne!(derived, "add-login");
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

    #[test]
    fn the_brief_names_the_directory_and_the_artifacts() {
        let b = brief("Add login", "openspec/changes/add-login");
        assert!(b.contains("openspec/changes/add-login"));
        for f in okena_core::specs::CHANGE_ARTIFACTS {
            assert!(b.contains(f), "brief never mentions {f}");
        }
    }
}
