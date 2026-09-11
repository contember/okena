//! Engineering-harness knowledge actions, over `okena-knowledge` (ADR-0003).
//!
//! Reads are scoped to roots discovery found and path-checked against them:
//! these actions are reachable by any paired client and by agents through
//! okena's MCP server, so they must not become a way to read arbitrary files.
//!
//! None of them touches the workspace, and all of them may run git, so the
//! daemon runs them off its lock with the project list copied out by
//! [`knowledge_project_sources`].

use super::ActionResult;
use crate::workspace::persistence::{AppSettings, get_config_dir};
use crate::workspace::state::ProjectData;
use okena_core::api::ActionRequest;
use okena_core::knowledge::{KnowledgeDocument, KnowledgeRoot, KnowledgeRootKind, KnowledgeStores};
use okena_knowledge::discover::{self, ProjectSource, Sources};
use okena_knowledge::registry::{self, RegisterOutcome};
use okena_knowledge::setup::{self, SetupRequest};
use okena_knowledge::{KnowledgeError, git, tree};
use std::path::Path;

/// Largest file `KnowledgeRead` returns. Knowledge is prose; past this it is
/// not, and streaming it through a JSON reply would stall the client.
const MAX_DOC_BYTES: u64 = 2 * 1024 * 1024;

/// The projects to look for knowledge in.
///
/// None when project discovery is off, and never a worktree (a second checkout
/// of a repo already listed) or an agent session (rooted above repos, not in
/// one).
pub fn knowledge_project_sources(
    projects: &[ProjectData],
    settings: &AppSettings,
) -> Vec<ProjectSource> {
    if !settings.harness.knowledge.projects {
        return Vec::new();
    }
    projects
        .iter()
        .filter(|p| p.worktree_info.is_none() && !p.is_any_agent_session())
        .map(|p| ProjectSource {
            name: p.name.clone(),
            path: okena_core::fs::expand_home(&p.path),
        })
        .collect()
}

/// Run a knowledge action; `None` for any other action.
pub fn execute_knowledge_action(
    action: &ActionRequest,
    projects: &[ProjectSource],
    settings: &AppSettings,
) -> Option<ActionResult> {
    execute_at(
        &registry::registry_path(&get_config_dir()),
        action,
        projects,
        settings,
    )
}

fn execute_at(
    registry: &Path,
    action: &ActionRequest,
    projects: &[ProjectSource],
    settings: &AppSettings,
) -> Option<ActionResult> {
    Some(match action {
        ActionRequest::KnowledgeStores => to_result(
            serde_json::to_value(stores(registry, projects)),
            "knowledge stores",
        ),
        ActionRequest::KnowledgeTree { root } => tree_of(registry, projects, root.as_deref()),
        ActionRequest::KnowledgeRead { root, path } => {
            read(registry, projects, root.as_deref(), path)
        }
        ActionRequest::KnowledgeStoreClone { url, path } => match git::clone_store(
            registry,
            url,
            path.as_deref(),
            &settings.harness.knowledge.clone_dir(),
        ) {
            Ok(out) => registered(out),
            Err(e) => failed(e),
        },
        ActionRequest::KnowledgeStoreRegister { path } => register(registry, path),
        ActionRequest::KnowledgeStoreUnregister { id } => {
            match registry::unregister(registry, id) {
                Ok(left) => ActionResult::Ok(Some(serde_json::json!({
                    "id": id,
                    "left_on_disk": left.to_string_lossy(),
                }))),
                Err(e) => failed(e),
            }
        }
        ActionRequest::KnowledgeStoreSetup {
            id,
            path,
            name,
            description,
            remote,
            init_git,
        } => {
            let request = SetupRequest {
                id: id.clone(),
                path: path.clone(),
                name: name.clone(),
                description: description.clone(),
                remote: remote.clone(),
                init_git: *init_git,
            };
            match setup::setup_store(registry, &request) {
                Ok(out) => ActionResult::Ok(Some(serde_json::json!({
                    "id": out.id,
                    "root": out.root.to_string_lossy(),
                    "git_initialized": out.git_initialized,
                    "committed": out.committed,
                }))),
                Err(e) => failed(e),
            }
        }
        ActionRequest::KnowledgeStoreFetch { root } => sync(registry, projects, root, false),
        ActionRequest::KnowledgeStorePull { root } => sync(registry, projects, root, true),
        _ => return None,
    })
}

