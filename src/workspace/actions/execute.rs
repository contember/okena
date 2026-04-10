//! Unified action execution layer.
//!
//! Single entry point for all `ActionRequest` actions — used by both
//! the desktop UI and the remote API to eliminate code duplication
//! and ensure consistent behavior.

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point};
use crate::remote::bridge::CommandResult;
use crate::remote::types::ActionRequest;
use crate::settings::settings;
use crate::terminal::backend::TerminalBackend;
use crate::terminal::shell_config::ShellType;
use crate::terminal::terminal::{Terminal, TerminalSize};
use crate::workspace::state::DropZone;
use okena_terminal::TerminalsRegistry;
use crate::workspace::hooks;
use crate::workspace::state::{LayoutNode, Workspace};
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
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
    cx: &mut Context<Workspace>,
) -> ActionResult {
    match action {
        ActionRequest::CreateTerminal { project_id } => {
            ws.add_terminal(&project_id, cx);
            spawn_uninitialized_terminals(ws, &project_id, backend, terminals, cx)
        }
        ActionRequest::SplitTerminal {
            project_id,
            path,
            direction,
        } => {
            ws.split_terminal(&project_id, &path, direction, cx);
            spawn_uninitialized_terminals(ws, &project_id, backend, terminals, cx)
        }
        ActionRequest::CloseTerminal {
            project_id,
            terminal_id,
        } => {
            let path = find_terminal_path(ws, &project_id, &terminal_id);
            match path {
                Some(path) => {
                    backend.kill(&terminal_id);
                    terminals.lock().remove(&terminal_id);
                    ws.close_terminal_and_focus_sibling(&project_id, &path, cx);
                    ActionResult::Ok(None)
                }
                None => ActionResult::Err(format!("terminal not found: {}", terminal_id)),
            }
        }
        ActionRequest::CloseTerminals {
            project_id,
            terminal_ids,
        } => {
            let mut last_err = None;
            for terminal_id in &terminal_ids {
                let path = find_terminal_path(ws, &project_id, terminal_id);
                match path {
                    Some(path) => {
                        backend.kill(terminal_id);
                        terminals.lock().remove(terminal_id);
                        ws.close_terminal_and_focus_sibling(&project_id, &path, cx);
                    }
                    None => {
                        last_err = Some(format!("terminal not found: {}", terminal_id));
                    }
                }
            }
            match last_err {
                Some(e) => ActionResult::Err(e),
                None => ActionResult::Ok(None),
            }
        }
        ActionRequest::FocusTerminal {
            project_id,
            terminal_id,
        } => {
            let path = find_terminal_path(ws, &project_id, &terminal_id);
            match path {
                Some(path) => {
                    ws.set_focused_terminal(project_id, path, cx);
                    ActionResult::Ok(None)
                }
                None => ActionResult::Err(format!("terminal not found: {}", terminal_id)),
            }
        }
        ActionRequest::SendText { terminal_id, text } => {
            match ensure_terminal(&terminal_id, terminals, backend, ws) {
                Some(term) => {
                    term.claim_resize_remote();
                    term.send_input(&text);
                    ActionResult::Ok(None)
                }
                None => ActionResult::Err(format!("terminal not found: {}", terminal_id)),
            }
        }
        ActionRequest::RunCommand {
            terminal_id,
            command,
        } => match ensure_terminal(&terminal_id, terminals, backend, ws) {
            Some(term) => {
                term.claim_resize_remote();
                term.send_input(&format!("{}\r", command));
                ActionResult::Ok(None)
            }
            None => ActionResult::Err(format!("terminal not found: {}", terminal_id)),
        },
        ActionRequest::SendSpecialKey { terminal_id, key } => {
            match ensure_terminal(&terminal_id, terminals, backend, ws) {
                Some(term) => {
                    term.claim_resize_remote();
                    term.send_bytes(key.to_bytes());
                    ActionResult::Ok(None)
                }
                None => ActionResult::Err(format!("terminal not found: {}", terminal_id)),
            }
        }
        ActionRequest::Resize {
            terminal_id,
            cols,
            rows,
        } => match ensure_terminal(&terminal_id, terminals, backend, ws) {
            Some(term) => {
                term.claim_resize_remote();
                let size = TerminalSize {
                    cols,
                    rows,
                    cell_width: 8.0,
                    cell_height: 16.0,
                };
                term.resize(size);
                ActionResult::Ok(None)
            }
            None => ActionResult::Err(format!("terminal not found: {}", terminal_id)),
        },
        ActionRequest::UpdateSplitSizes {
            project_id,
            path,
            sizes,
        } => {
            ws.update_split_sizes(&project_id, &path, sizes, cx);
            ActionResult::Ok(None)
        }
        ActionRequest::ToggleMinimized {
            project_id,
            terminal_id,
        } => {
            ws.toggle_terminal_minimized_by_id(&project_id, &terminal_id, cx);
            ActionResult::Ok(None)
        }
        ActionRequest::SetFullscreen {
            project_id,
            terminal_id,
        } => {
            match terminal_id {
                Some(tid) => ws.set_fullscreen_terminal(project_id, tid, cx),
                None => ws.exit_fullscreen(cx),
            }
            ActionResult::Ok(None)
        }
        ActionRequest::RenameTerminal {
            project_id,
            terminal_id,
            name,
        } => {
            ws.rename_terminal(&project_id, &terminal_id, name, cx);
            ActionResult::Ok(None)
        }
        ActionRequest::AddTab {
            project_id,
            path,
            in_group,
        } => {
            if in_group {
                ws.add_tab_to_group(&project_id, &path, cx);
            } else {
                ws.add_tab(&project_id, &path, cx);
            }
            spawn_uninitialized_terminals(ws, &project_id, backend, terminals, cx)
        }
        ActionRequest::SetActiveTab {
            project_id,
            path,
            index,
        } => {
            ws.set_active_tab(&project_id, &path, index, cx);
            ActionResult::Ok(None)
        }
        ActionRequest::MoveTab {
            project_id,
            path,
            from_index,
            to_index,
        } => {
            ws.move_tab(&project_id, &path, from_index, to_index, cx);
            ActionResult::Ok(None)
        }
        ActionRequest::MoveTerminalToTabGroup {
            project_id,
            terminal_id,
            target_path,
            position,
            target_project_id,
        } => {
            let target_pid = target_project_id.as_deref().unwrap_or(&project_id);
            ws.move_terminal_to_tab_group(&project_id, &terminal_id, target_pid, &target_path, position, cx);
            ActionResult::Ok(None)
        }
        ActionRequest::MovePaneTo {
            project_id,
            terminal_id,
            target_project_id,
            target_terminal_id,
            zone,
        } => {
            let drop_zone = match zone.as_str() {
                "top" => DropZone::Top,
                "bottom" => DropZone::Bottom,
                "left" => DropZone::Left,
                "right" => DropZone::Right,
                "center" => DropZone::Center,
                _ => return ActionResult::Err(format!("invalid drop zone: {}", zone)),
            };
            ws.move_pane(&project_id, &terminal_id, &target_project_id, &target_terminal_id, drop_zone, cx);
            ActionResult::Ok(None)
        }
        ActionRequest::GitStatus { project_id } => {
            match ws.project(&project_id) {
                Some(p) => {
                    let path = p.path.clone();
                    let status = crate::git::get_git_status(std::path::Path::new(&path));
                    ActionResult::Ok(Some(serde_json::to_value(status).expect("BUG: GitStatus must serialize")))
                }
                None => ActionResult::Err(format!("project not found: {}", project_id)),
            }
        }
        ActionRequest::GitDiffSummary { project_id } => {
            match ws.project(&project_id) {
                Some(p) => {
                    let path = p.path.clone();
                    let summary = crate::git::get_diff_file_summary(std::path::Path::new(&path));
                    ActionResult::Ok(Some(serde_json::to_value(summary).expect("BUG: FileDiffSummary must serialize")))
                }
                None => ActionResult::Err(format!("project not found: {}", project_id)),
            }
        }
        ActionRequest::GitDiff { project_id, mode, ignore_whitespace } => {
            match ws.project(&project_id) {
                Some(p) => {
                    let path = p.path.clone();
                    match crate::git::get_diff_with_options(std::path::Path::new(&path), mode, ignore_whitespace) {
                        Ok(diff) => ActionResult::Ok(Some(serde_json::to_value(diff).expect("BUG: DiffResult must serialize"))),
                        Err(e) => ActionResult::Err(e),
                    }
                }
                None => ActionResult::Err(format!("project not found: {}", project_id)),
            }
        }
        ActionRequest::GitBranches { project_id } => {
            match ws.project(&project_id) {
                Some(p) => {
                    let path = p.path.clone();
                    let branches = crate::git::get_available_branches_for_worktree(std::path::Path::new(&path));
                    ActionResult::Ok(Some(serde_json::to_value(branches).expect("BUG: branches must serialize")))
                }
                None => ActionResult::Err(format!("project not found: {}", project_id)),
            }
        }
        ActionRequest::GitFileContents { project_id, file_path, mode } => {
            match ws.project(&project_id) {
                Some(p) => {
                    let repo_path = p.path.clone();
                    let (old, new) = crate::git::get_file_contents_for_diff(
                        std::path::Path::new(&repo_path),
                        &file_path,
                        mode,
                    );
                    ActionResult::Ok(Some(serde_json::json!({
                        "old_content": old,
                        "new_content": new,
                    })))
                }
                None => ActionResult::Err(format!("project not found: {}", project_id)),
            }
        }
        ActionRequest::GitCommitGraph { project_id, count, branch } => {
            match ws.project(&project_id) {
                Some(p) => {
                    let path = p.path.clone();
                    let entries = crate::git::get_commit_graph(
                        std::path::Path::new(&path),
                        count,
                        branch.as_deref(),
                    );
                    ActionResult::Ok(Some(serde_json::to_value(entries).expect("BUG: GraphRow must serialize")))
                }
                None => ActionResult::Err(format!("project not found: {}", project_id)),
            }
        }
        ActionRequest::GitListBranches { project_id } => {
            match ws.project(&project_id) {
                Some(p) => {
                    let path = p.path.clone();
                    let branches = crate::git::list_branches(std::path::Path::new(&path));
                    ActionResult::Ok(Some(serde_json::to_value(branches).expect("BUG: branches must serialize")))
                }
                None => ActionResult::Err(format!("project not found: {}", project_id)),
            }
        }
        ActionRequest::ListFiles { project_id, show_ignored, show_hidden } => {
            match ws.project(&project_id) {
                Some(p) => {
                    let path = match std::path::Path::new(&p.path).canonicalize() {
                        Ok(c) => c,
                        Err(e) => return ActionResult::Err(format!("Cannot resolve project path: {}", e)),
                    };
                    let files = okena_files::file_search::FileSearchDialog::scan_files(&path, show_ignored, show_hidden);
                    ActionResult::Ok(Some(serde_json::to_value(files).expect("BUG: FileEntry must serialize")))
                }
                None => ActionResult::Err(format!("project not found: {}", project_id)),
            }
        }
        ActionRequest::ReadFile { project_id, relative_path } => {
            match ws.project(&project_id) {
                Some(p) => {
                    let canonical = match resolve_project_file(&p.path, &relative_path) {
                        Ok(c) => c,
                        Err(e) => return ActionResult::Err(e),
                    };
                    match std::fs::read_to_string(&canonical) {
                        Ok(content) => ActionResult::Ok(Some(serde_json::json!({ "content": content }))),
                        Err(e) => ActionResult::Err(format!("Cannot read file: {}", e)),
                    }
                }
                None => ActionResult::Err(format!("project not found: {}", project_id)),
            }
        }
        ActionRequest::FileSize { project_id, relative_path } => {
            match ws.project(&project_id) {
                Some(p) => {
                    let canonical = match resolve_project_file(&p.path, &relative_path) {
                        Ok(c) => c,
                        Err(e) => return ActionResult::Err(e),
                    };
                    match std::fs::metadata(&canonical) {
                        Ok(m) => ActionResult::Ok(Some(serde_json::json!({ "size": m.len() }))),
                        Err(e) => ActionResult::Err(format!("Cannot read file: {}", e)),
                    }
                }
                None => ActionResult::Err(format!("project not found: {}", project_id)),
            }
        }
        ActionRequest::SearchContent { project_id, query, case_sensitive, mode, max_results, file_glob, context_lines } => {
            if let Some(ref glob) = file_glob {
                if glob.contains("..") || glob.starts_with('/') {
                    return ActionResult::Err("file_glob must not contain '..' or start with '/'".to_string());
                }
            }
            match ws.project(&project_id) {
                Some(p) => {
                    let path = match std::path::Path::new(&p.path).canonicalize() {
                        Ok(c) => c,
                        Err(e) => return ActionResult::Err(format!("Cannot resolve project path: {}", e)),
                    };
                    let search_mode = match mode.as_str() {
                        "regex" => okena_files::content_search::SearchMode::Regex,
                        "fuzzy" => okena_files::content_search::SearchMode::Fuzzy,
                        _ => okena_files::content_search::SearchMode::Literal,
                    };
                    let config = okena_files::content_search::ContentSearchConfig {
                        case_sensitive,
                        mode: search_mode,
                        max_results,
                        file_glob,
                        context_lines,
                        show_ignored: false,
                        show_hidden: false,
                    };
                    let cancelled = std::sync::atomic::AtomicBool::new(false);
                    let mut results = Vec::new();
                    okena_files::content_search::search_content(
                        &path, &query, &config, &cancelled, &mut |result| results.push(result),
                    );
                    ActionResult::Ok(Some(serde_json::to_value(results).expect("BUG: FileSearchResult must serialize")))
                }
                None => ActionResult::Err(format!("project not found: {}", project_id)),
            }
        }
        ActionRequest::AddProject { name, path } => {
            let project_id = ws.add_project(name, path, true, &settings(cx).hooks, cx);
            spawn_uninitialized_terminals(ws, &project_id, backend, terminals, cx)
        }
        ActionRequest::ReorderProjectInFolder {
            folder_id,
            project_id,
            new_index,
        } => {
            ws.reorder_project_in_folder(&folder_id, &project_id, new_index, cx);
            ActionResult::Ok(None)
        }
        ActionRequest::SetProjectColor { project_id, color } => {
            ws.set_folder_color(&project_id, color, cx);
            ActionResult::Ok(None)
        }
        ActionRequest::SetFolderColor { folder_id, color } => {
            ws.set_folder_item_color(&folder_id, color, cx);
            ActionResult::Ok(None)
        }
        ActionRequest::ReadContent { terminal_id } => {
            match ensure_terminal(&terminal_id, terminals, backend, ws) {
                Some(term) => {
                    let content = term.with_content(|term| {
                        let grid = term.grid();
                        let screen_lines = grid.screen_lines();
                        let cols = grid.columns();
                        let mut lines = Vec::with_capacity(screen_lines);

                        for row in 0..screen_lines as i32 {
                            let mut line = String::with_capacity(cols);
                            for col in 0..cols {
                                let cell = &grid[Point::new(Line(row), Column(col))];
                                line.push(cell.c);
                            }
                            let trimmed = line.trim_end().to_string();
                            lines.push(trimmed);
                        }

                        while lines.last().map_or(false, |l| l.is_empty()) {
                            lines.pop();
                        }

                        lines.join("\n")
                    });
                    ActionResult::Ok(Some(serde_json::json!({"content": content})))
                }
                None => ActionResult::Err(format!("terminal not found: {}", terminal_id)),
            }
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
        ActionRequest::CreateWorktree { project_id, branch, create_branch } => {
            let project = match ws.project(&project_id) {
                Some(p) => p,
                None => return ActionResult::Err(format!("project not found: {}", project_id)),
            };
            let project_path = std::path::PathBuf::from(&project.path);
            let (git_root, subdir) = okena_git::resolve_git_root_and_subdir(&project_path);
            let path_template = settings(cx).worktree.path_template.clone();
            let (worktree_path, wt_project_path) = okena_git::compute_target_paths(&git_root, &subdir, &path_template, &branch);
            let global_hooks = settings(cx).hooks.clone();

            match ws.create_worktree_project(&project_id, &branch, &git_root, &worktree_path, &wt_project_path, create_branch, &global_hooks, cx) {
                Ok(new_project_id) => {
                    let result = spawn_uninitialized_terminals(ws, &new_project_id, backend, terminals, cx);
                    let terminal_id = ws.project(&new_project_id)
                        .and_then(|p| p.layout.as_ref())
                        .and_then(|l| find_first_terminal_id(l));
                    match result {
                        ActionResult::Ok(_) => ActionResult::Ok(Some(serde_json::json!({
                            "project_id": new_project_id,
                            "terminal_id": terminal_id,
                            "path": wt_project_path,
                        }))),
                        err => err,
                    }
                }
                Err(e) => ActionResult::Err(e),
            }
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
        if let Some(layout) = &project.layout {
            if layout.find_terminal_path(terminal_id).is_some() {
                cwd = Some(project.path.clone());
                break;
            }
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
    if ws.creating_projects.contains(project_id) {
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
            if let Some(wrapped) = hooks::apply_on_create(&shell, cmd, &env) {
                shell = wrapped;
            }
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
