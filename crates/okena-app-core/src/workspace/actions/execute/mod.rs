//! Unified action execution layer.
//!
//! Single entry point for all `ActionRequest` actions — used by both
//! the desktop UI and the remote API to eliminate code duplication
//! and ensure consistent behavior.

// All `.expect("BUG: ... must serialize")` call sites in this module
// serialize internal response DTOs to serde_json::Value. Failure is
// unreachable for well-formed types, and callers cannot recover anyway.
#![allow(clippy::expect_used)]

mod files;
mod git;
mod project;
mod session;
mod tab;
mod terminal;
mod terminal_batch;

use crate::workspace::focus::FocusManager;
use crate::workspace::hooks;
use crate::workspace::persistence::AppSettings;
use crate::workspace::state::{LayoutNode, WindowId, Workspace};
use okena_core::api::{ActionRequest, CommandResult};
use okena_terminal::TerminalsRegistry;
use okena_terminal::backend::{TerminalBackend, TerminalLaunchPlan};
use okena_terminal::shell_config::ShellType;
use okena_terminal::terminal::{Terminal, TerminalSize};
use okena_workspace::context::WorkspaceCx;
use std::collections::HashMap;
use std::sync::Arc;

pub use files::{
    PreparedContentSearch, execute_prepared_content_search,
    execute_prepared_content_search_with_cancellation, prepare_content_search,
};
pub use session::{
    apply_imported_workspace, apply_loaded_session, begin_workspace_replacement,
    cleanup_stale_workspace_replacement, ensure_workspace_replacement_allowed,
    fail_workspace_replacement, finish_workspace_replacement, import_workspace_data,
    load_session_data, load_session_data_for_shell, materialize_workspace_replacement,
    prepare_workspace_replacement,
};
pub use terminal_batch::{
    PreparedTerminalLaunch, PreparedTerminalLaunchOutcome, PublishedTerminalOwners,
    cleanup_stale_prepared_terminal_launches, materialize_prepared_terminal_launches,
    publish_prepared_terminal_launches,
};

/// Result of executing an action.
pub enum ActionResult {
    /// Success with optional JSON payload.
    Ok(Option<serde_json::Value>),
    /// Error with human-readable message.
    Err(String),
}

impl ActionResult {
    pub fn into_command_result(self) -> CommandResult {
        match self {
            ActionResult::Ok(v) => CommandResult::Ok(v),
            ActionResult::Err(e) => CommandResult::Err(e),
        }
    }
}