fn to_result(value: serde_json::Result<serde_json::Value>, what: &str) -> ActionResult {
    match value {
        Ok(v) => ActionResult::Ok(Some(v)),
        Err(e) => ActionResult::Err(format!("could not serialize {what}: {e}")),
    }
}

fn failed(e: KnowledgeError) -> ActionResult {
    ActionResult::Err(e.to_string())
}

fn registered(out: RegisterOutcome) -> ActionResult {
    ActionResult::Ok(Some(serde_json::json!({
        "id": out.id,
        "root": out.root.to_string_lossy(),
        "already_registered": out.already_registered,
        "identity_missing": out.identity_missing,
    })))
}

fn discovered(registry: &Path, projects: &[ProjectSource]) -> KnowledgeStores {
    discover::discover(&Sources {
        registry_path: registry.to_path_buf(),
        projects: projects.to_vec(),
    })
}

/// Every root, with sync state on each store.
fn stores(registry: &Path, projects: &[ProjectSource]) -> KnowledgeStores {
    let mut stores = discovered(registry, projects);
    git::attach_status(&mut stores);
    stores
}

/// A usable root the client named, checked against what discovery found —
/// never a path taken on trust.
fn resolve_root(
    registry: &Path,
    projects: &[ProjectSource],
    key: Option<&str>,
) -> Result<KnowledgeRoot, String> {
    let stores = discovered(registry, projects);
    let root = match key.map(str::trim).filter(|k| !k.is_empty()) {
        Some(key) => stores.root(key).cloned().ok_or_else(|| {
            format!(
                "unknown knowledge root `{key}` — it is no longer discovered; refresh the Knowledge view"
            )
        })?,
        None => stores.default_root().cloned().ok_or_else(|| {
            "no knowledge stores yet — clone or add one in Settings → Knowledge".to_string()
        })?,
    };
    if !root.healthy {
        let why = root
            .status
            .first()
            .map(|d| d.message.clone())
            .unwrap_or_else(|| "it is not a usable knowledge root".into());
        return Err(format!("can't open `{}`: {why}", root.name));
    }
    Ok(root)
}

fn tree_of(registry: &Path, projects: &[ProjectSource], key: Option<&str>) -> ActionResult {
    let root = match resolve_root(registry, projects, key) {
        Ok(r) => r,
        Err(e) => return ActionResult::Err(e),
    };
    let mut t = tree::read_tree(Path::new(&root.path));
    t.root_key = root.key;
    t.store_id = root.store_id;
    to_result(serde_json::to_value(&t), "knowledge tree")
}

fn read(
    registry: &Path,
    projects: &[ProjectSource],
    key: Option<&str>,
    path: &str,
) -> ActionResult {
    let root = match resolve_root(registry, projects, key) {
        Ok(r) => r,
        Err(e) => return ActionResult::Err(e),
    };
    let real = match tree::resolve_document(Path::new(&root.path), path) {
        Ok(p) => p,
        Err(e) => return ActionResult::Err(e),
    };
    if let Ok(meta) = real.metadata()
        && meta.len() > MAX_DOC_BYTES
    {
        return ActionResult::Err(format!(
            "{path} is too large to display ({} KB)",
            meta.len() / 1024
        ));
    }
    match std::fs::read_to_string(&real) {
        Ok(content) => to_result(
            serde_json::to_value(KnowledgeDocument {
                root_key: root.key,
                path: path.to_string(),
                content,
            }),
            "knowledge document",
        ),
        Err(e) if e.kind() == std::io::ErrorKind::InvalidData => {
            ActionResult::Err(format!("{path} is not a text file"))
        }
        Err(e) => ActionResult::Err(format!("could not read {path}: {e}")),
    }
}

