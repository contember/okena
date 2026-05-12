//! Unit tests for `SidebarProjectInfo::from_project`.
//!
//! Lives in its own file because adding `#[test]` to `sidebar/mod.rs` (which
//! contains very deeply nested GPUI builder chains) trips a rustc/syn
//! recursion limit during attribute expansion. A separate module avoids it.

use super::SidebarProjectInfo;
use okena_core::theme::FolderColor;
use okena_workspace::settings::HooksConfig;
use okena_workspace::state::{LayoutNode, ProjectData, WindowId, WindowState, Workspace, WorkspaceData};
use std::collections::HashMap;

fn make_project(id: &str) -> ProjectData {
    ProjectData {
        id: id.to_string(),
        name: format!("Project {}", id),
        path: "/tmp/test".to_string(),
        layout: Some(LayoutNode::new_terminal()),
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
    }
}

fn make_workspace(hidden: &[&str]) -> Workspace {
    let mut data = WorkspaceData {
        version: 1,
        projects: vec![],
        project_order: vec![],
        service_panel_heights: HashMap::new(),
        hook_panel_heights: HashMap::new(),
        folders: vec![],
        main_window: WindowState::default(),
        extra_windows: Vec::new(),
    };
    for id in hidden {
        data.main_window.hidden_project_ids.insert(id.to_string());
    }
    Workspace::new(data)
}

#[test]
fn from_project_show_in_overview_reads_from_workspace_hidden_set() {
    // Per-window viewport model: SidebarProjectInfo.show_in_overview is
    // derived from workspace.is_project_hidden(...) (reading
    // main_window.hidden_project_ids -- the source of truth).
    let workspace = make_workspace(&["p_hidden"]);

    let p_hidden = make_project("p_hidden");
    let p_visible = make_project("p_visible");

    let info_hidden = SidebarProjectInfo::from_project(&p_hidden, &workspace, WindowId::Main);
    assert!(
        !info_hidden.show_in_overview,
        "project listed in main_window.hidden_project_ids must project as not-visible"
    );

    let info_visible = SidebarProjectInfo::from_project(&p_visible, &workspace, WindowId::Main);
    assert!(
        info_visible.show_in_overview,
        "project absent from hidden set must project as visible (legacy field is ignored)"
    );
}
