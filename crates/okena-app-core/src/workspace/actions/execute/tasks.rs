//! Engineering-harness task actions.
//!
//! The daemon owns the provider credential and every network call, so a thin
//! client (desktop, web, mobile) never holds a task-manager token and never
//! talks to Linear directly — it asks the daemon, exactly as it does for git.
//!
//! Task *lists* are deliberately not cached in workspace state: they're remote
//! data whose staleness is invisible to the user, and a stale issue list is
//! worse than a slow one. Only the task↔worktree link persists, on the project.

use super::ActionResult;
use crate::workspace::focus::FocusManager;
use crate::workspace::persistence::AppSettings;
use crate::workspace::state::{WindowId, Workspace};
use okena_core::tasks::{TaskAuthState, TaskAuthStatusResponse, TaskProviderStatus};
use okena_tasks::provider::{AuthStatus, Credential, TaskError, TaskProvider};
use okena_terminal::TerminalsRegistry;
use okena_terminal::backend::TerminalBackend;
use okena_workspace::context::WorkspaceCx;

/// Render a provider error for the UI.
///
/// `Unauthorized` and `NotAuthenticated` are worth distinguishing in the text
/// because they need different user action — reconnect vs connect — and the UI
/// surfaces the message verbatim.
fn describe(e: TaskError) -> String {
    e.to_string()
}

fn resolve(provider: &str) -> Result<Box<dyn TaskProvider>, String> {
    okena_tasks::provider_for(provider)
        .ok_or_else(|| format!("unknown task provider: `{provider}`"))
}

/// Map a provider's live auth state onto the shared wire type.
fn provider_status(p: &dyn TaskProvider) -> TaskProviderStatus {
    let auth = match p.auth_status() {
        AuthStatus::Disconnected => TaskAuthState::Disconnected,
        AuthStatus::Connected { account } => TaskAuthState::Connected { account },
        AuthStatus::Expired => TaskAuthState::Expired,
    };
    TaskProviderStatus {
        provider: p.id().to_string(),
        display_name: p.display_name().to_string(),
        auth,
    }
}

/// Auth state for every provider this build knows about. Local only.
pub(super) fn auth_status() -> ActionResult {
    let providers: Vec<TaskProviderStatus> = okena_tasks::KNOWN_PROVIDERS
        .iter()
        .filter_map(|id| okena_tasks::provider_for(id))
        .map(|p| provider_status(p.as_ref()))
        .collect();
    let response = TaskAuthStatusResponse { providers };
    match serde_json::to_value(&response) {
        Ok(v) => ActionResult::Ok(Some(v)),
        Err(e) => ActionResult::Err(format!("could not serialize auth status: {e}")),
    }
}

/// Verify an API key with one live call, then store it.
///
/// Verify-before-store is the point: a mistyped key that got written would fail
/// every later call with no obvious cause, and the user would have no signal
/// that the key — rather than the network — was the problem.
pub(super) fn connect_api_key(provider: String, api_key: String) -> ActionResult {
    let key = api_key.trim().to_string();
    if key.is_empty() {
        return ActionResult::Err("API key is empty".into());
    }
    // Construct a provider bound to the candidate key without touching disk.
    let candidate: Box<dyn TaskProvider> = match provider.as_str() {
        "linear" => Box::new(okena_tasks::LinearProvider::new(Some(Credential::ApiKey(
            key.clone(),
        )))),
        other => return ActionResult::Err(format!("unknown task provider: `{other}`")),
    };

    match candidate.list_assigned() {
        Ok(tasks) => {
            if let Err(e) = okena_tasks::store::save(&provider, &Credential::ApiKey(key)) {
                return ActionResult::Err(format!("could not store credential: {e}"));
            }
            // Re-read through the stored path so the reported status is what a
            // later call will actually see, not what we just held in hand.
            let stored = match resolve(&provider) {
                Ok(p) => provider_status(p.as_ref()),
                Err(e) => return ActionResult::Err(e),
            };
            match serde_json::to_value(&stored) {
                Ok(status) => ActionResult::Ok(Some(serde_json::json!({
                    "status": status,
                    "task_count": tasks.len(),
                }))),
                Err(e) => ActionResult::Err(format!("could not serialize status: {e}")),
            }
        }
        Err(e) => ActionResult::Err(describe(e)),
    }
}

pub(super) fn disconnect(provider: String) -> ActionResult {
    match okena_tasks::store::clear(&provider) {
        Ok(()) => ActionResult::Ok(Some(serde_json::json!({ "provider": provider }))),
        Err(e) => ActionResult::Err(format!("could not clear credential: {e}")),
    }
}

/// Teams or projects the user can file a new task in.
pub(super) fn containers(provider: String) -> ActionResult {
    let p = match resolve(&provider) {
        Ok(p) => p,
        Err(e) => return ActionResult::Err(e),
    };
    match p.list_containers() {
        Ok(list) => match serde_json::to_value(&list) {
            Ok(v) => ActionResult::Ok(Some(serde_json::json!({ "containers": v }))),
            Err(e) => ActionResult::Err(format!("could not serialize teams: {e}")),
        },
        Err(e) => ActionResult::Err(describe(e)),
    }
}

/// Create a task, optionally as a child of another.
pub(super) fn create(
    provider: String,
    title: String,
    description: String,
    kind: String,
    parent_external_id: Option<String>,
    container_id: Option<String>,
) -> ActionResult {
    let p = match resolve(&provider) {
        Ok(p) => p,
        Err(e) => return ActionResult::Err(e),
    };
    let description = description.trim().to_string();
    let draft = okena_tasks::provider::TaskDraft {
        title,
        // Empty means "no body", not an empty body — the provider should not
        // be asked to store a blank description.
        description: (!description.is_empty()).then_some(description),
        kind: parse_kind(&kind),
        parent_external_id,
        container_id,
    };
    match p.create_task(&draft) {
        Ok(task) => match serde_json::to_value(&task) {
            Ok(v) => ActionResult::Ok(Some(v)),
            Err(e) => ActionResult::Err(format!("could not serialize the new task: {e}")),
        },
        Err(e) => ActionResult::Err(describe(e)),
    }
}

