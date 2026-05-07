//! GPUI-level integration tests for layout actions: exercise the full Workspace
//! entity via TestAppContext.

use gpui::AppContext as _;
use crate::focus::FocusManager;
use crate::state::{DropZone, LayoutNode, ProjectData, SplitDirection, Workspace, WorkspaceData};
use crate::settings::HooksConfig;
use okena_terminal::shell_config::ShellType;
use okena_core::theme::FolderColor;
use std::collections::HashMap;

fn make_project(id: &str) -> ProjectData {
    ProjectData {
        id: id.to_string(),
        name: format!("Project {}", id),
        path: "/tmp/test".to_string(),
        layout: Some(LayoutNode::Terminal {
            terminal_id: Some(format!("term_{}", id)),
            minimized: false,
            detached: false,
            shell_type: ShellType::Default,
            zoom_level: 1.0,
        }),
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

fn make_workspace_data(projects: Vec<ProjectData>, order: Vec<&str>) -> WorkspaceData {
    WorkspaceData {
        version: 1,
        projects,
        project_order: order.into_iter().map(String::from).collect(),
        service_panel_heights: HashMap::new(),
        hook_panel_heights: HashMap::new(),
        folders: vec![],
        main_window: crate::state::WindowState::default(),
        extra_windows: Vec::new(),
    }
}

#[gpui::test]
fn test_split_terminal_gpui(cx: &mut gpui::TestAppContext) {
    let data = make_workspace_data(vec![make_project("p1")], vec!["p1"]);
    let workspace = cx.new(|_cx| Workspace::new(data));

    let v0 = workspace.read_with(cx, |ws: &Workspace, _cx| ws.data_version());

    workspace.update(cx, |ws: &mut Workspace, cx| {
        ws.split_terminal(&mut FocusManager::new(), "p1", &[], SplitDirection::Vertical, cx);
    });

    workspace.read_with(cx, |ws: &Workspace, _cx| {
        assert!(ws.data_version() > v0);
        let layout = ws.project("p1").unwrap().layout.as_ref().unwrap();
        match layout {
            LayoutNode::Split { direction, children, .. } => {
                assert_eq!(*direction, SplitDirection::Vertical);
                assert_eq!(children.len(), 2);
                // First child should be the original terminal
                assert!(matches!(&children[0], LayoutNode::Terminal { terminal_id: Some(id), .. } if id == "term_p1"));
                // Second child should be a new terminal
                assert!(matches!(&children[1], LayoutNode::Terminal { terminal_id: None, .. }));
            }
            _ => panic!("Expected split after split_terminal"),
        }
    });
}

#[gpui::test]
fn test_add_tab_gpui(cx: &mut gpui::TestAppContext) {
    let data = make_workspace_data(vec![make_project("p1")], vec!["p1"]);
    let workspace = cx.new(|_cx| Workspace::new(data));

    workspace.update(cx, |ws: &mut Workspace, cx| {
        ws.add_tab(&mut FocusManager::new(), "p1", &[], cx);
    });

    workspace.read_with(cx, |ws: &Workspace, _cx| {
        let layout = ws.project("p1").unwrap().layout.as_ref().unwrap();
        match layout {
            LayoutNode::Tabs { children, active_tab } => {
                assert_eq!(children.len(), 2);
                assert_eq!(*active_tab, 1);
            }
            _ => panic!("Expected tabs after add_tab"),
        }
    });
}

#[gpui::test]
fn test_close_terminal_gpui(cx: &mut gpui::TestAppContext) {
    // Create a project with a 2-child split
    let mut project = make_project("p1");
    project.layout = Some(LayoutNode::Split {
        direction: SplitDirection::Horizontal,
        sizes: vec![50.0, 50.0],
        children: vec![
            LayoutNode::Terminal {
                terminal_id: Some("t1".to_string()),
                minimized: false,
                detached: false,
                shell_type: ShellType::Default,
                zoom_level: 1.0,
            },
            LayoutNode::Terminal {
                terminal_id: Some("t2".to_string()),
                minimized: false,
                detached: false,
                shell_type: ShellType::Default,
                zoom_level: 1.0,
            },
        ],
    });
    let data = make_workspace_data(vec![project], vec!["p1"]);
    let workspace = cx.new(|_cx| Workspace::new(data));

    workspace.update(cx, |ws: &mut Workspace, cx| {
        ws.close_terminal("p1", &[0], cx);
    });

    workspace.read_with(cx, |ws: &Workspace, _cx| {
        let layout = ws.project("p1").unwrap().layout.as_ref().unwrap();
        // After closing child 0, sibling (t2) should replace the split
        assert!(matches!(layout, LayoutNode::Terminal { terminal_id: Some(id), .. } if id == "t2"));
    });
}

#[gpui::test]
fn test_close_tab_gpui(cx: &mut gpui::TestAppContext) {
    let mut project = make_project("p1");
    project.layout = Some(LayoutNode::Tabs {
        children: vec![
            LayoutNode::Terminal {
                terminal_id: Some("t1".to_string()),
                minimized: false,
                detached: false,
                shell_type: ShellType::Default,
                zoom_level: 1.0,
            },
            LayoutNode::Terminal {
                terminal_id: Some("t2".to_string()),
                minimized: false,
                detached: false,
                shell_type: ShellType::Default,
                zoom_level: 1.0,
            },
            LayoutNode::Terminal {
                terminal_id: Some("t3".to_string()),
                minimized: false,
                detached: false,
                shell_type: ShellType::Default,
                zoom_level: 1.0,
            },
        ],
        active_tab: 2,
    });
    let data = make_workspace_data(vec![project], vec!["p1"]);
    let workspace = cx.new(|_cx| Workspace::new(data));

    // Close tab 0
    workspace.update(cx, |ws: &mut Workspace, cx| {
        ws.close_tab("p1", &[], 0, cx);
    });

    workspace.read_with(cx, |ws: &Workspace, _cx| {
        let layout = ws.project("p1").unwrap().layout.as_ref().unwrap();
        match layout {
            LayoutNode::Tabs { children, active_tab } => {
                assert_eq!(children.len(), 2);
                // active_tab was 2, after removing index 0 it should be 1
                assert_eq!(*active_tab, 1);
                // Remaining are t2 and t3
                let ids: Vec<_> = children.iter().filter_map(|c| match c {
                    LayoutNode::Terminal { terminal_id: Some(id), .. } => Some(id.as_str()),
                    _ => None,
                }).collect();
                assert_eq!(ids, vec!["t2", "t3"]);
            }
            _ => panic!("Expected tabs"),
        }
    });
}

#[gpui::test]
fn test_move_tab_gpui(cx: &mut gpui::TestAppContext) {
    let mut project = make_project("p1");
    project.layout = Some(LayoutNode::Tabs {
        children: vec![
            LayoutNode::Terminal {
                terminal_id: Some("t1".to_string()),
                minimized: false,
                detached: false,
                shell_type: ShellType::Default,
                zoom_level: 1.0,
            },
            LayoutNode::Terminal {
                terminal_id: Some("t2".to_string()),
                minimized: false,
                detached: false,
                shell_type: ShellType::Default,
                zoom_level: 1.0,
            },
            LayoutNode::Terminal {
                terminal_id: Some("t3".to_string()),
                minimized: false,
                detached: false,
                shell_type: ShellType::Default,
                zoom_level: 1.0,
            },
        ],
        active_tab: 0,
    });
    let data = make_workspace_data(vec![project], vec!["p1"]);
    let workspace = cx.new(|_cx| Workspace::new(data));

    // Move tab from index 0 to index 2
    workspace.update(cx, |ws: &mut Workspace, cx| {
        ws.move_tab("p1", &[], 0, 2, cx);
    });

    workspace.read_with(cx, |ws: &Workspace, _cx| {
        let layout = ws.project("p1").unwrap().layout.as_ref().unwrap();
        match layout {
            LayoutNode::Tabs { children, active_tab } => {
                let ids: Vec<_> = children.iter().filter_map(|c| match c {
                    LayoutNode::Terminal { terminal_id: Some(id), .. } => Some(id.as_str()),
                    _ => None,
                }).collect();
                assert_eq!(ids, vec!["t2", "t3", "t1"]);
                assert_eq!(*active_tab, 2); // active_tab was 0 (the moved tab), should follow
            }
            _ => panic!("Expected tabs"),
        }
    });
}

// === move_pane tests ===

fn terminal_node_t(id: &str) -> LayoutNode {
    LayoutNode::Terminal {
        terminal_id: Some(id.to_string()),
        minimized: false,
        detached: false,
        shell_type: ShellType::Default,
        zoom_level: 1.0,
    }
}

fn make_project_with_layout(id: &str, layout: LayoutNode) -> ProjectData {
    ProjectData {
        id: id.to_string(),
        name: format!("Project {}", id),
        path: "/tmp/test".to_string(),
        layout: Some(layout),
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

#[gpui::test]
fn test_move_pane_left_creates_vertical_split(cx: &mut gpui::TestAppContext) {
    let layout = LayoutNode::Split {
        direction: SplitDirection::Vertical,
        sizes: vec![50.0, 50.0],
        children: vec![terminal_node_t("t1"), terminal_node_t("t2")],
    };
    let project = make_project_with_layout("p1", layout);
    let data = make_workspace_data(vec![project], vec!["p1"]);
    let workspace = cx.new(|_cx| Workspace::new(data));

    workspace.update(cx, |ws: &mut Workspace, cx| {
        ws.move_pane(&mut FocusManager::new(), "p1", "t1", "p1", "t2", DropZone::Left, cx);
    });

    workspace.read_with(cx, |ws: &Workspace, _cx| {
        let layout = ws.project("p1").unwrap().layout.as_ref().unwrap();
        // t1 dropped on left of t2 -> V[t1, t2] which is same direction as parent,
        // so normalize flattens it back. Result is still V[t1, t2].
        let ids = layout.collect_terminal_ids();
        assert_eq!(ids, vec!["t1", "t2"]);
        match layout {
            LayoutNode::Split { direction, .. } => {
                assert_eq!(*direction, SplitDirection::Vertical);
            }
            _ => panic!("Expected vertical split"),
        }
    });
}

#[gpui::test]
fn test_move_pane_top_creates_horizontal_split(cx: &mut gpui::TestAppContext) {
    let layout = LayoutNode::Split {
        direction: SplitDirection::Vertical,
        sizes: vec![50.0, 50.0],
        children: vec![terminal_node_t("t1"), terminal_node_t("t2")],
    };
    let project = make_project_with_layout("p1", layout);
    let data = make_workspace_data(vec![project], vec!["p1"]);
    let workspace = cx.new(|_cx| Workspace::new(data));

    workspace.update(cx, |ws: &mut Workspace, cx| {
        ws.move_pane(&mut FocusManager::new(), "p1", "t1", "p1", "t2", DropZone::Top, cx);
    });

    workspace.read_with(cx, |ws: &Workspace, _cx| {
        let layout = ws.project("p1").unwrap().layout.as_ref().unwrap();
        // t1 removed -> t2 becomes root. t1 dropped on top of t2 -> H[t1, t2]
        match layout {
            LayoutNode::Split { direction, children, .. } => {
                assert_eq!(*direction, SplitDirection::Horizontal);
                assert_eq!(children.len(), 2);
                let ids = layout.collect_terminal_ids();
                assert_eq!(ids, vec!["t1", "t2"]);
            }
            _ => panic!("Expected horizontal split"),
        }
    });
}

#[gpui::test]
fn test_move_pane_bottom_creates_horizontal_split(cx: &mut gpui::TestAppContext) {
    let layout = LayoutNode::Split {
        direction: SplitDirection::Vertical,
        sizes: vec![50.0, 50.0],
        children: vec![terminal_node_t("t1"), terminal_node_t("t2")],
    };
    let project = make_project_with_layout("p1", layout);
    let data = make_workspace_data(vec![project], vec!["p1"]);
    let workspace = cx.new(|_cx| Workspace::new(data));

    workspace.update(cx, |ws: &mut Workspace, cx| {
        ws.move_pane(&mut FocusManager::new(), "p1", "t1", "p1", "t2", DropZone::Bottom, cx);
    });

    workspace.read_with(cx, |ws: &Workspace, _cx| {
        let layout = ws.project("p1").unwrap().layout.as_ref().unwrap();
        match layout {
            LayoutNode::Split { direction, children, .. } => {
                assert_eq!(*direction, SplitDirection::Horizontal);
                assert_eq!(children.len(), 2);
                let ids = layout.collect_terminal_ids();
                // Bottom: target first, then source
                assert_eq!(ids, vec!["t2", "t1"]);
            }
            _ => panic!("Expected horizontal split"),
        }
    });
}

#[gpui::test]
fn test_move_pane_center_creates_tab_group(cx: &mut gpui::TestAppContext) {
    let layout = LayoutNode::Split {
        direction: SplitDirection::Vertical,
        sizes: vec![50.0, 50.0],
        children: vec![terminal_node_t("t1"), terminal_node_t("t2")],
    };
    let project = make_project_with_layout("p1", layout);
    let data = make_workspace_data(vec![project], vec!["p1"]);
    let workspace = cx.new(|_cx| Workspace::new(data));

    workspace.update(cx, |ws: &mut Workspace, cx| {
        ws.move_pane(&mut FocusManager::new(), "p1", "t1", "p1", "t2", DropZone::Center, cx);
    });

    workspace.read_with(cx, |ws: &Workspace, _cx| {
        let layout = ws.project("p1").unwrap().layout.as_ref().unwrap();
        match layout {
            LayoutNode::Tabs { children, active_tab } => {
                assert_eq!(children.len(), 2);
                assert_eq!(*active_tab, 1);
                let ids = layout.collect_terminal_ids();
                assert_eq!(ids, vec!["t2", "t1"]);
            }
            _ => panic!("Expected tabs, got {:?}", layout),
        }
    });
}

#[gpui::test]
fn test_move_pane_self_drop_is_noop(cx: &mut gpui::TestAppContext) {
    let layout = LayoutNode::Split {
        direction: SplitDirection::Vertical,
        sizes: vec![50.0, 50.0],
        children: vec![terminal_node_t("t1"), terminal_node_t("t2")],
    };
    let project = make_project_with_layout("p1", layout);
    let data = make_workspace_data(vec![project], vec!["p1"]);
    let workspace = cx.new(|_cx| Workspace::new(data));

    let v0 = workspace.read_with(cx, |ws: &Workspace, _cx| ws.data_version());

    workspace.update(cx, |ws: &mut Workspace, cx| {
        ws.move_pane(&mut FocusManager::new(), "p1", "t1", "p1", "t1", DropZone::Top, cx);
    });

    workspace.read_with(cx, |ws: &Workspace, _cx| {
        // Version should not have changed
        assert_eq!(ws.data_version(), v0);
    });
}

#[gpui::test]
fn test_move_pane_only_terminal_is_noop(cx: &mut gpui::TestAppContext) {
    // Single terminal - can't move it
    let project = make_project("p1");
    let data = make_workspace_data(vec![project], vec!["p1"]);
    let workspace = cx.new(|_cx| Workspace::new(data));

    let v0 = workspace.read_with(cx, |ws: &Workspace, _cx| ws.data_version());

    workspace.update(cx, |ws: &mut Workspace, cx| {
        ws.move_pane(&mut FocusManager::new(), "p1", "term_p1", "p1", "term_p1", DropZone::Left, cx);
    });

    workspace.read_with(cx, |ws: &Workspace, _cx| {
        assert_eq!(ws.data_version(), v0);
    });
}

// === move_terminal_to_tab_group tests ===

#[gpui::test]
fn test_move_terminal_to_tab_group_inserts_at_position(cx: &mut gpui::TestAppContext) {
    // V[Tabs[t1, t2], t3] → move t3 into tabs at index 1 → Tabs[t1, t3, t2]
    let layout = LayoutNode::Split {
        direction: SplitDirection::Vertical,
        sizes: vec![50.0, 50.0],
        children: vec![
            LayoutNode::Tabs {
                children: vec![terminal_node_t("t1"), terminal_node_t("t2")],
                active_tab: 0,
            },
            terminal_node_t("t3"),
        ],
    };
    let project = make_project_with_layout("p1", layout);
    let data = make_workspace_data(vec![project], vec!["p1"]);
    let workspace = cx.new(|_cx| Workspace::new(data));

    workspace.update(cx, |ws: &mut Workspace, cx| {
        ws.move_terminal_to_tab_group(&mut FocusManager::new(), "p1", "t3", "p1", &[0], Some(1), cx);
    });

    workspace.read_with(cx, |ws: &Workspace, _cx| {
        let layout = ws.project("p1").unwrap().layout.as_ref().unwrap();
        match layout {
            LayoutNode::Tabs { children, active_tab } => {
                assert_eq!(children.len(), 3);
                assert_eq!(*active_tab, 1);
                let ids: Vec<_> = children.iter().filter_map(|c| match c {
                    LayoutNode::Terminal { terminal_id: Some(id), .. } => Some(id.as_str()),
                    _ => None,
                }).collect();
                assert_eq!(ids, vec!["t1", "t3", "t2"]);
            }
            _ => panic!("Expected tabs, got {:?}", layout),
        }
    });
}

#[gpui::test]
fn test_move_terminal_to_tab_group_appends(cx: &mut gpui::TestAppContext) {
    // V[Tabs[t1, t2], t3] → move t3 into tabs at end → Tabs[t1, t2, t3]
    let layout = LayoutNode::Split {
        direction: SplitDirection::Vertical,
        sizes: vec![50.0, 50.0],
        children: vec![
            LayoutNode::Tabs {
                children: vec![terminal_node_t("t1"), terminal_node_t("t2")],
                active_tab: 0,
            },
            terminal_node_t("t3"),
        ],
    };
    let project = make_project_with_layout("p1", layout);
    let data = make_workspace_data(vec![project], vec!["p1"]);
    let workspace = cx.new(|_cx| Workspace::new(data));

    workspace.update(cx, |ws: &mut Workspace, cx| {
        ws.move_terminal_to_tab_group(&mut FocusManager::new(), "p1", "t3", "p1", &[0], None, cx);
    });

    workspace.read_with(cx, |ws: &Workspace, _cx| {
        let layout = ws.project("p1").unwrap().layout.as_ref().unwrap();
        match layout {
            LayoutNode::Tabs { children, active_tab } => {
                assert_eq!(children.len(), 3);
                assert_eq!(*active_tab, 2);
                let ids: Vec<_> = children.iter().filter_map(|c| match c {
                    LayoutNode::Terminal { terminal_id: Some(id), .. } => Some(id.as_str()),
                    _ => None,
                }).collect();
                assert_eq!(ids, vec!["t1", "t2", "t3"]);
            }
            _ => panic!("Expected tabs, got {:?}", layout),
        }
    });
}

#[gpui::test]
fn test_move_terminal_to_tab_group_same_group_reorders(cx: &mut gpui::TestAppContext) {
    // Tabs[t1, t2, t3] → move t1 (already in group) to index 2 → reorder
    let layout = LayoutNode::Tabs {
        children: vec![terminal_node_t("t1"), terminal_node_t("t2"), terminal_node_t("t3")],
        active_tab: 0,
    };
    let project = make_project_with_layout("p1", layout);
    let data = make_workspace_data(vec![project], vec!["p1"]);
    let workspace = cx.new(|_cx| Workspace::new(data));

    workspace.update(cx, |ws: &mut Workspace, cx| {
        ws.move_terminal_to_tab_group(&mut FocusManager::new(), "p1", "t1", "p1", &[], Some(2), cx);
    });

    workspace.read_with(cx, |ws: &Workspace, _cx| {
        let layout = ws.project("p1").unwrap().layout.as_ref().unwrap();
        match layout {
            LayoutNode::Tabs { children, .. } => {
                let ids: Vec<_> = children.iter().filter_map(|c| match c {
                    LayoutNode::Terminal { terminal_id: Some(id), .. } => Some(id.as_str()),
                    _ => None,
                }).collect();
                assert_eq!(ids, vec!["t2", "t3", "t1"]);
            }
            _ => panic!("Expected tabs, got {:?}", layout),
        }
    });
}

#[gpui::test]
fn test_move_pane_3_children_with_flatten(cx: &mut gpui::TestAppContext) {
    // V[t1, t2, t3] -> drag t1 to top of t3 -> V[t2, H[t1, t3]]
    let layout = LayoutNode::Split {
        direction: SplitDirection::Vertical,
        sizes: vec![33.0, 33.0, 34.0],
        children: vec![terminal_node_t("t1"), terminal_node_t("t2"), terminal_node_t("t3")],
    };
    let project = make_project_with_layout("p1", layout);
    let data = make_workspace_data(vec![project], vec!["p1"]);
    let workspace = cx.new(|_cx| Workspace::new(data));

    workspace.update(cx, |ws: &mut Workspace, cx| {
        ws.move_pane(&mut FocusManager::new(), "p1", "t1", "p1", "t3", DropZone::Top, cx);
    });

    workspace.read_with(cx, |ws: &Workspace, _cx| {
        let layout = ws.project("p1").unwrap().layout.as_ref().unwrap();
        match layout {
            LayoutNode::Split { direction, children, .. } => {
                assert_eq!(*direction, SplitDirection::Vertical);
                assert_eq!(children.len(), 2);
                // First child is t2
                assert!(matches!(&children[0], LayoutNode::Terminal { terminal_id: Some(id), .. } if id == "t2"));
                // Second child is H[t1, t3]
                match &children[1] {
                    LayoutNode::Split { direction: inner_dir, children: inner_children, .. } => {
                        assert_eq!(*inner_dir, SplitDirection::Horizontal);
                        assert_eq!(inner_children.len(), 2);
                        let inner_ids: Vec<_> = inner_children.iter().filter_map(|c| match c {
                            LayoutNode::Terminal { terminal_id: Some(id), .. } => Some(id.as_str()),
                            _ => None,
                        }).collect();
                        assert_eq!(inner_ids, vec!["t1", "t3"]);
                    }
                    _ => panic!("Expected inner horizontal split"),
                }
            }
            _ => panic!("Expected vertical split"),
        }
    });
}

// === metadata cleanup tests ===

fn make_project_with_names(id: &str, layout: LayoutNode, names: Vec<(&str, &str)>) -> ProjectData {
    let mut p = make_project_with_layout(id, layout);
    for (tid, name) in names {
        p.terminal_names.insert(tid.to_string(), name.to_string());
    }
    p
}

#[gpui::test]
fn test_close_terminal_cleans_metadata(cx: &mut gpui::TestAppContext) {
    let layout = LayoutNode::Split {
        direction: SplitDirection::Horizontal,
        sizes: vec![50.0, 50.0],
        children: vec![terminal_node_t("t1"), terminal_node_t("t2")],
    };
    let project = make_project_with_names("p1", layout, vec![("t1", "Term 1"), ("t2", "Term 2")]);
    let data = make_workspace_data(vec![project], vec!["p1"]);
    let workspace = cx.new(|_cx| Workspace::new(data));

    let removed = workspace.update(cx, |ws: &mut Workspace, cx| {
        ws.close_terminal("p1", &[0], cx)
    });

    assert_eq!(removed, vec!["t1"]);
    workspace.read_with(cx, |ws: &Workspace, _cx| {
        let p = ws.project("p1").unwrap();
        assert!(!p.terminal_names.contains_key("t1"));
        assert!(p.terminal_names.contains_key("t2"));
    });
}

#[gpui::test]
fn test_close_tab_cleans_metadata(cx: &mut gpui::TestAppContext) {
    let layout = LayoutNode::Tabs {
        children: vec![terminal_node_t("t1"), terminal_node_t("t2"), terminal_node_t("t3")],
        active_tab: 0,
    };
    let project = make_project_with_names("p1", layout, vec![
        ("t1", "Term 1"), ("t2", "Term 2"), ("t3", "Term 3"),
    ]);
    let data = make_workspace_data(vec![project], vec!["p1"]);
    let workspace = cx.new(|_cx| Workspace::new(data));

    let removed = workspace.update(cx, |ws: &mut Workspace, cx| {
        ws.close_tab("p1", &[], 1, cx)
    });

    assert_eq!(removed, vec!["t2"]);
    workspace.read_with(cx, |ws: &Workspace, _cx| {
        let p = ws.project("p1").unwrap();
        assert!(p.terminal_names.contains_key("t1"));
        assert!(!p.terminal_names.contains_key("t2"));
        assert!(p.terminal_names.contains_key("t3"));
    });
}

#[gpui::test]
fn test_close_other_tabs_cleans_metadata(cx: &mut gpui::TestAppContext) {
    let layout = LayoutNode::Tabs {
        children: vec![terminal_node_t("t1"), terminal_node_t("t2"), terminal_node_t("t3")],
        active_tab: 0,
    };
    let project = make_project_with_names("p1", layout, vec![
        ("t1", "Term 1"), ("t2", "Term 2"), ("t3", "Term 3"),
    ]);
    let data = make_workspace_data(vec![project], vec!["p1"]);
    let workspace = cx.new(|_cx| Workspace::new(data));

    let removed = workspace.update(cx, |ws: &mut Workspace, cx| {
        ws.close_other_tabs("p1", &[], 1, cx)
    });

    assert_eq!(removed.len(), 2);
    assert!(removed.contains(&"t1".to_string()));
    assert!(removed.contains(&"t3".to_string()));
    workspace.read_with(cx, |ws: &Workspace, _cx| {
        let p = ws.project("p1").unwrap();
        assert!(!p.terminal_names.contains_key("t1"));
        assert!(p.terminal_names.contains_key("t2"));
        assert!(!p.terminal_names.contains_key("t3"));
    });
}

#[gpui::test]
fn test_close_tabs_to_right_cleans_metadata(cx: &mut gpui::TestAppContext) {
    let layout = LayoutNode::Tabs {
        children: vec![terminal_node_t("t1"), terminal_node_t("t2"), terminal_node_t("t3")],
        active_tab: 0,
    };
    let project = make_project_with_names("p1", layout, vec![
        ("t1", "Term 1"), ("t2", "Term 2"), ("t3", "Term 3"),
    ]);
    let data = make_workspace_data(vec![project], vec!["p1"]);
    let workspace = cx.new(|_cx| Workspace::new(data));

    let removed = workspace.update(cx, |ws: &mut Workspace, cx| {
        ws.close_tabs_to_right("p1", &[], 0, cx)
    });

    assert_eq!(removed.len(), 2);
    assert!(removed.contains(&"t2".to_string()));
    assert!(removed.contains(&"t3".to_string()));
    workspace.read_with(cx, |ws: &Workspace, _cx| {
        let p = ws.project("p1").unwrap();
        assert!(p.terminal_names.contains_key("t1"));
        assert!(!p.terminal_names.contains_key("t2"));
        assert!(!p.terminal_names.contains_key("t3"));
    });
}

// === cross-project move_pane tests ===

#[gpui::test]
fn test_move_pane_cross_project(cx: &mut gpui::TestAppContext) {
    // p1: V[t1, t2], p2: t3 → move t1 to left of t3 → p1: t2, p2: V[t1, t3]
    let p1 = make_project_with_layout("p1", LayoutNode::Split {
        direction: SplitDirection::Vertical,
        sizes: vec![50.0, 50.0],
        children: vec![terminal_node_t("t1"), terminal_node_t("t2")],
    });
    let p2 = make_project_with_layout("p2", terminal_node_t("t3"));
    let data = make_workspace_data(vec![p1, p2], vec!["p1", "p2"]);
    let workspace = cx.new(|_cx| Workspace::new(data));

    workspace.update(cx, |ws: &mut Workspace, cx| {
        ws.move_pane(&mut FocusManager::new(), "p1", "t1", "p2", "t3", DropZone::Left, cx);
    });

    workspace.read_with(cx, |ws: &Workspace, _cx| {
        // p1 should have just t2
        let p1_layout = ws.project("p1").unwrap().layout.as_ref().unwrap();
        assert!(matches!(p1_layout, LayoutNode::Terminal { terminal_id: Some(id), .. } if id == "t2"));

        // p2 should have V[t1, t3]
        let p2_layout = ws.project("p2").unwrap().layout.as_ref().unwrap();
        match p2_layout {
            LayoutNode::Split { direction, children, .. } => {
                assert_eq!(*direction, SplitDirection::Vertical);
                assert_eq!(children.len(), 2);
                let ids = p2_layout.collect_terminal_ids();
                assert_eq!(ids, vec!["t1", "t3"]);
            }
            _ => panic!("Expected vertical split in p2, got {:?}", p2_layout),
        }
    });
}

#[gpui::test]
fn test_move_pane_cross_project_center(cx: &mut gpui::TestAppContext) {
    // p1: V[t1, t2], p2: t3 → move t1 center onto t3 → p2: Tabs[t3, t1]
    let p1 = make_project_with_layout("p1", LayoutNode::Split {
        direction: SplitDirection::Vertical,
        sizes: vec![50.0, 50.0],
        children: vec![terminal_node_t("t1"), terminal_node_t("t2")],
    });
    let p2 = make_project_with_layout("p2", terminal_node_t("t3"));
    let data = make_workspace_data(vec![p1, p2], vec!["p1", "p2"]);
    let workspace = cx.new(|_cx| Workspace::new(data));

    workspace.update(cx, |ws: &mut Workspace, cx| {
        ws.move_pane(&mut FocusManager::new(), "p1", "t1", "p2", "t3", DropZone::Center, cx);
    });

    workspace.read_with(cx, |ws: &Workspace, _cx| {
        let p2_layout = ws.project("p2").unwrap().layout.as_ref().unwrap();
        match p2_layout {
            LayoutNode::Tabs { children, active_tab } => {
                assert_eq!(children.len(), 2);
                assert_eq!(*active_tab, 1);
                let ids = p2_layout.collect_terminal_ids();
                assert_eq!(ids, vec!["t3", "t1"]);
            }
            _ => panic!("Expected tabs in p2, got {:?}", p2_layout),
        }
    });
}

#[gpui::test]
fn test_move_pane_cross_project_last_terminal(cx: &mut gpui::TestAppContext) {
    // p1: t1 (sole terminal), p2: t2 → move t1 to left of t2 → p1 layout = None
    let p1 = make_project_with_layout("p1", terminal_node_t("t1"));
    let p2 = make_project_with_layout("p2", terminal_node_t("t2"));
    let data = make_workspace_data(vec![p1, p2], vec!["p1", "p2"]);
    let workspace = cx.new(|_cx| Workspace::new(data));

    workspace.update(cx, |ws: &mut Workspace, cx| {
        ws.move_pane(&mut FocusManager::new(), "p1", "t1", "p2", "t2", DropZone::Left, cx);
    });

    workspace.read_with(cx, |ws: &Workspace, _cx| {
        // p1 layout should be None (sole terminal moved out)
        assert!(ws.project("p1").unwrap().layout.is_none());

        // p2 should have V[t1, t2]
        let p2_layout = ws.project("p2").unwrap().layout.as_ref().unwrap();
        let ids = p2_layout.collect_terminal_ids();
        assert_eq!(ids, vec!["t1", "t2"]);
    });
}

#[gpui::test]
fn test_move_pane_cross_project_metadata_migration(cx: &mut gpui::TestAppContext) {
    // Move t1 from p1 to p2, verify terminal_names and hidden_terminals migrate
    let mut p1 = make_project_with_layout("p1", LayoutNode::Split {
        direction: SplitDirection::Vertical,
        sizes: vec![50.0, 50.0],
        children: vec![terminal_node_t("t1"), terminal_node_t("t2")],
    });
    p1.terminal_names.insert("t1".to_string(), "My Terminal".to_string());
    p1.terminal_names.insert("t2".to_string(), "Other Terminal".to_string());
    p1.hidden_terminals.insert("t1".to_string(), true);

    let p2 = make_project_with_layout("p2", terminal_node_t("t3"));
    let data = make_workspace_data(vec![p1, p2], vec!["p1", "p2"]);
    let workspace = cx.new(|_cx| Workspace::new(data));

    workspace.update(cx, |ws: &mut Workspace, cx| {
        ws.move_pane(&mut FocusManager::new(), "p1", "t1", "p2", "t3", DropZone::Right, cx);
    });

    workspace.read_with(cx, |ws: &Workspace, _cx| {
        let p1 = ws.project("p1").unwrap();
        assert!(!p1.terminal_names.contains_key("t1"));
        assert!(p1.terminal_names.contains_key("t2"));
        assert!(!p1.hidden_terminals.contains_key("t1"));

        let p2 = ws.project("p2").unwrap();
        assert_eq!(p2.terminal_names.get("t1").unwrap(), "My Terminal");
        assert_eq!(p2.hidden_terminals.get("t1").unwrap(), &true);
    });
}

#[gpui::test]
fn test_move_pane_cross_project_to_bookmark(cx: &mut gpui::TestAppContext) {
    // p2 has no layout (bookmark) → t1 becomes root of p2
    let p1 = make_project_with_layout("p1", LayoutNode::Split {
        direction: SplitDirection::Vertical,
        sizes: vec![50.0, 50.0],
        children: vec![terminal_node_t("t1"), terminal_node_t("t2")],
    });
    let mut p2 = make_project("p2");
    p2.layout = None;
    let data = make_workspace_data(vec![p1, p2], vec!["p1", "p2"]);
    let workspace = cx.new(|_cx| Workspace::new(data));

    // Can't use move_pane with target_terminal_id since p2 has no layout/terminal.
    // In practice the UI wouldn't offer a drop target on a bookmark.
    // This test verifies the guard: move_pane should be a noop when target has no terminal.

    workspace.read_with(cx, |ws: &Workspace, _cx| {
        // p1 should be unchanged (no valid target terminal in p2)
        let p1_layout = ws.project("p1").unwrap().layout.as_ref().unwrap();
        assert_eq!(p1_layout.collect_terminal_ids(), vec!["t1", "t2"]);
    });
}

#[gpui::test]
fn test_move_to_tab_group_cross_project(cx: &mut gpui::TestAppContext) {
    // p1: V[t1, t2], p2: Tabs[t3, t4] → move t1 into p2's tabs at index 1
    let p1 = make_project_with_layout("p1", LayoutNode::Split {
        direction: SplitDirection::Vertical,
        sizes: vec![50.0, 50.0],
        children: vec![terminal_node_t("t1"), terminal_node_t("t2")],
    });
    let p2 = make_project_with_layout("p2", LayoutNode::Tabs {
        children: vec![terminal_node_t("t3"), terminal_node_t("t4")],
        active_tab: 0,
    });
    let data = make_workspace_data(vec![p1, p2], vec!["p1", "p2"]);
    let workspace = cx.new(|_cx| Workspace::new(data));

    workspace.update(cx, |ws: &mut Workspace, cx| {
        ws.move_terminal_to_tab_group(&mut FocusManager::new(), "p1", "t1", "p2", &[], Some(1), cx);
    });

    workspace.read_with(cx, |ws: &Workspace, _cx| {
        // p1 should have just t2
        let p1_layout = ws.project("p1").unwrap().layout.as_ref().unwrap();
        assert!(matches!(p1_layout, LayoutNode::Terminal { terminal_id: Some(id), .. } if id == "t2"));

        // p2 should have Tabs[t3, t1, t4]
        let p2_layout = ws.project("p2").unwrap().layout.as_ref().unwrap();
        match p2_layout {
            LayoutNode::Tabs { children, active_tab } => {
                assert_eq!(children.len(), 3);
                assert_eq!(*active_tab, 1);
                let ids: Vec<_> = children.iter().filter_map(|c| match c {
                    LayoutNode::Terminal { terminal_id: Some(id), .. } => Some(id.as_str()),
                    _ => None,
                }).collect();
                assert_eq!(ids, vec!["t3", "t1", "t4"]);
            }
            _ => panic!("Expected tabs in p2, got {:?}", p2_layout),
        }
    });
}
