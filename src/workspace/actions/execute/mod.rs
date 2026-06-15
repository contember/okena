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
mod tab;
mod terminal;

use crate::remote::bridge::CommandResult;
use crate::remote::types::ActionRequest;
use crate::settings::settings;
use crate::terminal::backend::TerminalBackend;
use crate::terminal::shell_config::ShellType;
use crate::terminal::terminal::{Terminal, TerminalSize};
use okena_terminal::TerminalsRegistry;
use crate::workspace::focus::FocusManager;
use crate::workspace::hooks;
use crate::workspace::state::{LayoutNode, WindowId, Workspace};
use gpui::*;
use std::sync::Arc;

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
pub fn execute_action(
    action: ActionRequest,
    ws: &mut Workspace,
    window_id: WindowId,
    focus_manager: &mut FocusManager,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
    cx: &mut Context<Workspace>,
) -> ActionResult {
    match action {
        // ── Terminal ops ─────────────────────────────────────────────
        ActionRequest::CreateTerminal { project_id } => {
            terminal::create(ws, focus_manager, project_id, backend, terminals, cx)
        }
        ActionRequest::SplitTerminal { project_id, path, direction } => {
            terminal::split(ws, focus_manager, project_id, path, direction, backend, terminals, cx)
        }
        ActionRequest::CloseTerminal { project_id, terminal_id } => {
            terminal::close(ws, focus_manager, project_id, terminal_id, backend, terminals, cx)
        }
        ActionRequest::CloseTerminals { project_id, terminal_ids } => {
            terminal::close_many(ws, focus_manager, project_id, terminal_ids, backend, terminals, cx)
        }
        ActionRequest::FocusTerminal { project_id, terminal_id, window: _ } => {
            // `window` was already consumed at the bridge to resolve the target
            // `window_id` (passed in above); the per-window FocusManager handed
            // to `execute_action` is already the right one.
            terminal::focus(ws, focus_manager, project_id, terminal_id, cx)
        }
        ActionRequest::SendText { terminal_id, text } => {
            terminal::send_text(ws, terminal_id, text, backend, terminals)
        }
        ActionRequest::RunCommand { terminal_id, command } => {
            terminal::run_command(ws, terminal_id, command, backend, terminals)
        }
        ActionRequest::SendSpecialKey { terminal_id, key } => {
            terminal::send_special_key(ws, terminal_id, key, backend, terminals)
        }
        ActionRequest::Resize { terminal_id, cols, rows } => {
            terminal::resize(ws, terminal_id, cols, rows, backend, terminals)
        }
        ActionRequest::UpdateSplitSizes { project_id, path, sizes } => {
            terminal::update_split_sizes(ws, project_id, path, sizes, cx)
        }
        ActionRequest::ToggleMinimized { project_id, terminal_id } => {
            terminal::toggle_minimized(ws, project_id, terminal_id, cx)
        }
        ActionRequest::SetFullscreen { project_id, terminal_id, window: _ } => {
            terminal::set_fullscreen(ws, focus_manager, project_id, terminal_id, cx)
        }
        ActionRequest::RenameTerminal { project_id, terminal_id, name } => {
            terminal::rename(ws, project_id, terminal_id, name, cx)
        }
        ActionRequest::ReadContent { terminal_id } => {
            terminal::read_content(ws, terminal_id, backend, terminals)
        }

        // ── Tab / pane-move ops ──────────────────────────────────────
        ActionRequest::AddTab { project_id, path, in_group } => {
            tab::add_tab(ws, focus_manager, project_id, path, in_group, backend, terminals, cx)
        }
        ActionRequest::SetActiveTab { project_id, path, index } => {
            tab::set_active_tab(ws, project_id, path, index, cx)
        }
        ActionRequest::MoveTab { project_id, path, from_index, to_index } => {
            tab::move_tab(ws, project_id, path, from_index, to_index, cx)
        }
        ActionRequest::MoveTerminalToTabGroup { project_id, terminal_id, target_path, position, target_project_id } => {
            tab::move_terminal_to_tab_group(ws, focus_manager, project_id, terminal_id, target_path, position, target_project_id, cx)
        }
        ActionRequest::MovePaneTo { project_id, terminal_id, target_project_id, target_terminal_id, zone } => {
            tab::move_pane_to(ws, focus_manager, project_id, terminal_id, target_project_id, target_terminal_id, zone, cx)
        }

        // ── Git ops ──────────────────────────────────────────────────
        ActionRequest::GitStatus { project_id } => git::status(ws, project_id),
        ActionRequest::GitDiffSummary { project_id } => git::diff_summary(ws, project_id),
        ActionRequest::GitDiff { project_id, mode, ignore_whitespace } => {
            git::diff(ws, project_id, mode, ignore_whitespace)
        }
        ActionRequest::GitBranches { project_id } => git::branches(ws, project_id),
        ActionRequest::GitFileContents { project_id, file_path, mode } => {
            git::file_contents(ws, project_id, file_path, mode)
        }
        ActionRequest::GitCommitGraph { project_id, count, branch } => {
            git::commit_graph(ws, project_id, count, branch)
        }
        ActionRequest::GitListBranches { project_id } => git::list_branches(ws, project_id),
        ActionRequest::GitStageFile { project_id, file_path } => {
            git::stage_file(ws, project_id, file_path)
        }
        ActionRequest::GitUnstageFile { project_id, file_path } => {
            git::unstage_file(ws, project_id, file_path)
        }
        ActionRequest::GitDiscardFile { project_id, file_path } => {
            git::discard_file(ws, project_id, file_path)
        }
        ActionRequest::GitBlame { project_id, relative_path } => {
            git::blame(ws, project_id, relative_path)
        }

        // ── Filesystem ops ───────────────────────────────────────────
        ActionRequest::ListFiles { project_id, show_ignored } => {
            files::list_files(ws, project_id, show_ignored)
        }
        ActionRequest::ListDirectory { project_id, relative_path, show_ignored } => {
            files::list_directory(ws, project_id, relative_path, show_ignored)
        }
        ActionRequest::ReadFile { project_id, relative_path } => {
            files::read_file(ws, project_id, relative_path)
        }
        ActionRequest::ReadFileBytes { project_id, relative_path } => {
            files::read_file_bytes(ws, project_id, relative_path)
        }
        ActionRequest::FileSize { project_id, relative_path } => {
            files::file_size(ws, project_id, relative_path)
        }
        ActionRequest::SearchContent { project_id, query, case_sensitive, mode, max_results, file_glob, context_lines } => {
            files::search_content(ws, project_id, query, case_sensitive, mode, max_results, file_glob, context_lines)
        }
        ActionRequest::RenameFile { project_id, relative_path, new_name } => {
            files::rename_file(ws, project_id, relative_path, new_name)
        }
        ActionRequest::DeleteFile { project_id, relative_path } => {
            files::delete_file(ws, project_id, relative_path)
        }
        ActionRequest::CreateFile { project_id, relative_path } => {
            files::create_file(ws, project_id, relative_path)
        }
        ActionRequest::CreateDirectory { project_id, relative_path } => {
            files::create_directory(ws, project_id, relative_path)
        }

        // ── Project / folder / worktree ops ──────────────────────────
        ActionRequest::AddProject { name, path } => {
            project::add_project(ws, window_id, name, path, backend, terminals, cx)
        }
        ActionRequest::ReorderProjectInFolder { folder_id, project_id, new_index } => {
            project::reorder_in_folder(ws, folder_id, project_id, new_index, cx)
        }
        ActionRequest::SetProjectColor { project_id, color } => {
            project::set_project_color(ws, project_id, color, cx)
        }
        ActionRequest::SetFolderColor { folder_id, color } => {
            project::set_folder_color(ws, folder_id, color, cx)
        }
        ActionRequest::RenameProject { project_id, name } => {
            project::rename_project(ws, project_id, name, cx)
        }
        ActionRequest::RenameProjectDirectory { project_id, new_name } => {
            project::rename_project_directory(ws, project_id, new_name, cx)
        }
        ActionRequest::DeleteProject { project_id } => {
            project::delete_project(ws, focus_manager, project_id, cx)
        }
        ActionRequest::SetProjectShowInOverview { project_id, show, window: _ } => {
            project::set_show_in_overview(ws, focus_manager, window_id, project_id, show, cx)
        }
        ActionRequest::RemoveWorktreeProject { project_id, force } => {
            project::remove_worktree_project(ws, focus_manager, project_id, force, cx)
        }
        ActionRequest::CreateFolder { name } => project::create_folder(ws, name, cx),
        ActionRequest::DeleteFolder { folder_id } => project::delete_folder(ws, folder_id, cx),
        ActionRequest::RenameFolder { folder_id, name } => {
            project::rename_folder(ws, folder_id, name, cx)
        }
        ActionRequest::MoveProjectToFolder { project_id, folder_id, position } => {
            project::move_to_folder(ws, project_id, folder_id, position, cx)
        }
        ActionRequest::MoveProjectOutOfFolder { project_id, top_level_index } => {
            project::move_out_of_folder(ws, project_id, top_level_index, cx)
        }
        ActionRequest::CreateWorktree { project_id, branch, create_branch } => {
            project::create_worktree(ws, window_id, project_id, branch, create_branch, backend, terminals, cx)
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
) -> Option<Arc<Terminal>> {
    // Fast path: already in registry
    if let Some(term) = terminals.lock().get(terminal_id).cloned() {
        return Some(term);
    }

    // Find which project owns this terminal_id and get its path
    let mut cwd = None;
    for project in &ws.data().projects {
        if let Some(layout) = &project.layout
            && layout.find_terminal_path(terminal_id).is_some() {
                cwd = Some(project.path.clone());
                break;
            }
    }
    let cwd = cwd?;

    // Spawn PTY via backend
    match backend.reconnect_terminal(terminal_id, &cwd, None) {
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

/// Spawn PTYs for any uninitialized terminals (`terminal_id: None`) in a project's layout.
///
/// Used after `CreateTerminal` / `SplitTerminal` to eagerly create PTYs for
/// remote clients that don't have a rendering layer to trigger lazy spawning.
pub fn spawn_uninitialized_terminals(
    ws: &mut Workspace,
    project_id: &str,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
    cx: &mut Context<Workspace>,
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
    let project_name = project.name.clone();
    let project_hooks = project.hooks.clone();
    let is_worktree = project.worktree_info.is_some();
    let parent_hooks = project.worktree_info.as_ref()
        .and_then(|wt| ws.project(&wt.parent_project_id))
        .map(|p| p.hooks.clone());
    let project_default_shell = project.default_shell.clone();
    let mut uninitialized = Vec::new();
    if let Some(layout) = &project.layout {
        collect_uninitialized_terminals_with_shell(layout, vec![], &mut uninitialized);
    }
    log::info!("spawn_uninitialized_terminals: project={}, uninitialized_count={}", project_id, uninitialized.len());

    let app_settings = settings(cx);
    let global_default = app_settings.default_shell.clone();
    let global_hooks = app_settings.hooks;

    // Resolve shell_wrapper and on_create once for all terminals in this project
    let shell_wrapper = hooks::resolve_shell_wrapper(&project_hooks, parent_hooks.as_ref(), &global_hooks);
    let on_create_cmd = hooks::resolve_terminal_on_create(&project_hooks, parent_hooks.as_ref(), &global_hooks, cx);
    let folder = ws.folder_for_project_or_parent(project_id);
    let folder_id = folder.map(|f| f.id.as_str());
    let folder_name = folder.map(|f| f.name.as_str());
    let env = hooks::terminal_hook_env(project_id, &project_name, &project_path, is_worktree, folder_id, folder_name);

    let mut spawned_ids = Vec::new();
    for (path, shell_type) in uninitialized {
        let mut shell = match shell_type {
            ShellType::Default => project_default_shell
                .clone()
                .unwrap_or_else(|| global_default.clone()),
            other => other,
        };

        // Apply shell_wrapper if configured
        if let Some(ref wrapper) = shell_wrapper {
            shell = hooks::apply_shell_wrapper(&shell, wrapper, &env);
        }

        // Apply on_create: wrap shell to run command first, then exec into shell
        if let Some(ref cmd) = on_create_cmd {
            shell = hooks::apply_on_create(&shell, cmd, &env);
        }

        match backend.create_terminal(&project_path, Some(&shell)) {
            Ok(terminal_id) => {
                ws.set_terminal_id(project_id, &path, terminal_id.clone(), cx);
                let terminal = Arc::new(Terminal::new(
                    terminal_id.clone(),
                    TerminalSize::default(),
                    backend.transport(),
                    project_path.clone(),
                ));

                terminals.lock().insert(terminal_id.clone(), terminal);
                spawned_ids.push(terminal_id);
            }
            Err(e) => {
                log::error!(
                    "Failed to spawn terminal for project {}: {}",
                    project_id,
                    e
                );
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
fn resolve_project_file(project_path: &str, relative_path: &str) -> Result<std::path::PathBuf, String> {
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
fn resolve_new_project_file(project_path: &str, relative_path: &str) -> Result<std::path::PathBuf, String> {
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