/// Sub-tasks of a task, whoever they are assigned to.
pub(super) fn children(provider: String, task_external_id: String) -> ActionResult {
    let p = match resolve(&provider) {
        Ok(p) => p,
        Err(e) => return ActionResult::Err(e),
    };
    let id = okena_core::tasks::TaskId::new(provider, task_external_id);
    match p.list_children(&id) {
        Ok(tasks) => match serde_json::to_value(&tasks) {
            Ok(v) => ActionResult::Ok(Some(serde_json::json!({ "tasks": v }))),
            Err(e) => ActionResult::Err(format!("could not serialize sub-tasks: {e}")),
        },
        Err(e) => ActionResult::Err(describe(e)),
    }
}

/// Read a kind off the wire.
///
/// Unknown values fall back to `Task` rather than failing: a newer client — or
/// an agent guessing — asking for a kind this build does not model should still
/// get a task, not an error.
pub(super) fn parse_kind(raw: &str) -> okena_core::tasks::TaskKind {
    use okena_core::tasks::TaskKind;
    match raw.trim().to_ascii_lowercase().as_str() {
        "epic" | "initiative" => TaskKind::Epic,
        "feature" => TaskKind::Feature,
        "story" | "user story" => TaskKind::Story,
        "defect" | "bug" | "fix" | "hotfix" => TaskKind::Defect,
        _ => TaskKind::Task,
    }
}

/// Tasks assigned to the authenticated user.
pub(super) fn list(provider: String) -> ActionResult {
    let p = match resolve(&provider) {
        Ok(p) => p,
        Err(e) => return ActionResult::Err(e),
    };
    match p.list_assigned() {
        Ok(tasks) => match serde_json::to_value(&tasks) {
            Ok(v) => ActionResult::Ok(Some(serde_json::json!({
                "provider": provider,
                "tasks": v,
            }))),
            Err(e) => ActionResult::Err(format!("could not serialize tasks: {e}")),
        },
        Err(e) => ActionResult::Err(describe(e)),
    }
}

/// Substitute task placeholders in an agent argument.
///
/// Kept textual and explicit rather than a template engine: the only inputs are
/// four known fields, and a missing placeholder should leave the argument
/// untouched rather than erroring.
fn substitute(arg: &str, task: &okena_core::tasks::Task, branch: &str) -> String {
    arg.replace("{key}", &task.display_key)
        .replace("{title}", &task.title)
        .replace("{url}", &task.url)
        .replace("{branch}", branch)
}

/// The shell an agent session (or agent worktree) should run.
///
/// `None` when no agent command is configured — starting work then just creates
/// worktrees and leaves an ordinary shell. Launching an AI agent is opt-in.
fn agent_shell(
    settings: &AppSettings,
    override_command: Option<&str>,
    task: &okena_core::tasks::Task,
    branch: &str,
) -> Option<okena_terminal::shell_config::ShellType> {
    // An explicit override wins, including an explicit empty string, which is
    // how a caller says "worktrees only" despite a configured default.
    // Trim both paths: a whitespace-only value from either source must read as
    // "no agent", not become the program name.
    let command = match override_command {
        Some(c) => c,
        None => settings.harness.agent_command.as_deref().unwrap_or(""),
    }
    .trim()
    .to_string();
    if command.is_empty() {
        return None;
    }
    let mut args: Vec<String> = settings
        .harness
        .agent_args
        .iter()
        .map(|a| substitute(a, task, branch))
        .collect();
    // Hand the agent okena's MCP server so it can ask what task it is on and
    // report back without the user configuring anything.
    args.extend(super::agent_mcp::injection_args(&command, settings));

    Some(okena_terminal::shell_config::ShellType::Custom {
        path: command,
        args,
    })
}

/// Resolve the directory an agent session should run in.
///
/// Explicit argument wins, then the configured root, then the parent of the
/// first project — right for a `~/p/<repo>` layout, which is why the setting
/// exists for everyone else.
fn resolve_agent_root(
    explicit: Option<String>,
    settings: &AppSettings,
    first_project_path: &str,
) -> Option<String> {
    if let Some(root) = explicit.filter(|r| !r.trim().is_empty()) {
        return Some(root);
    }
    if let Some(root) = settings
        .harness
        .agent_root
        .clone()
        .filter(|r| !r.trim().is_empty())
    {
        return Some(root);
    }
    std::path::Path::new(first_project_path)
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
}