fn register(registry: &Path, path: &str) -> ActionResult {
    // Record the checkout's origin, when it is a checkout, for the settings list.
    let remote = registry::resolve_checkout(path)
        .ok()
        .filter(|p| okena_git::repository::is_repository_at_root(p))
        .and_then(|p| okena_git::repository::origin_url(&p));
    match registry::register(registry, path, remote) {
        Ok(out) => registered(out),
        Err(e) => failed(e),
    }
}

/// Fetch, or fetch and fast-forward, a store's checkout. Replies with its sync
/// state after.
fn sync(registry: &Path, projects: &[ProjectSource], key: &str, pull: bool) -> ActionResult {
    let root = match resolve_root(registry, projects, Some(key)) {
        Ok(r) => r,
        Err(e) => return ActionResult::Err(e),
    };
    if root.kind != KnowledgeRootKind::Store {
        return ActionResult::Err(format!(
            "`{}` is part of a project, not a store; sync it with the project's own git",
            root.name
        ));
    }
    let path = Path::new(&root.path);
    let synced = if pull {
        git::pull(path)
    } else {
        git::fetch(path).map(|()| git::status(path).unwrap_or_default())
    };
    match synced {
        Ok(status) => to_result(serde_json::to_value(status), "sync state"),
        Err(e) => failed(e),
    }
}

// ─── Drafting with an agent ─────────────────────────────────────────────────

/// Brief an agent to add to or update a knowledge root.
///
/// States the layout inline rather than assuming the agent knows it: most have
/// never seen it, and a wrong guess produces a plausible file in the wrong
/// place.
fn draft_brief(request: &str, root: &KnowledgeRoot) -> String {
    let what = match root.kind {
        KnowledgeRootKind::Store => "knowledge store",
        KnowledgeRootKind::Project => "project's knowledge folder",
    };
    let mut out = format!(
        "Add to or update this knowledge base: {request}\n\n\
         You are in `{path}`, a {what}. Its layout:\n\
         - `docs/**/*.md` — engineering principles, processes, architecture and runbooks.\n\
         - `skills/<name>/SKILL.md` — one skill per directory, in the Agent Skills format \
         (frontmatter `name` and `description`), with its supporting files beside it.\n\
         - `agents/<name>.md` — one subagent per file, in the Claude Code subagent format \
         (frontmatter `name`, `description`, optional `tools` and `model`).\n\
         - `templates/**/*.md` — prompt templates: frontmatter `for:` lists the launch flows \
         they apply to, and the body uses `{{placeholder}}` names.\n\n\
         Give every file frontmatter with a one-line `description` (plus `title` and `tags` \
         for docs): people and agents choose entries by it. Read the existing entries first, \
         and extend one rather than duplicating it. Keep it short and specific to how this \
         team works, and ask me about anything ambiguous rather than inventing policy.",
        path = root.path,
    );
    match root.kind {
        KnowledgeRootKind::Store => out.push_str(
            "\n\nThis is a shared git repository. Work on a new branch named \
             `knowledge/<short-topic>`, commit when done, and do not push unless I ask.",
        ),
        KnowledgeRootKind::Project => out
            .push_str("\n\nThese files live inside a project repository. Leave committing to me."),
    }
    out
}

/// The agent a draft session runs. Unlike a spec draft there is nothing to
/// scaffold, so a session without an agent would have no purpose.
fn draft_shell(
    settings: &AppSettings,
    agent_command: Option<&str>,
    prompt: &str,
) -> Result<okena_terminal::shell_config::ShellType, String> {
    super::specs::spec_agent_shell(settings, agent_command, prompt).ok_or_else(|| {
        "no agent to start — pick one, or set the agent command in Settings → Harness".to_string()
    })
}