/// Execute any `ActionRequest` against the workspace.
///
/// This is the single source of truth for all client-facing actions.
/// Both desktop UI handlers and the remote API delegate here.
// Takes the workspace, focus manager, backend, terminals registry, settings
// and cx as distinct dependencies; bundling them into a context struct would
// obscure more than it clarifies here (matching the sub-handler modules).
#[allow(clippy::too_many_arguments)]
pub fn execute_action(
    action: ActionRequest,
    ws: &mut Workspace,
    window_id: WindowId,
    focus_manager: &mut FocusManager,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
    settings: &AppSettings,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    match action {
        // ── Terminal ops ─────────────────────────────────────────────
        ActionRequest::CreateTerminal { project_id } => terminal::create(
            ws,
            focus_manager,
            project_id,
            backend,
            terminals,
            settings,
            cx,
        ),
        ActionRequest::SplitTerminal {
            project_id,
            path,
            direction,
        } => terminal::split(
            ws,
            focus_manager,
            project_id,
            path,
            direction,
            backend,
            terminals,
            settings,
            cx,
        ),
        ActionRequest::CloseTerminal {
            project_id,
            terminal_id,
        } => terminal::close(
            ws,
            focus_manager,
            project_id,
            terminal_id,
            backend,
            terminals,
            cx,
        ),
        ActionRequest::CloseTerminals {
            project_id,
            terminal_ids,
        } => terminal::close_many(
            ws,
            focus_manager,
            project_id,
            terminal_ids,
            backend,
            terminals,
            cx,
        ),
        ActionRequest::FocusTerminal {
            project_id,
            terminal_id,
            window: _,
        } => {
            // `window` was already consumed at the bridge to resolve the target
            // `window_id` (passed in above); the per-window FocusManager handed
            // to `execute_action` is already the right one.
            terminal::focus(ws, focus_manager, project_id, terminal_id, cx)
        }
        ActionRequest::RecordProjectActivity { project_id } => {
            if ws.project(&project_id).is_none() {
                ActionResult::Err(format!("project not found: {project_id}"))
            } else {
                ws.bump_activity(&project_id, cx);
                ActionResult::Ok(None)
            }
        }
        ActionRequest::SendText { terminal_id, text } => {
            terminal::send_text(ws, terminal_id, text, backend, terminals, settings)
        }
        ActionRequest::SendBytes { terminal_id, data } => {
            terminal::send_bytes(ws, terminal_id, data, backend, terminals, settings)
        }
        ActionRequest::RunCommand {
            terminal_id,
            command,
        } => terminal::run_command(ws, terminal_id, command, backend, terminals, settings),
        ActionRequest::SendSpecialKey { terminal_id, key } => {
            terminal::send_special_key(ws, terminal_id, key, backend, terminals, settings)
        }
        ActionRequest::Resize {
            terminal_id,
            cols,
            rows,
        } => terminal::resize(ws, terminal_id, cols, rows, backend, terminals, settings),
        ActionRequest::UpdateSplitSizes {
            project_id,
            path,
            sizes,
        } => terminal::update_split_sizes(ws, project_id, path, sizes, cx),
        ActionRequest::ToggleMinimized {
            project_id,
            terminal_id,
        } => terminal::toggle_minimized(ws, project_id, terminal_id, cx),
        ActionRequest::SetFullscreen {
            project_id,
            terminal_id,
            window: _,
        } => terminal::set_fullscreen(ws, focus_manager, project_id, terminal_id, cx),
        ActionRequest::RenameTerminal {
            project_id,
            terminal_id,
            name,
        } => terminal::rename(ws, project_id, terminal_id, name, cx),
        ActionRequest::SwitchTerminalShell {
            project_id,
            terminal_id,
            shell,
        } => terminal::switch_shell(
            ws,
            project_id,
            terminal_id,
            shell,
            backend,
            terminals,
            settings,
            cx,
        ),
        ActionRequest::AddDiscoveredWorktree {
            parent_project_id,
            worktree_path,
            branch,
        } => project::add_discovered_worktree(
            ws,
            window_id,
            parent_project_id,
            worktree_path,
            branch,
            backend,
            terminals,
            settings,
            cx,
        ),
        ActionRequest::RerunHook {
            project_id,
            terminal_id,
        } => project::rerun_hook(ws, project_id, terminal_id, backend, terminals, cx),
        ActionRequest::DismissHook {
            project_id,
            terminal_id,
        } => project::dismiss_hook(ws, project_id, terminal_id, backend, terminals, cx),
        ActionRequest::ReadContent { terminal_id } => {
            terminal::read_content(ws, terminal_id, backend, terminals, settings)
        }
        ActionRequest::ExportBuffer { terminal_id } => {
            terminal::export_buffer(terminal_id, backend)
        }

        // ── Tab / pane-move ops ──────────────────────────────────────
        ActionRequest::AddTab {
            project_id,
            path,
            in_group,
        } => tab::add_tab(
            ws,
            focus_manager,
            project_id,
            path,
            in_group,
            backend,
            terminals,
            settings,
            cx,
        ),
        ActionRequest::SetActiveTab {
            project_id,
            path,
            index,
        } => tab::set_active_tab(ws, project_id, path, index, cx),
        ActionRequest::MoveTab {
            project_id,
            path,
            from_index,
            to_index,
        } => tab::move_tab(ws, project_id, path, from_index, to_index, cx),
        ActionRequest::MoveTerminalToTabGroup {
            project_id,
            terminal_id,
            target_path,
            position,
            target_project_id,
        } => tab::move_terminal_to_tab_group(
            ws,
            focus_manager,
            project_id,
            terminal_id,
            target_path,
            position,
            target_project_id,
            cx,
        ),
        ActionRequest::MovePaneTo {
            project_id,
            terminal_id,
            target_project_id,
            target_terminal_id,
            zone,
        } => tab::move_pane_to(
            ws,
            focus_manager,
            project_id,
            terminal_id,
            target_project_id,
            target_terminal_id,
            zone,
            cx,
        ),

        // ── Git ops ──────────────────────────────────────────────────
        ActionRequest::GitStatus { project_id } => git::status(ws, project_id),
        ActionRequest::GitDiffSummary { project_id } => git::diff_summary(ws, project_id),
        ActionRequest::GitDiff {
            project_id,
            mode,
            ignore_whitespace,
        } => git::diff(ws, project_id, mode, ignore_whitespace),
        ActionRequest::GitBranches { project_id } => git::branches(ws, project_id),
        ActionRequest::GitListPullRequests { project_id, limit } => {
            git::list_pull_requests(ws, project_id, limit)
        }
        ActionRequest::GitFileContents {
            project_id,
            file_path,
            mode,
        } => git::file_contents(ws, project_id, file_path, mode),
        ActionRequest::GitCommitGraph {
            project_id,
            count,
            branch,
        } => git::commit_graph(ws, project_id, count, branch),
        ActionRequest::GitListBranches { project_id } => git::list_branches(ws, project_id),
        ActionRequest::GitListWorktrees { project_id } => git::list_worktrees(ws, project_id),
        ActionRequest::WorktreeCloseInfo { project_id } => git::worktree_close_info(ws, project_id),
        ActionRequest::GenerateWorktreeBranchName { project_id } => {
            git::generate_worktree_branch_name(ws, project_id)
        }
        ActionRequest::GitListBranchesClassified { project_id } => {
            git::list_branches_classified(ws, project_id)
        }
        ActionRequest::GitCheckoutLocalBranch { project_id, branch } => {
            git::checkout_local_branch(ws, project_id, branch)
        }
        ActionRequest::GitCheckoutRemoteBranch {
            project_id,
            remote_branch,
        } => git::checkout_remote_branch(ws, project_id, remote_branch),
        ActionRequest::GitCreateAndCheckoutBranch {
            project_id,
            new_name,
            start_point,
        } => git::create_and_checkout_branch(ws, project_id, new_name, start_point),
        ActionRequest::GitStageFile {
            project_id,
            file_path,
        } => git::stage_file(ws, project_id, file_path),
        ActionRequest::GitUnstageFile {
            project_id,
            file_path,
        } => git::unstage_file(ws, project_id, file_path),
        ActionRequest::GitDiscardFile {
            project_id,
            file_path,
        } => git::discard_file(ws, project_id, file_path),
        ActionRequest::GitBlame {
            project_id,
            relative_path,
        } => git::blame(ws, project_id, relative_path),

        // ── Filesystem ops ───────────────────────────────────────────
        ActionRequest::ListFiles {
            project_id,
            show_ignored,
        } => files::list_files(ws, project_id, show_ignored),
        ActionRequest::ListDirectory {
            project_id,
            relative_path,
            show_ignored,
        } => files::list_directory(ws, project_id, relative_path, show_ignored),
        ActionRequest::ReadFile {
            project_id,
            relative_path,
        } => files::read_file(ws, project_id, relative_path),
        ActionRequest::ReadFileBytes {
            project_id,
            relative_path,
        } => files::read_file_bytes(ws, project_id, relative_path),
        ActionRequest::FileSize {
            project_id,
            relative_path,
        } => files::file_size(ws, project_id, relative_path),
        ActionRequest::SearchContent {
            project_id,
            query,
            case_sensitive,
            mode,
            max_results,
            file_glob,
            context_lines,
            show_ignored,
        } => files::search_content(
            ws,
            project_id,
            query,
            case_sensitive,
            mode,
            max_results,
            file_glob,
            context_lines,
            show_ignored,
        ),
        ActionRequest::RenameFile {
            project_id,
            relative_path,
            new_name,
        } => files::rename_file(ws, project_id, relative_path, new_name),
        ActionRequest::DeleteFile {
            project_id,
            relative_path,
        } => files::delete_file(ws, project_id, relative_path),
        ActionRequest::CreateFile {
            project_id,
            relative_path,
        } => files::create_file(ws, project_id, relative_path),
        ActionRequest::CreateDirectory {
            project_id,
            relative_path,
        } => files::create_directory(ws, project_id, relative_path),

        // ── Project / folder / worktree ops ──────────────────────────
        ActionRequest::AddProject { name, path } => {
            project::add_project(ws, window_id, name, path, backend, terminals, settings, cx)
        }
        ActionRequest::ReorderProjectInFolder {
            folder_id,
            project_id,
            new_index,
        } => project::reorder_in_folder(ws, folder_id, project_id, new_index, cx),
        ActionRequest::SetProjectColor { project_id, color } => {
            project::set_project_color(ws, project_id, color, cx)
        }
        ActionRequest::SetFolderColor { folder_id, color } => {
            project::set_folder_color(ws, folder_id, color, cx)
        }
        ActionRequest::RenameProject { project_id, name } => {
            project::rename_project(ws, project_id, name, cx)
        }
        ActionRequest::UpdateProjectHooks { project_id, hooks } => {
            project::update_project_hooks(ws, project_id, *hooks, cx)
        }
        ActionRequest::RenameProjectDirectory {
            project_id,
            new_name,
        } => project::rename_project_directory(ws, project_id, new_name, cx),
        ActionRequest::DeleteProject { project_id } => {
            project::delete_project(ws, focus_manager, project_id, settings, cx)
        }
        ActionRequest::SetProjectShowInOverview {
            project_id,
            show,
            window: _,
        } => project::set_show_in_overview(ws, focus_manager, window_id, project_id, show, cx),
        ActionRequest::RemoveWorktreeProject { project_id, force } => {
            project::remove_worktree_project(ws, focus_manager, project_id, force, settings, cx)
        }
        ActionRequest::CloseWorktree {
            project_id,
            merge,
            stash,
            fetch,
            push,
            delete_branch,
        } => project::close_worktree(
            ws,
            focus_manager,
            project_id,
            merge,
            stash,
            fetch,
            push,
            delete_branch,
            settings,
            cx,
        ),
        ActionRequest::CreateFolder { name } => project::create_folder(ws, name, cx),
        ActionRequest::DeleteFolder { folder_id } => project::delete_folder(ws, folder_id, cx),
        ActionRequest::RenameFolder { folder_id, name } => {
            project::rename_folder(ws, folder_id, name, cx)
        }
        ActionRequest::MoveProjectToFolder {
            project_id,
            folder_id,
            position,
        } => project::move_to_folder(ws, project_id, folder_id, position, cx),
        ActionRequest::MoveProjectOutOfFolder {
            project_id,
            top_level_index,
        } => project::move_out_of_folder(ws, project_id, top_level_index, cx),
        ActionRequest::MoveProject {
            project_id,
            new_index,
        } => project::move_project(ws, project_id, new_index, cx),
        ActionRequest::MoveItemInOrder { item_id, new_index } => {
            project::move_item_in_order(ws, item_id, new_index, cx)
        }
        ActionRequest::ToggleProjectPinned { project_id } => {
            project::toggle_project_pinned(ws, project_id, cx)
        }
        ActionRequest::ReorderWorktree {
            parent_id,
            worktree_id,
            new_index,
        } => project::reorder_worktree(ws, parent_id, worktree_id, new_index, cx),
        ActionRequest::SetWorktreeColorOverride { project_id, color } => {
            project::set_worktree_color_override(ws, project_id, color, cx)
        }
        ActionRequest::CreateWorktree {
            project_id,
            branch,
            create_branch,
        } => project::create_worktree(
            ws,
            window_id,
            project_id,
            branch,
            create_branch,
            backend,
            terminals,
            settings,
            cx,
        ),

        // ── Sessions (whole-workspace; daemon owns session files + state) ──
        ActionRequest::ListSessions => session::list_sessions_action(),
        ActionRequest::LoadSession { name } => {
            session::load_session_action(ws, focus_manager, name, backend, terminals, settings, cx)
        }
        ActionRequest::SaveSession { name } => session::save_session_action(ws, name),
        ActionRequest::RenameSession { old_name, new_name } => {
            session::rename_session_action(old_name, new_name)
        }
        ActionRequest::DeleteSession { name } => session::delete_session_action(name),
        ActionRequest::ImportWorkspace { path } => session::import_workspace_action(
            ws,
            focus_manager,
            path,
            backend,
            terminals,
            settings,
            cx,
        ),
        ActionRequest::ExportWorkspace { path } => session::export_workspace_action(ws, path),

        // Soft-close undo / finalize are handled by the daemon command loop
        // directly (it owns the grace deadlines + kept-alive PTYs).
        ActionRequest::UndoSoftClose { .. } | ActionRequest::CloseTerminalNow { .. } => {
            ActionResult::Err(
                "soft-close undo/finalize must be handled by the daemon command loop".to_string(),
            )
        }

        // Service actions are handled by the remote command loop directly
        ActionRequest::StartService { .. }
        | ActionRequest::StopService { .. }
        | ActionRequest::RestartService { .. }
        | ActionRequest::StartAllServices { .. }
        | ActionRequest::StopAllServices { .. }
        | ActionRequest::ReloadServices { .. } => {
            ActionResult::Err("service actions must be handled via ServiceManager".to_string())
        }

        // App-scoped actions (settings, theme, command palette) are handled by
        // the remote command loop directly — they touch globals/windows outside
        // the Workspace, so they never reach this Workspace-scoped executor.
        ActionRequest::GetSettings
        | ActionRequest::GetSettingsSchema
        | ActionRequest::SetSettings { .. }
        | ActionRequest::GetThemes
        | ActionRequest::GetTheme { .. }
        | ActionRequest::SetTheme { .. }
        | ActionRequest::SaveCustomTheme { .. }
        | ActionRequest::ListActions
        | ActionRequest::InvokeAction { .. } => {
            ActionResult::Err("app-scoped action must be handled by the remote bridge".to_string())
        }
    }
}