/// Start work on a task across one or more projects.
///
/// Order matters: worktrees are created first and links written second, so a
/// failure part-way leaves usable checkouts rather than links pointing at
/// nothing.
#[allow(clippy::too_many_arguments)]
pub(super) fn start_work(
    ws: &mut Workspace,
    window_id: WindowId,
    provider: String,
    task_external_id: String,
    project_ids: Vec<String>,
    agent_root: Option<String>,
    branch_override: Option<String>,
    agent_command: Option<String>,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
    settings: &AppSettings,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    if project_ids.is_empty() {
        return ActionResult::Err("pick at least one project to work in".into());
    }
    // Validate every target up front: creating three of four worktrees and
    // then discovering the fourth was bogus is worse than refusing early.
    for id in &project_ids {
        if ws.project(id).is_none() {
            return ActionResult::Err(format!("project not found: {id}"));
        }
    }

    let p = match resolve(&provider) {
        Ok(p) => p,
        Err(e) => return ActionResult::Err(e),
    };

    // Re-fetch rather than trusting a branch name the client supplied: the
    // client's list may be minutes old, and the branch name is what every
    // worktree — and the provider's branch-to-issue linking — is keyed on.
    let tasks = match p.list_assigned() {
        Ok(t) => t,
        Err(e) => return ActionResult::Err(describe(e)),
    };
    let task = match tasks.iter().find(|t| t.id.external_id == task_external_id) {
        Some(t) => t.clone(),
        None => {
            return ActionResult::Err(format!(
                "task `{task_external_id}` is not in your assigned list"
            ));
        }
    };

    // The provider's branch name keeps its branch-to-issue linking working, so
    // it stays the default; the user can still name it themselves.
    let branch = branch_override
        .map(|b| b.trim().to_string())
        .filter(|b| !b.is_empty())
        .unwrap_or_else(|| p.branch_name(&task));
    if branch.is_empty() {
        return ActionResult::Err(format!(
            "could not derive a branch name for {}",
            task.display_key
        ));
    }

    let task_ref = okena_core::tasks::TaskRef::from(&task);
    let first_project_path = ws
        .project(&project_ids[0])
        .map(|p| p.path.clone())
        .unwrap_or_default();

    // ── Worktrees, one per assigned project, all on the same branch ──────────
    let mut created: Vec<serde_json::Value> = Vec::new();
    let mut failed: Vec<serde_json::Value> = Vec::new();

    for project_id in &project_ids {
        let project_name = ws
            .project(project_id)
            .map(|p| p.name.clone())
            .unwrap_or_else(|| project_id.clone());

        let result = super::project::create_worktree(
            ws,
            window_id,
            project_id.clone(),
            branch.clone(),
            // New work by definition. An existing branch surfaces as a create
            // error rather than silently attaching to someone else's work.
            true,
            // Single-project work runs the agent in the worktree itself; with
            // several projects the agent session below is its home instead, so
            // the per-worktree terminals stay ordinary shells for the human.
            (project_ids.len() == 1)
                .then(|| agent_shell(settings, agent_command.as_deref(), &task, &branch))
                .flatten(),
            backend,
            terminals,
            settings,
            cx,
        );

        match result {
            ActionResult::Ok(Some(payload)) => {
                let new_id = payload
                    .get("project_id")
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
                match new_id {
                    Some(new_id) => {
                        link_task(ws, &new_id, &task_ref);
                        created.push(serde_json::json!({
                            "project": project_name,
                            "project_id": new_id,
                            "path": payload.get("path").cloned(),
                        }));
                    }
                    None => failed.push(serde_json::json!({
                        "project": project_name,
                        "error": "worktree creation returned no project id",
                    })),
                }
            }
            ActionResult::Ok(None) => failed.push(serde_json::json!({
                "project": project_name,
                "error": "worktree creation returned no project",
            })),
            ActionResult::Err(e) => {
                failed.push(serde_json::json!({ "project": project_name, "error": e }))
            }
        }
    }

    if created.is_empty() {
        let detail = failed
            .iter()
            .filter_map(|f| {
                let name = f.get("project")?.as_str()?;
                let err = f.get("error")?.as_str()?;
                Some(format!("{name}: {err}"))
            })
            .collect::<Vec<_>>()
            .join("; ");
        return ActionResult::Err(format!("no worktrees were created — {detail}"));
    }

    // ── Agent session ────────────────────────────────────────────────────────
    //
    // Only for a multi-project task: with a single project the worktree itself
    // is the working directory, and an extra project rooted above it would just
    // be clutter.
    let mut agent_session: Option<serde_json::Value> = None;
    if created.len() > 1
        && let Some(root) = resolve_agent_root(agent_root, settings, &first_project_path)
    {
        let name = format!("{} (agent)", task.display_key);
        match ws.add_project(
            name.clone(),
            root.clone(),
            // With a terminal: this is where the agent runs.
            true,
            &settings.hooks,
            window_id,
            cx,
        ) {
            Ok(session_id) => {
                link_task(ws, &session_id, &task_ref);
                // Set before spawning: the terminal reads the project's default
                // shell as it starts.
                if let Some(shell) = agent_shell(settings, agent_command.as_deref(), &task, &branch)
                    && let Some(p) = ws.data.projects.iter_mut().find(|p| p.id == session_id)
                {
                    p.default_shell = Some(shell);
                }
                let result = super::spawn_uninitialized_terminals(
                    ws,
                    &session_id,
                    backend,
                    terminals,
                    settings,
                    None,
                    cx,
                );
                if let ActionResult::Err(e) = result {
                    log::warn!("[tasks] agent session terminal failed to spawn: {e}");
                }
                agent_session = Some(serde_json::json!({
                    "project_id": session_id,
                    "name": name,
                    "root": root,
                }));
            }
            // The worktrees are real and usable even if the session project
            // couldn't be created, so report rather than fail the whole call.
            Err(e) => failed.push(serde_json::json!({
                "project": "agent session",
                "error": e,
            })),
        }
    }

    ws.notify_data(cx);

    ActionResult::Ok(Some(serde_json::json!({
        "task": task_ref,
        "branch": branch,
        "created": created,
        "failed": failed,
        "agent_session": agent_session,
    })))
}

