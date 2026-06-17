//! Project, folder, and worktree action handlers.

// Handlers take the workspace, focus manager, terminals registry and cx as
// distinct dependencies; bundling them into a context struct would obscure
// more than it clarifies here.
#![allow(clippy::too_many_arguments)]

use super::{ActionResult, find_first_terminal_id, spawn_uninitialized_terminals};
use crate::settings::settings;
use okena_terminal::backend::TerminalBackend;
use crate::workspace::focus::FocusManager;
use crate::workspace::state::{WindowId, Workspace};
use gpui::*;
use okena_core::theme::FolderColor;
use okena_terminal::TerminalsRegistry;

pub(super) fn add_project(
    ws: &mut Workspace,
    window_id: WindowId,
    name: String,
    path: String,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
    cx: &mut Context<Workspace>,
) -> ActionResult {
    let project_id = ws.add_project(name, path, true, &settings(cx).hooks, window_id, cx);
    // Surface the newly-created project's id alongside the spawned terminal
    // ids so callers (e.g. the CLI `add-project` verb) can address the project
    // they just created without re-fetching state. `spawn_uninitialized_terminals`
    // returns `{ "terminal_ids": [...] }`; we merge `project_id` into that
    // object, leaving its terminal-spawning behavior unchanged.
    match spawn_uninitialized_terminals(ws, &project_id, backend, terminals, cx) {
        ActionResult::Ok(Some(serde_json::Value::Object(mut map))) => {
            map.insert(
                "project_id".to_string(),
                serde_json::Value::String(project_id),
            );
            ActionResult::Ok(Some(serde_json::Value::Object(map)))
        }
        ActionResult::Ok(_) => ActionResult::Ok(Some(serde_json::json!({
            "project_id": project_id,
            "terminal_ids": [],
        }))),
        err => err,
    }
}

pub(super) fn reorder_in_folder(
    ws: &mut Workspace,
    folder_id: String,
    project_id: String,
    new_index: usize,
    cx: &mut Context<Workspace>,
) -> ActionResult {
    ws.reorder_project_in_folder(&folder_id, &project_id, new_index, cx);
    ActionResult::Ok(None)
}

pub(super) fn set_project_color(
    ws: &mut Workspace,
    project_id: String,
    color: FolderColor,
    cx: &mut Context<Workspace>,
) -> ActionResult {
    ws.set_folder_color(&project_id, color, cx);
    ActionResult::Ok(None)
}

pub(super) fn set_folder_color(
    ws: &mut Workspace,
    folder_id: String,
    color: FolderColor,
    cx: &mut Context<Workspace>,
) -> ActionResult {
    ws.set_folder_item_color(&folder_id, color, cx);
    ActionResult::Ok(None)
}

pub(super) fn rename_project(
    ws: &mut Workspace,
    project_id: String,
    name: String,
    cx: &mut Context<Workspace>,
) -> ActionResult {
    if ws.project(&project_id).is_none() {
        return ActionResult::Err(format!("project not found: {}", project_id));
    }
    ws.rename_project(&project_id, name, cx);
    ActionResult::Ok(None)
}

pub(super) fn rename_project_directory(
    ws: &mut Workspace,
    project_id: String,
    new_name: String,
    cx: &mut Context<Workspace>,
) -> ActionResult {
    if let Err(e) = super::validate_leaf_name(&new_name) {
        return ActionResult::Err(e);
    }
    let current_path = match ws.project(&project_id) {
        Some(p) => p.path.clone(),
        None => return ActionResult::Err(format!("project not found: {}", project_id)),
    };
    let old_path = std::path::Path::new(&current_path);
    let parent = match old_path.parent() {
        Some(p) => p,
        None => return ActionResult::Err("cannot determine parent directory".to_string()),
    };
    let new_path = parent.join(&new_name);
    if new_path.exists() {
        return ActionResult::Err(format!("'{}' already exists", new_name));
    }
    if let Err(e) = std::fs::rename(old_path, &new_path) {
        return ActionResult::Err(format!("Failed to rename: {}", e));
    }
    let new_path_str = new_path.to_string_lossy().to_string();
    ws.rename_project_directory(&project_id, new_path_str, new_name, cx);
    ActionResult::Ok(None)
}

pub(super) fn delete_project(
    ws: &mut Workspace,
    focus_manager: &mut FocusManager,
    project_id: String,
    cx: &mut Context<Workspace>,
) -> ActionResult {
    if ws.project(&project_id).is_none() {
        return ActionResult::Err(format!("project not found: {}", project_id));
    }
    let global_hooks = settings(cx).hooks.clone();
    ws.delete_project(focus_manager, &project_id, &global_hooks, cx);
    ActionResult::Ok(None)
}

pub(super) fn set_show_in_overview(
    ws: &mut Workspace,
    focus_manager: &mut FocusManager,
    window_id: WindowId,
    project_id: String,
    show: bool,
    cx: &mut Context<Workspace>,
) -> ActionResult {
    apply_set_project_show_in_overview(ws, focus_manager, window_id, &project_id, show, cx)
}