/// Look up a terminal in the registry. If not found, attempt to spawn it
/// by finding the terminal_id in the workspace layout and creating a PTY.
pub fn ensure_terminal(
    terminal_id: &str,
    terminals: &TerminalsRegistry,
    backend: &dyn TerminalBackend,
    ws: &Workspace,
    settings: &AppSettings,
) -> Option<Arc<Terminal>> {
    // Fast path: already in registry
    if let Some(term) = terminals.lock().get(terminal_id).cloned() {
        return Some(term);
    }

    // Find which project owns this terminal_id and preserve its configured shell.
    // This is required for WSL reconnects: passing `None` routes the persistent
    // attach through the Windows host backend instead of the WSL distro backend.
    let mut reconnect = None;
    for project in &ws.data().projects {
        if let Some(layout) = &project.layout
            && let Some(path) = layout.find_terminal_path(terminal_id)
            && let Some(LayoutNode::Terminal { shell_type, .. }) = layout.get_at_path(&path)
        {
            let shell = shell_type
                .clone()
                .resolve_default(project.default_shell.as_ref(), &settings.default_shell);
            reconnect = Some((project.path.clone(), TerminalLaunchPlan::for_shell(shell)));
            break;
        }
    }
    let (cwd, plan) = reconnect?;

    // Spawn PTY via backend
    match backend.reconnect_terminal_with_plan(terminal_id, &cwd, &plan) {
        Ok(_id) => {
            let terminal = Arc::new(Terminal::new(
                terminal_id.to_string(),
                TerminalSize::default(),
                backend.transport(),
                cwd,
            ));
            terminals
                .lock()
                .insert(terminal_id.to_string(), terminal.clone());
            log::info!("Auto-spawned terminal {} for remote client", terminal_id);
            Some(terminal)
        }
        Err(e) => {
            log::error!("Failed to auto-spawn terminal {}: {}", terminal_id, e);
            None
        }
    }
}