/// Point a project at the task it was created for.
fn link_task(ws: &mut Workspace, project_id: &str, task_ref: &okena_core::tasks::TaskRef) {
    if let Some(project) = ws.data.projects.iter_mut().find(|p| p.id == project_id) {
        project.task_ref = Some(task_ref.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pull the error string out of a result, failing loudly on `Ok`.
    fn err_of(r: ActionResult) -> String {
        match r {
            ActionResult::Err(e) => e,
            ActionResult::Ok(v) => panic!("expected an error, got Ok({v:?})"),
        }
    }

    // These run with no profile initialized, so the credential store resolves
    // to "nothing stored" — which is exactly the state a fresh install is in.

    #[test]
    fn auth_status_lists_every_known_provider() {
        let ActionResult::Ok(Some(v)) = auth_status() else {
            panic!("auth_status should always succeed — it makes no network call");
        };
        // Decode through the shared wire type: this pins that what the daemon
        // emits is exactly what a client can parse.
        let decoded: TaskAuthStatusResponse =
            serde_json::from_value(v).expect("daemon output must decode as the shared type");
        assert_eq!(decoded.providers.len(), okena_tasks::KNOWN_PROVIDERS.len());
        let linear = decoded.provider("linear").expect("linear should be listed");
        assert_eq!(linear.display_name, "Linear");
        assert_eq!(linear.auth, TaskAuthState::Disconnected);
    }

    #[test]
    fn empty_api_key_is_rejected_before_any_network_call() {
        // Whitespace-only must be caught too — otherwise it reaches Linear as a
        // valid-looking header and fails with a confusing 401 instead.
        assert!(err_of(connect_api_key("linear".into(), "   ".into())).contains("empty"));
    }

    #[test]
    fn unknown_provider_is_an_error_not_a_silent_noop() {
        // A newer client asking an older daemon for a provider it lacks should
        // say so plainly rather than appearing to succeed.
        assert!(
            err_of(connect_api_key("jira".into(), "k".into())).contains("unknown task provider")
        );
        assert!(err_of(list("jira".into())).contains("unknown task provider"));
    }

    #[test]
    fn listing_without_a_credential_reports_not_authenticated() {
        let e = err_of(list("linear".into()));
        assert!(
            e.contains("not authenticated"),
            "expected a not-authenticated message, got: {e}"
        );
    }

    #[test]
    fn credential_writes_refuse_when_no_profile_is_active() {
        // Every real run initializes a profile before serving actions, so this
        // path only fires on a misconfiguration — and it must say so rather
        // than reporting a success that wrote nothing to disk.
        let e = err_of(disconnect("linear".into()));
        assert!(
            e.contains("no active profile"),
            "expected the missing-profile reason to surface, got: {e}"
        );
    }
}

#[cfg(test)]
mod agent_root_tests {
    use super::resolve_agent_root;
    use crate::workspace::persistence::AppSettings;

    fn settings_with_root(root: Option<&str>) -> AppSettings {
        let mut s = AppSettings::default();
        s.harness.agent_root = root.map(str::to_string);
        s
    }

    #[test]
    fn explicit_argument_wins() {
        let s = settings_with_root(Some("/configured"));
        assert_eq!(
            resolve_agent_root(Some("/explicit".into()), &s, "/Users/me/p/repo").as_deref(),
            Some("/explicit")
        );
    }

    #[test]
    fn falls_back_to_the_configured_root() {
        let s = settings_with_root(Some("/configured"));
        assert_eq!(
            resolve_agent_root(None, &s, "/Users/me/p/repo").as_deref(),
            Some("/configured")
        );
    }

    #[test]
    fn falls_back_to_the_projects_parent_directory() {
        // The `~/p/<repo>` layout: the agent runs one level above the repo so
        // it can see every sibling worktree.
        let s = settings_with_root(None);
        assert_eq!(
            resolve_agent_root(None, &s, "/Users/me/p/repo").as_deref(),
            Some("/Users/me/p")
        );
    }

    #[test]
    fn blank_values_are_treated_as_unset() {
        // A whitespace-only setting would otherwise become the agent's cwd.
        let s = settings_with_root(Some("   "));
        assert_eq!(
            resolve_agent_root(Some("  ".into()), &s, "/Users/me/p/repo").as_deref(),
            Some("/Users/me/p")
        );
    }

    #[test]
    fn a_root_level_path_has_no_parent_fallback() {
        let s = settings_with_root(None);
        assert_eq!(resolve_agent_root(None, &s, "/"), None);
    }
}

#[cfg(test)]
pub(super) mod agent_shell_tests {
    use super::{agent_shell, substitute};
    use crate::workspace::persistence::AppSettings;
    use okena_core::tasks::{Task, TaskId, TaskState};
    use okena_terminal::shell_config::ShellType;

    pub(super) fn task() -> Task {
        Task {
            id: TaskId::new("linear", "uuid-1"),
            display_key: "LIN-42".into(),
            title: "Ship the harness".into(),
            description: None,
            state: TaskState::Todo,
            state_name: "Todo".into(),
            url: "https://linear.app/x/issue/LIN-42".into(),
            branch_name: "nima/lin-42-ship".into(),
            updated_at: "2026-09-02T00:00:00Z".into(),
            kind: okena_core::tasks::TaskKind::Task,
            parent_id: None,
            parent_key: None,
            labels: Vec::new(),
            groups: Vec::new(),
        }
    }

    #[test]
    fn substitutes_every_placeholder() {
        let got = substitute("{key}: {title} ({url}) on {branch}", &task(), "b1");
        assert_eq!(
            got,
            "LIN-42: Ship the harness (https://linear.app/x/issue/LIN-42) on b1"
        );
    }

    #[test]
    fn leaves_unknown_placeholders_alone() {
        // A typo'd placeholder should reach the agent verbatim rather than
        // silently becoming an empty string.
        assert_eq!(substitute("{nope}", &task(), "b"), "{nope}");
    }

    #[test]
    fn no_agent_configured_means_no_launch() {
        // Starting work must not spawn an AI agent unless asked to.
        let s = AppSettings::default();
        assert!(agent_shell(&s, None, &task(), "b").is_none());
    }

    #[test]
    fn blank_command_is_treated_as_unset() {
        let mut s = AppSettings::default();
        s.harness.agent_command = Some("   ".into());
        assert!(agent_shell(&s, None, &task(), "b").is_none());
    }

    #[test]
    fn builds_a_custom_shell_with_substituted_args() {
        let mut s = AppSettings::default();
        s.harness.agent_command = Some("claude".into());
        s.harness.agent_args = vec!["Work on {key}: {title}".into()];
        match agent_shell(&s, None, &task(), "b1").expect("configured") {
            ShellType::Custom { path, args } => {
                assert_eq!(path, "claude");
                assert_eq!(args, vec!["Work on LIN-42: Ship the harness".to_string()]);
            }
            other => panic!("expected a custom shell, got {other:?}"),
        }
    }

    #[test]
    fn command_without_args_still_launches() {
        let mut s = AppSettings::default();
        s.harness.agent_command = Some("codex".into());
        match agent_shell(&s, None, &task(), "b").expect("configured") {
            ShellType::Custom { path, args } => {
                assert_eq!(path, "codex");
                assert!(args.is_empty());
            }
            other => panic!("expected a custom shell, got {other:?}"),
        }
    }
}

// ─── Agent reporting ─────────────────────────────────────────────────────────

/// Current wall-clock in Unix millis, or 0 if the clock is before the epoch.
fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Record an asset an agent produced.
pub(super) fn register_asset(
    ws: &mut Workspace,
    project_id: String,
    kind: String,
    title: String,
    url: Option<String>,
    project: Option<String>,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    let title = title.trim().to_string();
    if title.is_empty() {
        return ActionResult::Err("asset title is empty".into());
    }
    // An unrecognized kind decodes to `Other` rather than failing: an agent
    // reporting something this build doesn't model should still be visible.
    let kind: okena_core::harness::AgentAssetKind =
        serde_json::from_value(serde_json::Value::String(kind))
            .unwrap_or(okena_core::harness::AgentAssetKind::Other);

    let asset = okena_core::harness::AgentAsset {
        kind,
        title,
        url: url.filter(|u| !u.trim().is_empty()),
        project: project.filter(|p| !p.trim().is_empty()),
        created_at: now_millis(),
    };

    let Some(p) = ws.data.projects.iter_mut().find(|p| p.id == project_id) else {
        return ActionResult::Err(format!("project not found: {project_id}"));
    };
    let state = p.agent.get_or_insert_with(Default::default);
    state.assets.push(asset);
    let count = state.assets.len();
    ws.notify_data(cx);

    ActionResult::Ok(Some(serde_json::json!({
        "project_id": project_id,
        "asset_count": count,
    })))
}

/// Set the status an agent reports for its session.
pub(super) fn report_status(
    ws: &mut Workspace,
    project_id: String,
    status: String,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    let status = status.trim().to_string();
    let Some(p) = ws.data.projects.iter_mut().find(|p| p.id == project_id) else {
        return ActionResult::Err(format!("project not found: {project_id}"));
    };
    let state = p.agent.get_or_insert_with(Default::default);
    // An empty status clears it rather than displaying a blank line.
    state.status = (!status.is_empty()).then_some(status);
    ws.notify_data(cx);
    ActionResult::Ok(Some(serde_json::json!({ "project_id": project_id })))
}

#[cfg(test)]
mod agent_override_tests {
    use super::agent_shell;
    use super::agent_shell_tests::task;
    use crate::workspace::persistence::AppSettings;
    use okena_terminal::shell_config::ShellType;

    #[test]
    fn an_explicit_command_overrides_the_configured_one() {
        let mut s = AppSettings::default();
        s.harness.agent_command = Some("claude".into());
        match agent_shell(&s, Some("codex"), &task(), "b").expect("override applies") {
            ShellType::Custom { path, .. } => assert_eq!(path, "codex"),
            other => panic!("expected a custom shell, got {other:?}"),
        }
    }

    #[test]
    fn an_empty_override_means_no_agent_despite_a_default() {
        // "Worktrees only" must be expressible even when a default is set.
        let mut s = AppSettings::default();
        s.harness.agent_command = Some("claude".into());
        assert!(agent_shell(&s, Some(""), &task(), "b").is_none());
        assert!(agent_shell(&s, Some("   "), &task(), "b").is_none());
    }

    #[test]
    fn no_override_falls_back_to_the_setting() {
        let mut s = AppSettings::default();
        s.harness.agent_command = Some("claude".into());
        match agent_shell(&s, None, &task(), "b").expect("falls back") {
            ShellType::Custom { path, .. } => assert_eq!(path, "claude"),
            other => panic!("expected a custom shell, got {other:?}"),
        }
    }
}

/// Start a free-form agent session the user configured themselves.
///
/// Shares every mechanism with the task and spec routes — a session project, an
/// agent launched with okena's MCP wired in — and differs only in where the
/// brief comes from. Kept here beside `start_work` so the three stay in step.
#[allow(clippy::too_many_arguments)]
pub(super) fn start_custom_session(
    ws: &mut Workspace,
    window_id: WindowId,
    goal: String,
    name: String,
    root: String,
    project_ids: Vec<String>,
    agent_command: Option<String>,
    task: Option<okena_core::tasks::TaskRef>,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
    settings: &AppSettings,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    let goal = goal.trim().to_string();
    if goal.is_empty() {
        return ActionResult::Err("describe what the agent should do first".into());
    }

    // Every selected project must exist before anything is created: pointing an
    // agent at a project that has since gone is worth saying, not ignoring.
    let mut context: Vec<(String, String)> = Vec::new();
    for id in &project_ids {
        match ws.project(id) {
            Some(p) => context.push((p.name.clone(), p.path.clone())),
            None => return ActionResult::Err(format!("project not found: {id}")),
        }
    }

    let Some(root) = resolve_session_root(&root, &context, settings) else {
        return ActionResult::Err(
            "pick a working directory, a project, or set a projects root in Settings → Harness"
                .into(),
        );
    };
    if !std::path::Path::new(&root).is_dir() {
        return ActionResult::Err(format!("working directory not found: {root}"));
    }

    let label = session_label(&name, &goal);
    let display = format!("{label} (agent)");

    let session_id = match ws.add_project(
        display.clone(),
        root.clone(),
        // With a terminal: this is where the agent runs.
        true,
        &settings.hooks,
        window_id,
        cx,
    ) {
        Ok(id) => id,
        Err(e) => return ActionResult::Err(format!("could not create the session: {e}")),
    };

    // Mark it before spawning, so it is recognizable as a session from the
    // first snapshot the client sees.
    if let Some(p) = ws.data.projects.iter_mut().find(|p| p.id == session_id) {
        p.custom_session = Some(label.clone());
        // Both markers: it is a free-form session (so it gets no worktrees and
        // does not read as work in progress) that is nonetheless about a task,
        // so the task can list it and the agent's own MCP calls resolve it.
        p.task_ref = task;
    }

    let brief = custom_brief(&goal, &context);
    if let Some(shell) = custom_agent_shell(settings, agent_command.as_deref(), &brief)
        && let Some(p) = ws.data.projects.iter_mut().find(|p| p.id == session_id)
    {
        p.default_shell = Some(shell);
    }

    if let ActionResult::Err(e) = super::spawn_uninitialized_terminals(
        ws,
        &session_id,
        backend,
        terminals,
        settings,
        None,
        cx,
    ) {
        log::warn!("[agents] session terminal failed to spawn: {e}");
    }

    ws.notify_data(cx);

    ActionResult::Ok(Some(serde_json::json!({
        "project_id": session_id,
        "name": display,
        "root": root,
    })))
}

/// Where a custom session runs.
///
/// An explicit directory wins. Otherwise a single selected project runs in
/// itself, and several run at the configured projects root — an agent given
/// three repos needs a directory above all of them, the same reasoning
/// `start_work` uses for a multi-project task.
fn resolve_session_root(
    root: &str,
    context: &[(String, String)],
    settings: &AppSettings,
) -> Option<String> {
    let explicit = root.trim();
    if !explicit.is_empty() {
        return Some(expand_home(explicit));
    }
    if context.len() == 1 {
        return Some(context[0].1.clone());
    }
    settings
        .harness
        .agent_root
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(expand_home)
        // With several projects and no root configured, fall back to the parent
        // of the first, which is right for the common `~/p/<repo>` layout.
        .or_else(|| {
            context.first().and_then(|(_, path)| {
                std::path::Path::new(path)
                    .parent()
                    .map(|p| p.to_string_lossy().into_owned())
            })
        })
}

/// Expand a leading `~`. Settings and this dialog are both hand-typed, and
/// `~/p` is the form a person writes.
fn expand_home(p: &str) -> String {
    if let Some(rest) = p.strip_prefix("~/")
        && let Some(home) = dirs::home_dir()
    {
        return home.join(rest).to_string_lossy().into_owned();
    }
    p.to_string()
}

/// A short label for the session.
///
/// The user's name if they gave one; otherwise the first few words of the goal,
/// because a session listed as the whole paragraph is unreadable in a sidebar.
fn session_label(name: &str, goal: &str) -> String {
    let name = name.trim();
    if !name.is_empty() {
        return name.to_string();
    }
    let mut label: String = goal
        .split_whitespace()
        .take(6)
        .collect::<Vec<_>>()
        .join(" ");
    if label.len() > 48 {
        label.truncate(48);
        label = label.trim_end().to_string();
    }
    if label.is_empty() {
        "agent".to_string()
    } else {
        label
    }
}

/// The opening prompt for a custom session.
///
/// The goal verbatim, plus the projects the user pointed the agent at. The
/// paths matter: the session may be rooted above them, where "the project" is
/// ambiguous until they are named.
fn custom_brief(goal: &str, context: &[(String, String)]) -> String {
    if context.is_empty() {
        return goal.to_string();
    }
    let list = context
        .iter()
        .map(|(name, path)| format!("- {name} ({path})"))
        .collect::<Vec<_>>()
        .join("\n");
    format!("{goal}\n\nProjects you were given:\n{list}")
}

/// Shell for a custom agent session.
fn custom_agent_shell(
    settings: &AppSettings,
    override_command: Option<&str>,
    brief: &str,
) -> Option<okena_terminal::shell_config::ShellType> {
    // An explicit empty string means "no agent, just a shell here", even when a
    // default agent is configured — same contract as the other two routes.
    let command = match override_command {
        Some(c) => c,
        None => settings.harness.agent_command.as_deref().unwrap_or(""),
    }
    .trim()
    .to_string();
    if command.is_empty() {
        return None;
    }
    let mut args = super::specs::prompt_args(&command, brief);
    args.extend(super::agent_mcp::injection_args(&command, settings));
    Some(okena_terminal::shell_config::ShellType::Custom {
        path: command,
        args,
    })
}

/// What a teardown removes: checkouts on disk, and sessions that own only
/// terminals.
pub(super) struct TeardownPlan {
    /// `(id, name)` of each worktree to remove, checkout and all.
    pub worktrees: Vec<(String, String)>,
    /// `(id, name)` of each session project to drop.
    pub sessions: Vec<(String, String)>,
}

/// Decide what tearing down `project_id` should remove.
///
/// A task session owns the worktrees created for its task, in other repos, and
/// they go with it. A spec session owns nothing on disk — it runs *in* the
/// user's spec repository, which must survive — so only the session goes.
/// `None` for anything that is not a session: this route deletes checkouts, so
/// it refuses rather than guessing.
pub(super) fn teardown_plan(
    projects: &[okena_workspace::state::ProjectData],
    project_id: &str,
) -> Option<TeardownPlan> {
    let anchor = projects.iter().find(|p| p.id == project_id)?;

    // Neither a spec session nor a free-form one owns a checkout: the first
    // runs in the user's spec repository and the second in a directory they
    // chose, both of which must survive. Only the session goes.
    if anchor.is_spec_session() || anchor.is_custom_session() {
        return Some(TeardownPlan {
            worktrees: Vec::new(),
            sessions: vec![(anchor.id.clone(), anchor.name.clone())],
        });
    }

    // A worktree carries its task too, but it is a checkout, not a session —
    // deleting one must not take its siblings with it.
    let task = anchor
        .task_ref
        .as_ref()
        .filter(|_| anchor.worktree_info.is_none())?;

    let mut plan = TeardownPlan {
        worktrees: Vec::new(),
        sessions: Vec::new(),
    };
    for p in projects.iter() {
        let linked = p
            .task_ref
            .as_ref()
            .is_some_and(|t| t.id.external_id == task.id.external_id);
        if !linked {
            continue;
        }
        if p.worktree_info.is_some() {
            plan.worktrees.push((p.id.clone(), p.name.clone()));
        } else {
            plan.sessions.push((p.id.clone(), p.name.clone()));
        }
    }
    Some(plan)
}

/// Tear down a task's whole workspace.
///
/// Order is deliberate: worktrees first, session last. The session project is
/// how the user finds this workspace in the sidebar, so removing it first would
/// strand any worktree that failed to delete with no obvious route back to it.
///
/// Failures are collected rather than aborting: a dirty worktree that git
/// refuses to remove should not prevent the rest from being cleaned up, and the
/// result names exactly what survived and why.
pub(super) fn delete_workspace(
    ws: &mut Workspace,
    focus_manager: &mut FocusManager,
    project_id: String,
    force: bool,
    settings: &AppSettings,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    let Some(anchor) = ws.project(&project_id) else {
        return ActionResult::Err(format!("project not found: {project_id}"));
    };

    // What to tear down depends on the kind of session.
    //
    // A task session owns the worktrees created for its task, in other repos,
    // and they go with it. A spec session owns nothing on disk — it runs *in*
    // the user's spec repository, which must survive — so only the session
    // itself goes. Anything that is not a session is refused rather than
    // guessed at: this route deletes checkouts.
    let task = anchor.task_ref.clone();
    let Some(plan) = teardown_plan(&ws.data.projects, &project_id) else {
        return ActionResult::Err(
            "this project is not an agent session — nothing to tear down".into(),
        );
    };
    let (worktrees, sessions) = (plan.worktrees, plan.sessions);

    let mut removed: Vec<serde_json::Value> = Vec::new();
    let mut failed: Vec<serde_json::Value> = Vec::new();

    for (id, name) in worktrees {
        // Removes the checkout and closes the project's terminals, which ends
        // the tmux session and with it any agent running inside.
        match super::project::remove_worktree_project(
            ws,
            focus_manager,
            id.clone(),
            force,
            settings,
            cx,
        ) {
            ActionResult::Ok(_) => removed.push(serde_json::json!({
                "project": name,
                "kind": "worktree",
            })),
            ActionResult::Err(error) => failed.push(serde_json::json!({
                "project": name,
                "kind": "worktree",
                "error": error,
            })),
        }
    }

    for (id, name) in sessions {
        match super::project::delete_project(ws, focus_manager, id.clone(), settings, cx) {
            ActionResult::Ok(_) => removed.push(serde_json::json!({
                "project": name,
                "kind": "agent session",
            })),
            ActionResult::Err(error) => failed.push(serde_json::json!({
                "project": name,
                "kind": "agent session",
                "error": error,
            })),
        }
    }

    ws.notify_data(cx);

    ActionResult::Ok(Some(serde_json::json!({
        "task": task,
        "removed": removed,
        "failed": failed,
    })))
}

#[cfg(test)]
mod teardown_tests {
    use super::teardown_plan;
    use okena_workspace::state::ProjectData;

    fn project(json: serde_json::Value) -> ProjectData {
        serde_json::from_value(json).unwrap()
    }

    fn task_session(id: &str, external: &str) -> ProjectData {
        project(serde_json::json!({
            "id": id, "name": format!("{id} (agent)"), "path": "/p",
            "task_ref": {
                "id": { "provider": "linear", "external_id": external },
                "display_key": "QBL-1", "title": "t", "url": "http://x",
            },
        }))
    }

    fn worktree(id: &str, external: &str) -> ProjectData {
        project(serde_json::json!({
            "id": id, "name": format!("okena ({id})"), "path": format!("/p/wt/{id}"),
            "worktree_info": {
                "parent_project_id": "repo1",
                "main_repo_path": "/p/okena",
                "worktree_path": format!("/p/wt/{id}"),
                "branch_name": "feat/x",
            },
            "task_ref": {
                "id": { "provider": "linear", "external_id": external },
                "display_key": "QBL-1", "title": "t", "url": "http://x",
            },
        }))
    }

    fn spec_session(id: &str) -> ProjectData {
        project(serde_json::json!({
            "id": id, "name": format!("{id} (spec)"), "path": "/p/specs",
            "spec_change": "add-login",
        }))
    }

    fn repo(id: &str) -> ProjectData {
        project(serde_json::json!({ "id": id, "name": id, "path": format!("/p/{id}") }))
    }

    fn ids(v: &[(String, String)]) -> Vec<&str> {
        v.iter().map(|(id, _)| id.as_str()).collect()
    }

    #[test]
    fn a_task_session_takes_its_worktrees_with_it() {
        let projects = vec![
            repo("repo1"),
            task_session("s1", "u1"),
            worktree("wt1", "u1"),
            worktree("wt2", "u1"),
        ];
        let plan = teardown_plan(&projects, "s1").expect("a session");
        assert_eq!(ids(&plan.sessions), ["s1"]);
        assert_eq!(ids(&plan.worktrees), ["wt1", "wt2"]);
    }

    #[test]
    fn another_tasks_worktrees_are_left_alone() {
        let projects = vec![
            task_session("s1", "u1"),
            worktree("wt1", "u1"),
            worktree("other", "u9"),
        ];
        let plan = teardown_plan(&projects, "s1").expect("a session");
        assert_eq!(ids(&plan.worktrees), ["wt1"]);
    }

    #[test]
    fn a_spec_session_takes_nothing_on_disk() {
        // It runs *in* the user's spec repository. Removing a checkout here
        // would delete the repo they keep every spec in.
        let projects = vec![repo("repo1"), spec_session("s1")];
        let plan = teardown_plan(&projects, "s1").expect("a session");
        assert_eq!(ids(&plan.sessions), ["s1"]);
        assert!(plan.worktrees.is_empty(), "a spec session owns no checkout");
    }

    fn custom_session(id: &str) -> ProjectData {
        project(serde_json::json!({
            "id": id, "name": format!("{id} (agent)"), "path": "/p",
            "custom_session": "audit the unwraps",
        }))
    }

    #[test]
    fn a_custom_session_takes_nothing_on_disk() {
        // It runs in a directory the user chose — often a repo they work in.
        // Removing a checkout here would delete their project.
        let projects = vec![repo("repo1"), custom_session("s1")];
        let plan = teardown_plan(&projects, "s1").expect("a session");
        assert_eq!(ids(&plan.sessions), ["s1"]);
        assert!(
            plan.worktrees.is_empty(),
            "a custom session owns no checkout"
        );
    }

    #[test]
    fn an_ordinary_project_is_not_a_teardown_target() {
        // This route deletes checkouts, so it refuses rather than guessing.
        let projects = vec![repo("repo1")];
        assert!(teardown_plan(&projects, "repo1").is_none());
    }

    #[test]
    fn a_worktree_does_not_take_its_siblings_with_it() {
        // A worktree carries the task too. Treating it as the session would
        // let deleting one checkout delete every other checkout for that task.
        let projects = vec![
            task_session("s1", "u1"),
            worktree("wt1", "u1"),
            worktree("wt2", "u1"),
        ];
        assert!(teardown_plan(&projects, "wt1").is_none());
    }

    #[test]
    fn an_unknown_project_yields_no_plan() {
        assert!(teardown_plan(&[], "ghost").is_none());
    }
}

#[cfg(test)]
mod custom_session_tests {
    use super::{custom_brief, resolve_session_root, session_label};
    use crate::workspace::persistence::AppSettings;

    fn ctx(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(n, p)| (n.to_string(), p.to_string()))
            .collect()
    }

    fn settings_with_root(root: Option<&str>) -> AppSettings {
        let mut s = AppSettings::default();
        s.harness.agent_root = root.map(str::to_string);
        s
    }

    #[test]
    fn an_explicit_directory_wins() {
        let s = settings_with_root(Some("/configured"));
        let got = resolve_session_root("/explicit", &ctx(&[("a", "/p/a")]), &s);
        assert_eq!(got.as_deref(), Some("/explicit"));
    }

    #[test]
    fn one_project_runs_in_itself() {
        // Rooting a single-project session above the repo would make every
        // relative path in the brief wrong.
        let s = settings_with_root(Some("/configured"));
        let got = resolve_session_root("", &ctx(&[("a", "/p/a")]), &s);
        assert_eq!(got.as_deref(), Some("/p/a"));
    }

    #[test]
    fn several_projects_run_at_the_configured_root() {
        // An agent given three repos needs a directory above all of them.
        let s = settings_with_root(Some("/configured"));
        let got = resolve_session_root("", &ctx(&[("a", "/p/a"), ("b", "/p/b")]), &s);
        assert_eq!(got.as_deref(), Some("/configured"));
    }

    #[test]
    fn without_a_configured_root_several_projects_use_their_parent() {
        let s = settings_with_root(None);
        let got = resolve_session_root("", &ctx(&[("a", "/p/a"), ("b", "/p/b")]), &s);
        assert_eq!(got.as_deref(), Some("/p"));
    }

    #[test]
    fn nothing_to_go_on_yields_no_root() {
        // The caller turns this into "pick a directory" rather than guessing at
        // one and starting an agent somewhere unexpected.
        let s = settings_with_root(None);
        assert!(resolve_session_root("", &[], &s).is_none());
    }

    #[test]
    fn a_blank_configured_root_counts_as_unset() {
        let s = settings_with_root(Some("   "));
        assert!(resolve_session_root("", &[], &s).is_none());
    }

    #[test]
    fn a_name_is_used_verbatim() {
        assert_eq!(
            session_label("Refactor auth", "some long goal"),
            "Refactor auth"
        );
    }

    #[test]
    fn without_a_name_the_label_is_the_first_few_words() {
        // A session listed as a whole paragraph is unreadable in a sidebar.
        let goal = "Migrate the billing service off the legacy queue and delete the shim";
        let label = session_label("", goal);
        assert_eq!(label, "Migrate the billing service off the");
        assert!(label.len() <= 48);
    }

    #[test]
    fn an_empty_goal_still_gets_a_label() {
        assert_eq!(session_label("", ""), "agent");
    }

    #[test]
    fn the_brief_names_the_projects_it_was_given() {
        // The session may be rooted above them, where "the project" is
        // ambiguous until they are named.
        let b = custom_brief("Do the thing", &ctx(&[("okena", "/p/okena")]));
        assert!(b.starts_with("Do the thing"));
        assert!(b.contains("/p/okena"), "the path is what disambiguates");
    }

    #[test]
    fn a_brief_without_projects_is_just_the_goal() {
        assert_eq!(custom_brief("Do the thing", &[]), "Do the thing");
    }
}

#[cfg(test)]
mod kind_wire_tests {
    use super::parse_kind;
    use okena_core::tasks::TaskKind;

    #[test]
    fn every_kind_round_trips_through_its_wire_name() {
        // The UI and the MCP tools both send `wire_name`, so a kind that does
        // not survive the trip is silently created as a plain task.
        for kind in TaskKind::all() {
            assert_eq!(parse_kind(kind.wire_name()), kind, "{:?}", kind);
        }
    }

    #[test]
    fn a_providers_own_vocabulary_is_accepted() {
        // Agents and humans write "bug", not "defect".
        assert_eq!(parse_kind("bug"), TaskKind::Defect);
        assert_eq!(parse_kind("hotfix"), TaskKind::Defect);
        assert_eq!(parse_kind("initiative"), TaskKind::Epic);
        assert_eq!(parse_kind("user story"), TaskKind::Story);
    }

    #[test]
    fn matching_ignores_case_and_padding() {
        assert_eq!(parse_kind("  Epic "), TaskKind::Epic);
        assert_eq!(parse_kind("FEATURE"), TaskKind::Feature);
    }

    #[test]
    fn an_unknown_kind_becomes_a_task_rather_than_an_error() {
        // A newer client, or an agent guessing, should still get a task —
        // refusing the whole creation over a label would be the wrong trade.
        assert_eq!(parse_kind("spike"), TaskKind::Task);
        assert_eq!(parse_kind(""), TaskKind::Task);
    }
}