/// Apply the SetProjectShowInOverview action against the targeted window.
///
/// Reads the project's current per-window visibility from the targeted
/// window's `hidden_project_ids`, then toggles only when the desired and
/// current states differ. `window_id` carries through from `execute_action`
/// so remote-bridge invocations land on whichever window currently has OS
/// focus. For unknown extras (close-race), the read returns `None`; we treat
/// the project as visible and the toggle delegates to the silent-no-op path.
fn apply_set_project_show_in_overview(
    ws: &mut Workspace,
    focus_manager: &mut FocusManager,
    window_id: WindowId,
    project_id: &str,
    show: bool,
    cx: &mut Context<Workspace>,
) -> ActionResult {
    if ws.project(project_id).is_none() {
        return ActionResult::Err(format!("project not found: {}", project_id));
    }
    let current_hidden = ws
        .data()
        .window(window_id)
        .map(|w| w.hidden_project_ids.contains(project_id))
        .unwrap_or(false);
    let current_visible = !current_hidden;
    if current_visible != show {
        ws.toggle_project_overview_visibility(focus_manager, window_id, project_id, cx);
    }
    ActionResult::Ok(None)
}

pub(super) fn remove_worktree_project(
    ws: &mut Workspace,
    focus_manager: &mut FocusManager,
    project_id: String,
    force: bool,
    cx: &mut Context<Workspace>,
) -> ActionResult {
    if ws.project(&project_id).is_none() {
        return ActionResult::Err(format!("project not found: {}", project_id));
    }
    let global_hooks = settings(cx).hooks.clone();
    match ws.remove_worktree_project(focus_manager, &project_id, force, &global_hooks, cx) {
        Ok(()) => ActionResult::Ok(None),
        Err(e) => ActionResult::Err(e),
    }
}

pub(super) fn create_folder(ws: &mut Workspace, name: String, cx: &mut Context<Workspace>) -> ActionResult {
    let id = ws.create_folder(name, cx);
    ActionResult::Ok(Some(serde_json::json!({ "folder_id": id })))
}

pub(super) fn delete_folder(ws: &mut Workspace, folder_id: String, cx: &mut Context<Workspace>) -> ActionResult {
    ws.delete_folder(&folder_id, cx);
    ActionResult::Ok(None)
}

pub(super) fn rename_folder(ws: &mut Workspace, folder_id: String, name: String, cx: &mut Context<Workspace>) -> ActionResult {
    ws.rename_folder(&folder_id, name, cx);
    ActionResult::Ok(None)
}

pub(super) fn move_to_folder(
    ws: &mut Workspace,
    project_id: String,
    folder_id: String,
    position: Option<usize>,
    cx: &mut Context<Workspace>,
) -> ActionResult {
    if ws.project(&project_id).is_none() {
        return ActionResult::Err(format!("project not found: {}", project_id));
    }
    ws.move_project_to_folder(&project_id, &folder_id, position, cx);
    ActionResult::Ok(None)
}

pub(super) fn move_out_of_folder(
    ws: &mut Workspace,
    project_id: String,
    top_level_index: usize,
    cx: &mut Context<Workspace>,
) -> ActionResult {
    if ws.project(&project_id).is_none() {
        return ActionResult::Err(format!("project not found: {}", project_id));
    }
    ws.move_project_out_of_folder(&project_id, top_level_index, cx);
    ActionResult::Ok(None)
}