fn effective_terminal_launch(
    shell_type: ShellType,
    project_default_shell: Option<&ShellType>,
    global_default_shell: &ShellType,
    shell_wrapper: Option<&str>,
    on_create: Option<&str>,
    // Command to run after `on_create`, currently only an agent-session resume.
    // Composed into the same shell line so the pane still hands off to an
    // interactive shell once it exits.
    startup_command: Option<&str>,
    env: &HashMap<String, String>,
) -> TerminalLaunchPlan {
    let shell = shell_type.resolve_default(project_default_shell, global_default_shell);
    let composed = match (on_create, startup_command) {
        (Some(hook), Some(command)) => Some(format!(
            "{hook}{}{command}",
            hooks::startup_command_separator(&shell)
        )),
        (hook, command) => hook.or(command).map(str::to_string),
    };
    hooks::terminal_launch_plan(shell, shell_wrapper, composed.as_deref(), env)
}

/// Reserve IDs and launch plans for runtime-recovery slots without touching the backend.
pub fn reserve_uninitialized_terminal_launches(
    ws: &mut Workspace,
    project_ids: &[String],
    settings: &AppSettings,
    cx: &mut impl WorkspaceCx,
) -> Result<Vec<PreparedTerminalLaunch>, String> {
    let mut launches = Vec::new();
    for project_id in project_ids {
        let project = ws
            .project(project_id)
            .ok_or_else(|| format!("project not found: {project_id}"))?;
        if project.is_remote {
            return Err(format!(
                "remote project terminals cannot be materialized locally: {project_id}"
            ));
        }
        let project_path = project.path.clone();
        let project_name = project.name.clone();
        let project_hooks = project.hooks.clone();
        let is_worktree = project.worktree_info.is_some();
        let parent_hooks = project
            .worktree_info
            .as_ref()
            .and_then(|worktree| ws.project(&worktree.parent_project_id))
            .map(|parent| parent.hooks.clone());
        let project_default_shell = project.default_shell.clone();
        let mut uninitialized = Vec::new();
        if let Some(layout) = &project.layout {
            collect_uninitialized_terminals_with_shell(layout, Vec::new(), &mut uninitialized);
        }
        let shell_wrapper =
            hooks::resolve_shell_wrapper(&project_hooks, parent_hooks.as_ref(), &settings.hooks);
        let on_create = hooks::resolve_terminal_on_create_simple(
            &project_hooks,
            parent_hooks.as_ref(),
            &settings.hooks,
        );
        let folder = ws.folder_for_project_or_parent(project_id);
        let env = hooks::terminal_hook_env(
            project_id,
            &project_name,
            &project_path,
            is_worktree,
            folder.map(|folder| folder.id.as_str()),
            folder.map(|folder| folder.name.as_str()),
        );
        for (path, shell_type) in uninitialized {
            launches.push(PreparedTerminalLaunch::new(
                project_id.clone(),
                path,
                uuid::Uuid::new_v4().to_string(),
                project_path.clone(),
                effective_terminal_launch(
                    shell_type,
                    project_default_shell.as_ref(),
                    &settings.default_shell,
                    shell_wrapper.as_deref(),
                    on_create.as_deref(),
                    // Recovery re-launches panes that already hold live ids, so
                    // no restore-time resume is queued for them.
                    None,
                    &env,
                ),
            ));
        }
    }

    for launch in &launches {
        ws.set_terminal_id(
            launch.project_id(),
            launch.layout_path(),
            launch.terminal_id().to_string(),
            cx,
        );
    }
    Ok(launches)
}

/// Clear failed reservations without disturbing a terminal that replaced their ID.
pub fn clear_failed_terminal_launch_reservations(
    ws: &mut Workspace,
    launches: &[PreparedTerminalLaunch],
    failed_terminal_ids: &[String],
    cx: &mut impl WorkspaceCx,
) {
    let failed: std::collections::HashSet<&str> =
        failed_terminal_ids.iter().map(String::as_str).collect();
    for launch in launches {
        if !failed.contains(launch.terminal_id()) {
            continue;
        }
        ws.with_layout_node(
            launch.project_id(),
            launch.layout_path(),
            cx,
            |node| match node {
                LayoutNode::Terminal { terminal_id, .. }
                    if terminal_id.as_deref() == Some(launch.terminal_id()) =>
                {
                    *terminal_id = None;
                    true
                }
                _ => false,
            },
        );
    }
}

/// Spawn PTYs for any uninitialized terminals (`terminal_id: None`) in a project's layout.
///
/// Used after `CreateTerminal` / `SplitTerminal` to eagerly create PTYs for
/// remote clients that don't have a rendering layer to trigger lazy spawning.
/// The cwd a *new* terminal should inherit when it's created next to an
/// existing one (split / add-tab): the live working directory of the terminal
/// the user acted on — the node at `path`, or the visible terminal under it
/// when `path` is a group. Resolved from the action's `path` (client-independent,
/// so it holds in the daemon model). Uses `Terminal::current_cwd` (the OSC 7
/// shell cwd), so it follows wherever the source shell has `cd`-ed. `None` when
/// there's no live source terminal — callers then fall back to the project path.
pub(super) fn inherited_cwd(
    ws: &Workspace,
    terminals: &TerminalsRegistry,
    project_id: &str,
    path: &[usize],
) -> Option<String> {
    let layout = ws.project(project_id)?.layout.as_ref()?;
    let node = layout.get_at_path(path)?;
    let rel = node.find_visible_terminal_path();
    let LayoutNode::Terminal {
        terminal_id: Some(terminal_id),
        ..
    } = node.get_at_path(&rel)?
    else {
        return None;
    };
    let cwd = terminals.lock().get(terminal_id)?.current_cwd();
    (!cwd.is_empty()).then_some(cwd)
}

