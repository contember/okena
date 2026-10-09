//! Unit tests for `SidebarProjectInfo::from_project`.
//!
//! Lives in its own file because adding `#[test]` to `sidebar/mod.rs` (which
//! contains very deeply nested GPUI builder chains) trips a rustc/syn
//! recursion limit during attribute expansion. A separate module avoids it.

use super::SidebarProjectInfo;
use okena_core::theme::FolderColor;
use okena_workspace::settings::HooksConfig;
use okena_workspace::state::{
    LayoutNode, ProjectData, WindowId, WindowState, Workspace, WorkspaceData,
};
use std::collections::HashMap;

fn make_project(id: &str) -> ProjectData {
    ProjectData {
        id: id.to_string(),
        name: format!("Project {}", id),
        path: "/tmp/test".to_string(),
        layout: Some(LayoutNode::new_terminal()),
        terminal_names: HashMap::new(),
        status: None,
        hidden_terminals: HashMap::new(),
        worktree_info: None,
        worktree_ids: Vec::new(),
        folder_color: FolderColor::default(),
        hooks: HooksConfig::default(),
        connection_id: None,
        service_terminals: HashMap::new(),
        agent_sessions: Default::default(),
        default_shell: None,
        hook_terminals: HashMap::new(),
        pinned: false,
        last_activity_at: None,
        is_creating: false,
        is_closing: false,
        creating_progress: None,
    }
}

fn make_workspace(hidden: &[&str]) -> Workspace {
    let mut data = WorkspaceData {
        agent_session_history: Default::default(),
        dismissed_worktree_paths: Vec::new(),
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

fn make_worktree(id: &str, parent: &str, color_override: Option<FolderColor>) -> ProjectData {
    let mut wt = make_project(id);
    wt.worktree_info = Some(okena_workspace::state::WorktreeMetadata {
        parent_project_id: parent.to_string(),
        color_override,
        main_repo_path: String::new(),
        worktree_path: String::new(),
        branch_name: String::new(),
    });
    wt
}

#[test]
fn from_project_resolves_worktree_colour_through_override_then_parent() {
    // The terminal area and the project column colour a worktree by its own
    // override, else by its parent. The sidebar used to copy the parent's
    // colour over every child, so a recoloured worktree looked unchanged
    // in the sidebar while its panes already showed the new colour.
    let mut parent = make_project("main");
    parent.folder_color = FolderColor::Blue;
    parent.worktree_ids = vec!["wt-plain".to_string(), "wt-red".to_string()];
    let plain = make_worktree("wt-plain", "main", None);
    let red = make_worktree("wt-red", "main", Some(FolderColor::Red));

    let workspace = Workspace::new(WorkspaceData {
        agent_session_history: Default::default(),
        dismissed_worktree_paths: Vec::new(),
        version: 1,
        projects: vec![parent, plain, red],
        project_order: vec!["main".to_string()],
        service_panel_heights: HashMap::new(),
        hook_panel_heights: HashMap::new(),
        folders: vec![],
        main_window: WindowState::default(),
        extra_windows: Vec::new(),
    });

    let plain_info = SidebarProjectInfo::from_project(
        workspace.project("wt-plain").unwrap(),
        &workspace,
        WindowId::Main,
    );
    let red_info = SidebarProjectInfo::from_project(
        workspace.project("wt-red").unwrap(),
        &workspace,
        WindowId::Main,
    );
    assert_eq!(plain_info.folder_color, FolderColor::Blue);
    assert_eq!(red_info.folder_color, FolderColor::Red);
}