pub(super) fn create_worktree(
    ws: &mut Workspace,
    window_id: WindowId,
    project_id: String,
    branch: String,
    create_branch: bool,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
    cx: &mut Context<Workspace>,
) -> ActionResult {
    let project = match ws.project(&project_id) {
        Some(p) => p,
        None => return ActionResult::Err(format!("project not found: {}", project_id)),
    };
    let project_path = std::path::PathBuf::from(&project.path);
    let (git_root, subdir) = okena_git::resolve_git_root_and_subdir(&project_path);
    let path_template = settings(cx).worktree.path_template.clone();
    let (worktree_path, wt_project_path) = okena_git::compute_target_paths(&git_root, &subdir, &path_template, &branch);
    let global_hooks = settings(cx).hooks.clone();

    match ws.create_worktree_project(&project_id, &branch, &git_root, &worktree_path, &wt_project_path, create_branch, &global_hooks, window_id, cx) {
        Ok(new_project_id) => {
            let result = spawn_uninitialized_terminals(ws, &new_project_id, backend, terminals, cx);
            let terminal_id = ws.project(&new_project_id)
                .and_then(|p| p.layout.as_ref())
                .and_then(find_first_terminal_id);
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

#[cfg(test)]
mod set_show_in_overview_tests {
    use super::{apply_set_project_show_in_overview, ActionResult};
    use crate::workspace::state::{ProjectData, Workspace, WindowId, WindowState, WorkspaceData};
    use crate::workspace::settings::HooksConfig;
    use gpui::AppContext as _;
    use okena_core::theme::FolderColor;
    use std::collections::HashMap;

    fn make_workspace_data() -> WorkspaceData {
        WorkspaceData {
            version: 1,
            projects: vec![],
            project_order: vec![],
            service_panel_heights: HashMap::new(),
            hook_panel_heights: HashMap::new(),
            folders: vec![],
            main_window: WindowState::default(),
            extra_windows: Vec::new(),
        }
    }

    fn make_project(id: &str) -> ProjectData {
        ProjectData {
            id: id.to_string(),
            name: format!("Project {}", id),
            path: "/tmp/test".to_string(),
            layout: None,
            terminal_names: HashMap::new(),
            hidden_terminals: HashMap::new(),
            worktree_info: None,
            worktree_ids: Vec::new(),
            folder_color: FolderColor::default(),
            hooks: HooksConfig::default(),
            is_remote: false,
            connection_id: None,
            service_terminals: HashMap::new(),
            default_shell: None,
            hook_terminals: HashMap::new(),
            pinned: false,
            last_activity_at: None,
        }
    }

    #[gpui::test]
    fn apply_set_project_show_in_overview_reads_hidden_set(
        cx: &mut gpui::TestAppContext,
    ) {
        // The action's visibility decision must read from the targeted
        // window's hidden_project_ids. This fixture starts with p1 hidden in
        // main; the action says `show: true`, so the helper toggles,
        // clearing main's hidden set.
        let mut data = make_workspace_data();
        data.projects = vec![make_project("p1")];
        data.project_order = vec!["p1".to_string()];
        data.main_window.hidden_project_ids.insert("p1".to_string());
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            let mut fm = crate::workspace::focus::FocusManager::new();
            let result =
                apply_set_project_show_in_overview(ws, &mut fm, WindowId::Main, "p1", true, cx);
            assert!(matches!(result, ActionResult::Ok(_)));
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert!(
                !ws.data().main_window.hidden_project_ids.contains("p1"),
                "action should have toggled main's hidden set off"
            );
        });
    }

    #[gpui::test]
    fn apply_set_project_show_in_overview_unknown_project_errs(
        cx: &mut gpui::TestAppContext,
    ) {
        let workspace = cx.new(|_cx| Workspace::new(make_workspace_data()));
        workspace.update(cx, |ws: &mut Workspace, cx| {
            let mut fm = crate::workspace::focus::FocusManager::new();
            let result =
                apply_set_project_show_in_overview(ws, &mut fm, WindowId::Main, "missing", true, cx);
            assert!(matches!(result, ActionResult::Err(_)));
        });
    }

    #[gpui::test]
    fn apply_set_project_show_in_overview_targets_extra_when_window_id_extra(
        cx: &mut gpui::TestAppContext,
    ) {
        // PRD user story 27 / slice 05 cri 13: a remote-bridge action issued
        // while an extra window has OS focus must mutate that extra's
        // per-window hidden set, not main's. The extra starts with p1 hidden
        // (mirrors the spawn snapshot semantic where every project is hidden
        // in a fresh extra); the action says `show: true`, so the helper
        // toggles only on the extra. Main's hidden set must remain empty.
        let mut data = make_workspace_data();
        data.projects = vec![make_project("p1")];
        data.project_order = vec!["p1".to_string()];
        let mut extra = WindowState::default();
        extra.hidden_project_ids.insert("p1".to_string());
        let extra_id = extra.id;
        data.extra_windows = vec![extra];
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            let mut fm = crate::workspace::focus::FocusManager::new();
            let result = apply_set_project_show_in_overview(
                ws,
                &mut fm,
                WindowId::Extra(extra_id),
                "p1",
                true,
                cx,
            );
            assert!(matches!(result, ActionResult::Ok(_)));
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            let extra_state = ws
                .data()
                .window(WindowId::Extra(extra_id))
                .expect("extra still tracked");
            assert!(
                !extra_state.hidden_project_ids.contains("p1"),
                "action should have toggled the targeted extra's hidden set off",
            );
            assert!(
                ws.data().main_window.hidden_project_ids.is_empty(),
                "main's hidden set must stay untouched when routing to an extra",
            );
        });
    }

    #[gpui::test]
    fn apply_set_project_show_in_overview_extra_hide_inserts_only_on_extra(
        cx: &mut gpui::TestAppContext,
    ) {
        // The reverse direction: project visible in both main + extra,
        // action says `show: false` against the extra. Extra's hidden set
        // gains p1; main stays unchanged.
        let mut data = make_workspace_data();
        data.projects = vec![make_project("p1")];
        data.project_order = vec!["p1".to_string()];
        let extra = WindowState::default();
        let extra_id = extra.id;
        data.extra_windows = vec![extra];
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            let mut fm = crate::workspace::focus::FocusManager::new();
            let result = apply_set_project_show_in_overview(
                ws,
                &mut fm,
                WindowId::Extra(extra_id),
                "p1",
                false,
                cx,
            );
            assert!(matches!(result, ActionResult::Ok(_)));
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            let extra_state = ws.data().window(WindowId::Extra(extra_id)).unwrap();
            assert!(extra_state.hidden_project_ids.contains("p1"));
            assert!(ws.data().main_window.hidden_project_ids.is_empty());
        });
    }
}