pub fn spawn_uninitialized_terminals(
    ws: &mut Workspace,
    project_id: &str,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
    settings: &AppSettings,
    // When a new terminal is created next to an existing one (split / add-tab),
    // the caller passes the source terminal's live cwd so the new one opens
    // "here". `None` → the project path (fresh projects, worktrees, sessions).
    inherit_cwd: Option<String>,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    // Don't spawn terminals for projects whose worktree is still being created
    if ws.is_creating_project(project_id) {
        return ActionResult::Ok(None);
    }

    let project = match ws.project(project_id) {
        Some(p) => p,
        None => return ActionResult::Err(format!("project not found: {}", project_id)),
    };

    let project_path = project.path.clone();
    // The directory new PTYs actually spawn in: the inherited (source-terminal)
    // cwd when provided, else the project path.
    let spawn_cwd = inherit_cwd.unwrap_or_else(|| project_path.clone());
    let project_name = project.name.clone();
    let project_hooks = project.hooks.clone();
    let is_worktree = project.worktree_info.is_some();
    let parent_hooks = project
        .worktree_info
        .as_ref()
        .and_then(|wt| ws.project(&wt.parent_project_id))
        .map(|p| p.hooks.clone());
    let project_default_shell = project.default_shell.clone();
    let mut uninitialized = Vec::new();
    if let Some(layout) = &project.layout {
        collect_uninitialized_terminals_with_shell(layout, vec![], &mut uninitialized);
    }
    log::info!(
        "spawn_uninitialized_terminals: project={}, uninitialized_count={}",
        project_id,
        uninitialized.len()
    );

    let global_default = settings.default_shell.clone();
    let global_hooks = settings.hooks.clone();
    let auto_resume = settings.auto_resume_agent_sessions;

    // Resolve shell_wrapper and on_create once for all terminals in this project
    let shell_wrapper =
        hooks::resolve_shell_wrapper(&project_hooks, parent_hooks.as_ref(), &global_hooks);
    let on_create_cmd = hooks::resolve_terminal_on_create_simple(
        &project_hooks,
        parent_hooks.as_ref(),
        &global_hooks,
    );
    let folder = ws.folder_for_project_or_parent(project_id);
    let folder_id = folder.map(|f| f.id.as_str());
    let folder_name = folder.map(|f| f.name.as_str());
    let env = hooks::terminal_hook_env(
        project_id,
        &project_name,
        &project_path,
        is_worktree,
        folder_id,
        folder_name,
    );

    let mut spawned_ids = Vec::new();
    for (path, shell_type) in uninitialized {
        // Claim the agent session this pane carried before the restart, if any.
        // Taking it makes the resume exactly-once; the session is then re-keyed
        // onto the new terminal id below so the pane keeps its identity whether
        // or not we actually resumed.
        let pending_session = ws.take_pending_agent_resume(project_id, &path);
        let resume_command = pending_session.as_ref().filter(|_| auto_resume).and_then(
            |session| {
                okena_core::agent_harness::resume_command_line(
                    session,
                    std::path::Path::new(&spawn_cwd),
                )
            },
        );
        if let Some(ref command) = resume_command {
            log::info!("agent-resume: project={project_id} path={path:?} command={command}");
        }
        let plan = effective_terminal_launch(
            shell_type,
            project_default_shell.as_ref(),
            &global_default,
            shell_wrapper.as_deref(),
            on_create_cmd.as_deref(),
            resume_command.as_deref(),
            &env,
        );

        match backend.create_terminal_with_plan(&spawn_cwd, &plan) {
            Ok(terminal_id) => {
                ws.set_terminal_id(project_id, &path, terminal_id.clone(), cx);
                if let Some(session) = pending_session {
                    ws.set_agent_session(project_id, &terminal_id, session, cx);
                }
                let terminal = Arc::new(Terminal::new(
                    terminal_id.clone(),
                    TerminalSize::default(),
                    backend.transport(),
                    spawn_cwd.clone(),
                ));

                terminals.lock().insert(terminal_id.clone(), terminal);
                spawned_ids.push(terminal_id);
            }
            Err(e) => {
                log::error!("Failed to spawn terminal for project {}: {}", project_id, e);
                return ActionResult::Err(format!("failed to spawn terminal: {}", e));
            }
        }
    }

    // Always return terminal_ids — even when empty — so callers know the action completed
    ActionResult::Ok(Some(serde_json::json!({ "terminal_ids": spawned_ids })))
}

/// Find the first terminal_id in a layout tree (depth-first).
fn find_first_terminal_id(node: &LayoutNode) -> Option<String> {
    match node {
        LayoutNode::Terminal { terminal_id, .. } => terminal_id.clone(),
        LayoutNode::Split { children, .. } | LayoutNode::Tabs { children, .. } => {
            children.iter().find_map(find_first_terminal_id)
        }
    }
}

/// Find the layout path for a terminal within a project.
pub fn find_terminal_path(
    ws: &Workspace,
    project_id: &str,
    terminal_id: &str,
) -> Option<Vec<usize>> {
    ws.project(project_id)?
        .layout
        .as_ref()?
        .find_terminal_path(terminal_id)
}

/// Canonicalize a relative path within a project directory and verify it doesn't
/// escape the project root (path traversal protection).
fn resolve_project_file(
    project_path: &str,
    relative_path: &str,
) -> Result<std::path::PathBuf, String> {
    let full_path = std::path::Path::new(project_path).join(relative_path);
    let canonical = full_path
        .canonicalize()
        .map_err(|e| format!("Cannot read file: {}", e))?;
    let project_root = std::path::Path::new(project_path)
        .canonicalize()
        .map_err(|e| format!("Cannot resolve project path: {}", e))?;
    if !canonical.starts_with(&project_root) {
        return Err("path traversal not allowed".to_string());
    }
    Ok(canonical)
}

/// Resolve a new (possibly non-existent) target path inside a project. The parent
/// must exist and canonicalize inside the project root. The leaf filename is then
/// joined back on — so the target itself does not need to exist yet.
fn resolve_new_project_file(
    project_path: &str,
    relative_path: &str,
) -> Result<std::path::PathBuf, String> {
    if relative_path.is_empty() {
        return Err("relative_path must not be empty".to_string());
    }
    let full_path = std::path::Path::new(project_path).join(relative_path);
    let parent = full_path
        .parent()
        .ok_or_else(|| "relative_path has no parent".to_string())?;
    let file_name = full_path
        .file_name()
        .ok_or_else(|| "relative_path has no file name".to_string())?;
    let parent_canonical = parent
        .canonicalize()
        .map_err(|e| format!("Cannot resolve parent directory: {}", e))?;
    let project_root = std::path::Path::new(project_path)
        .canonicalize()
        .map_err(|e| format!("Cannot resolve project path: {}", e))?;
    if !parent_canonical.starts_with(&project_root) {
        return Err("path traversal not allowed".to_string());
    }
    Ok(parent_canonical.join(file_name))
}

/// Reject names that would escape a directory or traverse paths.
fn validate_leaf_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("name must not be empty".to_string());
    }
    if name.contains('/') || name.contains('\\') || name == "." || name == ".." {
        return Err("name must not contain path separators".to_string());
    }
    Ok(())
}