/// Open an agent session in a knowledge root, briefed to write there.
///
/// Runs on the workspace path, unlike every other knowledge action: it
/// creates a session project.
#[allow(clippy::too_many_arguments)]
pub(super) fn draft(
    ws: &mut crate::workspace::state::Workspace,
    window_id: crate::workspace::state::WindowId,
    root: Option<String>,
    request: String,
    agent_command: Option<String>,
    backend: &dyn okena_terminal::backend::TerminalBackend,
    terminals: &okena_terminal::TerminalsRegistry,
    settings: &AppSettings,
    cx: &mut impl okena_workspace::context::WorkspaceCx,
) -> ActionResult {
    let request = request.trim().to_string();
    if request.is_empty() {
        return ActionResult::Err("say what to write first".into());
    }
    let projects = knowledge_project_sources(&ws.data.projects, settings);
    let registry = registry::registry_path(&get_config_dir());
    let root = match resolve_root(&registry, &projects, root.as_deref()) {
        Ok(r) => r,
        Err(e) => return ActionResult::Err(e),
    };
    let shell = match draft_shell(
        settings,
        agent_command.as_deref(),
        &draft_brief(&request, &root),
    ) {
        Ok(s) => s,
        Err(e) => return ActionResult::Err(e),
    };
    let name = format!(
        "{} (knowledge)",
        root.store_id.as_deref().unwrap_or(&root.name)
    );
    let project_id = match ws.add_project(
        name.clone(),
        root.path.clone(),
        true,
        &settings.hooks,
        window_id,
        cx,
    ) {
        Ok(id) => id,
        Err(e) => {
            return ActionResult::Err(format!("could not open a session in `{}`: {e}", root.name));
        }
    };
    // Marked and given its agent before the terminal spawns: the terminal
    // reads the project's shell as it starts, and the marker keeps the session
    // out of knowledge discovery from the first snapshot.
    if let Some(p) = ws.data.projects.iter_mut().find(|p| p.id == project_id) {
        p.custom_session = Some(format!("Knowledge: {request}"));
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
        log::warn!("[knowledge] draft session terminal failed to spawn: {e}");
    }
    ws.notify_data(cx);
    ActionResult::Ok(Some(serde_json::json!({
        "root": root.key,
        "project_id": project_id,
        "name": name,
    })))
}

#[cfg(test)]
mod draft_tests {
    use super::{draft_brief, draft_shell};
    use crate::workspace::persistence::AppSettings;
    use okena_core::knowledge::{KnowledgeRoot, KnowledgeRootKind};

    fn root(kind: KnowledgeRootKind) -> KnowledgeRoot {
        KnowledgeRoot {
            key: "store:acme-eng".into(),
            kind,
            name: "acme-eng".into(),
            path: "/k/eng".into(),
            store_id: Some("acme-eng".into()),
            description: None,
            remote: None,
            healthy: true,
            git: None,
            counts: Default::default(),
            used_by: Vec::new(),
            status: Vec::new(),
        }
    }

    #[test]
    fn the_brief_carries_the_request_the_layout_and_the_branch_rule_for_stores() {
        let brief = draft_brief(
            "document how CI caches work",
            &root(KnowledgeRootKind::Store),
        );
        for needle in [
            "document how CI caches work",
            "`/k/eng`",
            "docs/**/*.md",
            "skills/<name>/SKILL.md",
            "agents/<name>.md",
            "templates/**/*.md",
            "`{placeholder}`",
            "`description`",
            "knowledge/<short-topic>",
            "do not push",
        ] {
            assert!(
                brief.contains(needle),
                "brief is missing {needle:?}:\n{brief}"
            );
        }

        let project = draft_brief("x", &root(KnowledgeRootKind::Project));
        assert!(!project.contains("knowledge/<short-topic>"));
        assert!(project.contains("Leave committing to me"));
    }

    #[test]
    fn a_draft_needs_an_agent_to_start() {
        let settings = AppSettings::default();
        assert!(draft_shell(&settings, None, "p").is_err_and(|e| e.contains("Settings → Harness")));
        assert!(draft_shell(&settings, Some("  "), "p").is_err());
        assert!(draft_shell(&settings, Some("claude"), "p").is_ok());
    }
}

#[cfg(test)]
mod tests {
    use super::{ActionResult, execute_at, knowledge_project_sources};
    use crate::workspace::persistence::AppSettings;
    use crate::workspace::state::ProjectData;
    use okena_core::api::ActionRequest;
    use okena_core::knowledge::{KnowledgeDocument, KnowledgeStores, KnowledgeTree};
    use okena_knowledge::discover::ProjectSource;
    use std::path::{Path, PathBuf};

    fn tmpdir(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "okena-knowledge-{tag}-{}-{}",
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

    /// A registry inside `sandbox`, so no test touches the real profile.
    fn registry(sandbox: &Path) -> PathBuf {
        okena_knowledge::registry::registry_path(&sandbox.join("config"))
    }

    fn store(root: &Path, id: &str) {
        write(
            &root.join(".okena-knowledge/store.yaml"),
            &format!("version: 1\nid: {id}\n"),
        );
        write(&root.join("docs/readme.md"), "# Readme\n");
    }

    fn run(sandbox: &Path, projects: &[ProjectSource], action: ActionRequest) -> ActionResult {
        execute_at(
            &registry(sandbox),
            &action,
            projects,
            &AppSettings::default(),
        )
        .expect("a knowledge action")
    }

    /// Decode a successful action's payload into whatever the binding asks for.
    /// A macro so the target type comes from the `let`: this crate has no
    /// direct `serde` dependency to name a deserialize bound with.
    macro_rules! ok {
        ($result:expr) => {
            match $result {
                ActionResult::Ok(Some(v)) => serde_json::from_value(v).unwrap(),
                ActionResult::Ok(None) => panic!("expected a payload"),
                ActionResult::Err(e) => panic!("expected success, got: {e}"),
            }
        };
    }

    fn err(result: ActionResult) -> String {
        match result {
            ActionResult::Err(e) => e,
            ActionResult::Ok(_) => panic!("expected an error"),
        }
    }

    fn project(json: serde_json::Value) -> ProjectData {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn project_sources_skip_worktrees_and_sessions_and_follow_the_setting() {
        let plain = project(serde_json::json!({"id": "p1", "name": "app", "path": "/r/app"}));
        let projects = [
            plain.clone(),
            project(serde_json::json!({
                "id": "p2", "name": "app-wt", "path": "/r/wt",
                "worktree_info": {"parent_project_id": "p1"},
            })),
            project(serde_json::json!({
                "id": "p3", "name": "spec", "path": "/r/s", "spec_change": "add-login",
            })),
            project(serde_json::json!({
                "id": "p4", "name": "agent", "path": "/r", "custom_session": "breakdown",
            })),
        ];
        let mut settings = AppSettings::default();
        let names: Vec<_> = knowledge_project_sources(&projects, &settings)
            .into_iter()
            .map(|p| p.name)
            .collect();
        assert_eq!(names, ["app"]);

        settings.harness.knowledge.projects = false;
        assert!(knowledge_project_sources(&[plain], &settings).is_empty());
    }

    #[test]
    fn register_list_open_and_unregister_through_the_actions() {
        let sandbox = tmpdir("lifecycle");
        let checkout = sandbox.join("eng");
        store(&checkout, "acme-eng");

        let registered: serde_json::Value = ok!(run(
            &sandbox,
            &[],
            ActionRequest::KnowledgeStoreRegister {
                path: checkout.to_string_lossy().into_owned(),
            },
        ));
        assert_eq!(registered["id"], "acme-eng");

        let stores: KnowledgeStores = ok!(run(&sandbox, &[], ActionRequest::KnowledgeStores));
        let root = stores.root("store:acme-eng").expect("listed");
        assert!(root.healthy);
        assert!(root.git.is_none(), "not a git checkout");

        let tree: KnowledgeTree = ok!(run(
            &sandbox,
            &[],
            ActionRequest::KnowledgeTree { root: None },
        ));
        assert_eq!(tree.root_key, "store:acme-eng");
        assert_eq!(tree.store_id.as_deref(), Some("acme-eng"));
        assert_eq!(tree.entries[0].title, "Readme");

        let doc: KnowledgeDocument = ok!(run(
            &sandbox,
            &[],
            ActionRequest::KnowledgeRead {
                root: Some("store:acme-eng".into()),
                path: "docs/readme.md".into(),
            },
        ));
        assert_eq!(doc.content, "# Readme\n");

        let _: serde_json::Value = ok!(run(
            &sandbox,
            &[],
            ActionRequest::KnowledgeStoreUnregister {
                id: "acme-eng".into(),
            },
        ));
        let stores: KnowledgeStores = ok!(run(&sandbox, &[], ActionRequest::KnowledgeStores));
        assert!(stores.roots.is_empty());
        assert!(checkout.join("docs/readme.md").is_file(), "left on disk");
        std::fs::remove_dir_all(&sandbox).ok();
    }

    #[test]
    fn undiscovered_keys_escaping_paths_and_huge_files_are_refused() {
        let sandbox = tmpdir("guards");
        let checkout = sandbox.join("eng");
        store(&checkout, "acme-eng");
        write(&sandbox.join("outside.md"), "SECRET");
        write(
            &checkout.join("docs/huge.md"),
            &"x".repeat(2 * 1024 * 1024 + 1),
        );
        okena_knowledge::registry::register(&registry(&sandbox), &checkout.to_string_lossy(), None)
            .unwrap();

        let key = format!("path:{}", sandbox.to_string_lossy());
        assert!(
            err(run(
                &sandbox,
                &[],
                ActionRequest::KnowledgeTree {
                    root: Some(key.clone())
                }
            ))
            .contains("unknown knowledge root")
        );
        assert!(
            err(run(
                &sandbox,
                &[],
                ActionRequest::KnowledgeRead {
                    root: Some(key),
                    path: "outside.md".into(),
                },
            ))
            .contains("unknown knowledge root")
        );
        assert!(
            !err(run(
                &sandbox,
                &[],
                ActionRequest::KnowledgeRead {
                    root: None,
                    path: "docs/../../outside.md".into(),
                },
            ))
            .is_empty()
        );
        assert!(
            err(run(
                &sandbox,
                &[],
                ActionRequest::KnowledgeRead {
                    root: None,
                    path: "docs/huge.md".into(),
                },
            ))
            .contains("too large")
        );
        std::fs::remove_dir_all(&sandbox).ok();
    }

    #[test]
    fn nothing_configured_says_where_to_configure_it() {
        let sandbox = tmpdir("empty");
        let e = err(run(
            &sandbox,
            &[],
            ActionRequest::KnowledgeTree { root: None },
        ));
        assert!(e.contains("Settings → Knowledge"), "unhelpful: {e}");
        std::fs::remove_dir_all(&sandbox).ok();
    }

    #[test]
    fn project_roots_open_but_do_not_sync() {
        let sandbox = tmpdir("project");
        let repo = sandbox.join("app");
        write(&repo.join(".okena/knowledge/docs/a.md"), "# A\n");
        let projects = [ProjectSource {
            name: "app".into(),
            path: repo.clone(),
        }];
        let stores: KnowledgeStores = ok!(run(&sandbox, &projects, ActionRequest::KnowledgeStores));
        let key = stores.roots[0].key.clone();

        let tree: KnowledgeTree = ok!(run(
            &sandbox,
            &projects,
            ActionRequest::KnowledgeTree {
                root: Some(key.clone()),
            },
        ));
        assert_eq!(tree.entries.len(), 1);
        assert!(tree.store_id.is_none());
        assert!(
            err(run(
                &sandbox,
                &projects,
                ActionRequest::KnowledgeStoreFetch { root: key },
            ))
            .contains("not a store")
        );
        std::fs::remove_dir_all(&sandbox).ok();
    }
}