/// Recursively collect paths to all Terminal nodes with `terminal_id: None`.
/// Collect uninitialized terminals in a layout tree, returning their paths and shell types.
fn collect_uninitialized_terminals_with_shell(
    node: &LayoutNode,
    current_path: Vec<usize>,
    result: &mut Vec<(Vec<usize>, ShellType)>,
) {
    match node {
        LayoutNode::Terminal {
            terminal_id: None,
            shell_type,
            ..
        } => {
            result.push((current_path, shell_type.clone()));
        }
        LayoutNode::Terminal { .. } => {}
        LayoutNode::Split { children, .. } | LayoutNode::Tabs { children, .. } => {
            for (i, child) in children.iter().enumerate() {
                let mut child_path = current_path.clone();
                child_path.push(i);
                collect_uninitialized_terminals_with_shell(child, child_path, result);
            }
        }
    }
}

#[cfg(test)]
mod path_guard_tests {
    use super::{resolve_new_project_file, resolve_project_file, validate_leaf_name};
    use std::fs;

    fn mktmp() -> std::path::PathBuf {
        let base = std::env::temp_dir().join(format!(
            "okena-exec-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&base).unwrap();
        base
    }

    #[test]
    fn resolve_project_file_rejects_traversal() {
        let root = mktmp();
        let outside = root.parent().unwrap().join("outside.txt");
        fs::write(&outside, "x").unwrap();
        let root_str = root.to_str().unwrap();
        let rel = format!("../{}", outside.file_name().unwrap().to_string_lossy());
        let err = resolve_project_file(root_str, &rel).unwrap_err();
        assert!(err.contains("path traversal"), "got: {}", err);
        fs::remove_file(&outside).ok();
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn resolve_project_file_ok_inside() {
        let root = mktmp();
        let inner = root.join("a.txt");
        fs::write(&inner, "x").unwrap();
        let out = resolve_project_file(root.to_str().unwrap(), "a.txt").unwrap();
        assert!(out.ends_with("a.txt"));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn resolve_new_project_file_parent_must_exist_inside_root() {
        let root = mktmp();
        // Parent exists (root), leaf doesn't.
        let out = resolve_new_project_file(root.to_str().unwrap(), "new.txt").unwrap();
        assert_eq!(out, root.canonicalize().unwrap().join("new.txt"));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn resolve_new_project_file_rejects_parent_traversal() {
        let root = mktmp();
        let err = resolve_new_project_file(root.to_str().unwrap(), "../evil.txt").unwrap_err();
        assert!(err.contains("path traversal"), "got: {}", err);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn resolve_new_project_file_rejects_missing_parent() {
        let root = mktmp();
        let err = resolve_new_project_file(root.to_str().unwrap(), "nope/new.txt").unwrap_err();
        assert!(err.contains("parent"), "got: {}", err);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn validate_leaf_name_rules() {
        assert!(validate_leaf_name("ok.txt").is_ok());
        assert!(validate_leaf_name("").is_err());
        assert!(validate_leaf_name(".").is_err());
        assert!(validate_leaf_name("..").is_err());
        assert!(validate_leaf_name("a/b").is_err());
        assert!(validate_leaf_name("a\\b").is_err());
    }
}

#[cfg(test)]
mod reconnect_shell_tests {
    #[cfg(windows)]
    use super::spawn_uninitialized_terminals;
    use super::{
        AppSettings, clear_failed_terminal_launch_reservations, ensure_terminal,
        reserve_uninitialized_terminal_launches,
    };
    use crate::workspace::settings::HooksConfig;
    use crate::workspace::state::{LayoutNode, ProjectData, WindowState, Workspace, WorkspaceData};
    use okena_terminal::TerminalsRegistry;
    use okena_terminal::backend::{TerminalBackend, TerminalLaunchPlan};
    use okena_terminal::shell_config::ShellType;
    use okena_terminal::terminal::TerminalTransport;
    use okena_workspace::context::WorkspaceCx;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    struct StubTransport;

    impl TerminalTransport for StubTransport {
        fn send_input(&self, _terminal_id: &str, _data: &[u8]) {}
        fn resize(&self, _terminal_id: &str, _cols: u16, _rows: u16) {}
        fn uses_mouse_backend(&self) -> bool {
            false
        }
    }

    #[derive(Default)]
    pub(super) struct RecordingBackend {
        created_shells: Mutex<Vec<Option<ShellType>>>,
        reconnected_shells: Mutex<Vec<Option<ShellType>>>,
        pub(super) plans: Mutex<Vec<TerminalLaunchPlan>>,
    }

    impl TerminalBackend for RecordingBackend {
        fn transport(&self) -> Arc<dyn TerminalTransport> {
            Arc::new(StubTransport)
        }

        fn create_terminal(&self, _cwd: &str, shell: Option<&ShellType>) -> anyhow::Result<String> {
            self.created_shells
                .lock()
                .expect("created shell lock")
                .push(shell.cloned());
            Ok("terminal".to_string())
        }

        fn create_terminal_with_plan(
            &self,
            _cwd: &str,
            plan: &TerminalLaunchPlan,
        ) -> anyhow::Result<String> {
            self.plans.lock().expect("plan lock").push(plan.clone());
            let shell = plan.initial_command.as_ref().map_or_else(
                || plan.route.clone(),
                |command| ShellType::Custom {
                    path: command.program.clone(),
                    args: command.args.clone(),
                },
            );
            self.created_shells
                .lock()
                .expect("created shell lock")
                .push(Some(shell));
            Ok("terminal".to_string())
        }

        fn reconnect_terminal(
            &self,
            terminal_id: &str,
            _cwd: &str,
            shell: Option<&ShellType>,
        ) -> anyhow::Result<String> {
            self.reconnected_shells
                .lock()
                .expect("reconnected shell lock")
                .push(shell.cloned());
            Ok(terminal_id.to_string())
        }

        fn reconnect_terminal_with_plan(
            &self,
            terminal_id: &str,
            _cwd: &str,
            plan: &TerminalLaunchPlan,
        ) -> anyhow::Result<String> {
            self.plans.lock().expect("plan lock").push(plan.clone());
            self.reconnected_shells
                .lock()
                .expect("reconnected shell lock")
                .push(Some(plan.route.clone()));
            Ok(terminal_id.to_string())
        }

        fn kill(&self, _terminal_id: &str) {}
        fn capture_buffer(&self, _terminal_id: &str) -> Option<std::path::PathBuf> {
            None
        }
        fn supports_buffer_capture(&self) -> bool {
            false
        }
        fn is_remote(&self) -> bool {
            false
        }
        fn get_shell_pid(&self, _terminal_id: &str) -> Option<u32> {
            None
        }
        fn get_service_pids(&self, _terminal_id: &str) -> Vec<u32> {
            Vec::new()
        }
    }

    pub(super) struct TestCx;

    impl WorkspaceCx for TestCx {
        fn notify(&mut self) {}
        fn refresh_views(&mut self) {}
        fn hook_runner(&self) -> Option<crate::workspace::hooks::HookRunner> {
            None
        }
        fn hook_monitor(&self) -> Option<crate::workspace::hook_monitor::HookMonitor> {
            None
        }
    }

    pub(super) fn workspace_with_terminal(
        shell_type: ShellType,
        default_shell: Option<ShellType>,
        terminal_id: Option<&str>,
        // Sessions queued for the root pane, as `validate_workspace_data` would
        // leave them after a restore that cleared terminal ids.
        pending_agent_resumes: HashMap<Vec<usize>, okena_core::agent_session::AgentSession>,
    ) -> Workspace {
        let project = ProjectData {
            id: "project".into(),
            name: "Project".into(),
            path: "/project".into(),
            layout: Some(LayoutNode::Terminal {
                terminal_id: terminal_id.map(str::to_string),
                shell_type,
                minimized: false,
                detached: false,
                zoom_level: 1.0,
            }),
            terminal_names: HashMap::new(),
            hidden_terminals: HashMap::new(),
            worktree_info: None,
            worktree_ids: Vec::new(),
            folder_color: Default::default(),
            hooks: HooksConfig::default(),
            is_remote: false,
            connection_id: None,
            service_terminals: HashMap::new(),
            agent_sessions: HashMap::new(),
            pending_agent_resumes,
            default_shell,
            hook_terminals: HashMap::new(),
            pinned: false,
            last_activity_at: None,
            is_creating: false,
            is_closing: false,
        };
        Workspace::new(WorkspaceData {
            version: 1,
            projects: vec![project],
            project_order: vec!["project".into()],
            folders: Vec::new(),
            service_panel_heights: HashMap::new(),
            hook_panel_heights: HashMap::new(),
            main_window: WindowState::default(),
            extra_windows: Vec::new(),
        })
    }

    fn workspace(shell_type: ShellType, default_shell: Option<ShellType>) -> Workspace {
        workspace_with_terminal(shell_type, default_shell, Some("terminal"), HashMap::new())
    }

    #[test]
    fn reconnect_passes_explicit_terminal_shell() {
        let explicit = ShellType::Custom {
            path: "/bin/explicit".into(),
            args: vec!["--login".into()],
        };
        let ws = workspace(explicit.clone(), None);
        let terminals: TerminalsRegistry = Arc::new(Default::default());
        let backend = RecordingBackend::default();
        let settings = AppSettings::default();

        assert!(ensure_terminal("terminal", &terminals, &backend, &ws, &settings).is_some());
        assert_eq!(
            backend
                .reconnected_shells
                .lock()
                .expect("shell lock")
                .as_slice(),
            &[Some(explicit)]
        );
    }

    #[test]
    fn reconnect_resolves_terminal_default_from_project() {
        let project_default = ShellType::Custom {
            path: "/bin/project-default".into(),
            args: Vec::new(),
        };
        let ws = workspace(ShellType::Default, Some(project_default.clone()));
        let terminals: TerminalsRegistry = Arc::new(Default::default());
        let backend = RecordingBackend::default();
        let settings = AppSettings::default();

        assert!(ensure_terminal("terminal", &terminals, &backend, &ws, &settings).is_some());
        assert_eq!(
            backend
                .reconnected_shells
                .lock()
                .expect("shell lock")
                .as_slice(),
            &[Some(project_default)]
        );
    }

    #[test]
    fn reconnect_passes_system_default_instead_of_none() {
        let ws = workspace(ShellType::Default, None);
        let terminals: TerminalsRegistry = Arc::new(Default::default());
        let backend = RecordingBackend::default();
        let settings = AppSettings::default();

        assert!(ensure_terminal("terminal", &terminals, &backend, &ws, &settings).is_some());
        assert_eq!(
            backend
                .reconnected_shells
                .lock()
                .expect("shell lock")
                .as_slice(),
            &[Some(ShellType::Default)]
        );
    }

    #[test]
    fn failed_reservation_does_not_clear_a_replacement_terminal_id() {
        let mut ws = workspace_with_terminal(ShellType::Default, None, None, HashMap::new());
        let mut cx = TestCx;
        let launches = reserve_uninitialized_terminal_launches(
            &mut ws,
            &["project".to_string()],
            &AppSettings::default(),
            &mut cx,
        )
        .expect("reserve terminal launch");
        assert_eq!(launches.len(), 1);
        let reserved_id = launches[0].terminal_id().to_string();

        ws.set_terminal_id("project", &[], "replacement".to_string(), &mut cx);
        clear_failed_terminal_launch_reservations(&mut ws, &launches, &[reserved_id], &mut cx);

        let LayoutNode::Terminal { terminal_id, .. } = ws
            .project("project")
            .and_then(|project| project.layout.as_ref())
            .expect("project terminal layout")
        else {
            panic!("expected terminal layout");
        };
        assert_eq!(terminal_id.as_deref(), Some("replacement"));
    }

    #[cfg(windows)]
    #[test]
    fn reconnect_keeps_base_wsl_route_and_does_not_replay_create_hook() {
        let wsl = ShellType::Wsl {
            distro: Some("Ubuntu".into()),
        };
        let mut create_ws = workspace_with_terminal(ShellType::Default, None, None, HashMap::new());
        let create_terminals: TerminalsRegistry = Arc::new(Default::default());
        let backend = RecordingBackend::default();
        let mut settings = AppSettings::default();
        settings.default_shell = wsl.clone();
        settings.hooks.terminal.on_create = Some("echo ready".to_string());
        let mut cx = TestCx;

        let _ = spawn_uninitialized_terminals(
            &mut create_ws,
            "project",
            &backend,
            &create_terminals,
            &settings,
            None,
            &mut cx,
        );

        let reconnect_ws = workspace(ShellType::Default, None);
        let reconnect_terminals: TerminalsRegistry = Arc::new(Default::default());
        assert!(
            ensure_terminal(
                "terminal",
                &reconnect_terminals,
                &backend,
                &reconnect_ws,
                &settings,
            )
            .is_some()
        );

        let plans = backend.plans.lock().expect("plan lock");
        assert_eq!(plans.len(), 2);
        assert_eq!(plans[0].route, wsl);
        assert!(plans[0].initial_command.is_some());
        assert_eq!(plans[1].route, wsl);
        assert!(plans[1].initial_command.is_none());
    }
}

/// Restore-time agent-session resume.
///
/// The daemon owns this: `validate_workspace_data` re-keys a surviving session
/// onto its pane's layout path, and `spawn_uninitialized_terminals` consumes it
/// as the pane's startup command while assigning the new terminal id.
#[cfg(test)]
mod agent_resume_tests {
    use super::reconnect_shell_tests::{RecordingBackend, TestCx, workspace_with_terminal};
    use super::{AppSettings, spawn_uninitialized_terminals};
    use okena_core::agent_harness::{AgentHarness, AgentHarnessRegistry};
    use okena_core::agent_session::AgentSession;
    use okena_terminal::TerminalsRegistry;
    use okena_terminal::shell_config::ShellType;
    use std::collections::HashMap;
    use std::path::Path;
    use std::sync::Arc;

    const UUID: &str = "3b9c1f2a-4d5e-6f70-8a9b-0c1d2e3f4a5b";

    struct StubHarness;

    impl AgentHarness for StubHarness {
        fn id(&self) -> &str {
            "test-agent"
        }
        fn resume_command(&self, session_id: &str, _cwd: &Path) -> Option<Vec<String>> {
            Some(vec![
                "test-agent".to_string(),
                "--resume".to_string(),
                session_id.to_string(),
            ])
        }
    }

    /// The harness registry is a process-wide `OnceLock` (first write wins), so
    /// every test in this binary shares this one stub.
    fn stub_registry() {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            let mut registry = AgentHarnessRegistry::new();
            registry.register(Arc::new(StubHarness));
            okena_core::agent_harness::init(registry);
        });
    }

    fn session(agent: &str) -> AgentSession {
        AgentSession {
            agent: agent.to_string(),
            session_id: UUID.to_string(),
            transcript_path: None,
        }
    }

    fn settings(auto_resume: bool) -> AppSettings {
        let mut settings = AppSettings::default();
        settings.auto_resume_agent_sessions = auto_resume;
        settings
    }

    /// Spawn the single root pane and return (startup command args, workspace).
    fn spawn(
        pending: Option<AgentSession>,
        auto_resume: bool,
    ) -> (Option<Vec<String>>, crate::workspace::state::Workspace) {
        stub_registry();
        let pending = pending
            .map(|s| HashMap::from([(Vec::new(), s)]))
            .unwrap_or_default();
        let mut ws = workspace_with_terminal(ShellType::Default, None, None, pending);
        let terminals: TerminalsRegistry = Arc::new(Default::default());
        let backend = RecordingBackend::default();

        spawn_uninitialized_terminals(
            &mut ws,
            "project",
            &backend,
            &terminals,
            &settings(auto_resume),
            None,
            &mut TestCx,
        );

        let plans = backend.plans.lock().expect("plan lock");
        let args = plans
            .first()
            .and_then(|p| p.initial_command.as_ref())
            .map(|c| c.args.clone());
        drop(plans);
        (args, ws)
    }

    #[test]
    fn restored_pane_resumes_its_session_as_a_startup_command() {
        let (args, _ws) = spawn(Some(session("test-agent")), true);

        let args = args.expect("resume must reach the launch plan, not the attached PTY");
        assert!(
            args.iter()
                .any(|a| a.contains(&format!("test-agent --resume {UUID}"))),
            "resume command missing from {args:?}"
        );
        // The pane keeps a shell after the agent exits.
        assert!(
            args.iter().any(|a| a.contains("exec")),
            "startup command must hand off to a shell: {args:?}"
        );
    }

    #[test]
    fn the_session_is_rekeyed_onto_the_new_terminal_id_and_consumed() {
        let (_, ws) = spawn(Some(session("test-agent")), true);

        assert_eq!(
            ws.agent_session("project", "terminal"),
            Some(session("test-agent")),
            "the restored pane must own the session under its new id"
        );
        assert!(
            ws.project("project")
                .expect("project")
                .pending_agent_resumes
                .is_empty(),
            "a consumed resume must not fire again"
        );
    }

    #[test]
    fn resume_is_skipped_when_the_setting_is_off_but_the_session_is_kept() {
        let (args, ws) = spawn(Some(session("test-agent")), false);

        assert_eq!(args, None, "opt-in setting must gate the resume");
        assert_eq!(
            ws.agent_session("project", "terminal"),
            Some(session("test-agent")),
            "the session identity survives even when we don't auto-run it"
        );
    }

    #[test]
    fn an_unknown_agent_is_stored_but_not_resumed() {
        let (args, ws) = spawn(Some(session("no-such-harness")), true);

        assert_eq!(args, None);
        assert_eq!(
            ws.agent_session("project", "terminal"),
            Some(session("no-such-harness"))
        );
    }

    #[test]
    fn a_pane_with_no_captured_session_launches_plainly() {
        let (args, ws) = spawn(None, true);

        assert_eq!(args, None);
        assert_eq!(ws.agent_session("project", "terminal"), None);
    }

    /// docs/agent-status.md promises the session is dropped on a hard close, a
    /// finalized soft close, or a shell switch. Only the soft close was covered;
    /// these three call sites had no test module at all, so dropping any of them
    /// would leave orphans in workspace.json — and a pane respawned in the same
    /// slot could inherit a stale id and be handed `claude --resume` for it.
    fn workspace_with_live_session() -> (
        crate::workspace::state::Workspace,
        TerminalsRegistry,
        RecordingBackend,
    ) {
        let mut ws = workspace_with_terminal(ShellType::Default, None, Some("terminal"), Default::default());
        ws.set_agent_session("project", "terminal", session("test-agent"), &mut TestCx);
        assert!(ws.agent_session("project", "terminal").is_some());
        (ws, Arc::new(Default::default()), RecordingBackend::default())
    }

    #[test]
    fn closing_a_terminal_forgets_its_agent_session() {
        let (mut ws, terminals, backend) = workspace_with_live_session();
        let mut focus = okena_workspace::focus::FocusManager::new();

        super::terminal::close(
            &mut ws,
            &mut focus,
            "project".to_string(),
            "terminal".to_string(),
            &backend,
            &terminals,
            &mut TestCx,
        );

        assert_eq!(ws.agent_session("project", "terminal"), None);
    }

    #[test]
    fn closing_many_terminals_forgets_their_agent_sessions() {
        let (mut ws, terminals, backend) = workspace_with_live_session();
        let mut focus = okena_workspace::focus::FocusManager::new();

        super::terminal::close_many(
            &mut ws,
            &mut focus,
            "project".to_string(),
            vec!["terminal".to_string()],
            &backend,
            &terminals,
            &mut TestCx,
        );

        assert_eq!(ws.agent_session("project", "terminal"), None);
    }

    #[test]
    fn switching_shell_forgets_the_agent_session() {
        // The pane keeps its slot, but the process behind it is gone — so the
        // captured session no longer describes what runs there.
        let (mut ws, terminals, backend) = workspace_with_live_session();

        super::terminal::switch_shell(
            &mut ws,
            "project".to_string(),
            "terminal".to_string(),
            ShellType::Custom {
                path: "/bin/zsh".to_string(),
                args: Vec::new(),
            },
            &backend,
            &terminals,
            &settings(false),
            &mut TestCx,
        );

        assert_eq!(ws.agent_session("project", "terminal"), None);
    }
}
