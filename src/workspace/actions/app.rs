//! App pane workspace action tests
//!
//! The action methods live in the okena-workspace crate (actions/app.rs).
//! Tests that depend on the main crate's types remain here.

#[cfg(test)]
mod tests {
    use gpui::AppContext as _;
    use crate::workspace::state::{LayoutNode, ProjectData, SplitDirection, Workspace, WorkspaceData};
    use crate::workspace::settings::HooksConfig;
    use crate::terminal::shell_config::ShellType;
    use crate::theme::FolderColor;
    use std::collections::HashMap;

    fn terminal_node(id: &str) -> LayoutNode {
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
            show_in_overview: true,
            layout: Some(layout),
            terminal_names: HashMap::new(),
            hidden_terminals: HashMap::new(),
            worktree_info: None,
            folder_color: FolderColor::default(),
            hooks: HooksConfig::default(),
            is_remote: false,
            connection_id: None,
            service_terminals: HashMap::new(),
            remote_services: vec![],
            remote_host: None,
            remote_git_status: None,
        }
    }

    fn make_workspace_data(projects: Vec<ProjectData>, order: Vec<&str>) -> WorkspaceData {
        WorkspaceData {
            version: 1,
            projects,
            project_order: order.into_iter().map(String::from).collect(),
            project_widths: HashMap::new(),
            folders: vec![],
            service_panel_heights: HashMap::new(),
        }
    }

    #[gpui::test]
    fn test_add_app_as_tab_wraps_in_tabs(cx: &mut gpui::TestAppContext) {
        // A bare terminal should be wrapped in Tabs { [Terminal, App], active_tab: 1 }
        let layout = terminal_node("t1");
        let project = make_project_with_layout("p1", layout);
        let data = make_workspace_data(vec![project], vec!["p1"]);
        let workspace = cx.new(|_cx| Workspace::new(data));

        let app_id = workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.add_app_as_tab("p1", &[], "kruh", serde_json::Value::Null, cx)
        });

        assert!(app_id.is_some());
        workspace.read_with(cx, |ws: &Workspace, _cx| {
            let layout = ws.project("p1").unwrap().layout.as_ref().unwrap();
            match layout {
                LayoutNode::Tabs { children, active_tab } => {
                    assert_eq!(children.len(), 2);
                    assert_eq!(*active_tab, 1);
                    assert!(matches!(&children[0], LayoutNode::Terminal { terminal_id: Some(id), .. } if id == "t1"));
                    assert!(matches!(&children[1], LayoutNode::App { app_kind, .. } if app_kind == "kruh"));
                }
                _ => panic!("Expected Tabs, got {:?}", layout),
            }
        });
    }

    #[gpui::test]
    fn test_add_app_as_tab_appends_to_existing_tabs(cx: &mut gpui::TestAppContext) {
        // When focused inside an existing Tabs, should append rather than nest
        let layout = LayoutNode::Tabs {
            children: vec![terminal_node("t1"), terminal_node("t2")],
            active_tab: 0,
        };
        let project = make_project_with_layout("p1", layout);
        let data = make_workspace_data(vec![project], vec!["p1"]);
        let workspace = cx.new(|_cx| Workspace::new(data));

        let app_id = workspace.update(cx, |ws: &mut Workspace, cx| {
            // Path [0] = first child of the Tabs container; parent is Tabs at []
            ws.add_app_as_tab("p1", &[0], "kruh", serde_json::Value::Null, cx)
        });

        assert!(app_id.is_some());
        workspace.read_with(cx, |ws: &Workspace, _cx| {
            let layout = ws.project("p1").unwrap().layout.as_ref().unwrap();
            match layout {
                LayoutNode::Tabs { children, active_tab } => {
                    assert_eq!(children.len(), 3);
                    assert_eq!(*active_tab, 2); // new app is active
                    assert!(matches!(&children[2], LayoutNode::App { app_kind, .. } if app_kind == "kruh"));
                }
                _ => panic!("Expected Tabs, got {:?}", layout),
            }
        });
    }

    #[gpui::test]
    fn test_replace_pane_with_app(cx: &mut gpui::TestAppContext) {
        // Should replace the terminal node with an App node at the same path
        let layout = terminal_node("t1");
        let project = make_project_with_layout("p1", layout);
        let data = make_workspace_data(vec![project], vec!["p1"]);
        let workspace = cx.new(|_cx| Workspace::new(data));

        let app_id = workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.replace_pane_with_app("p1", &[], "kruh", serde_json::Value::Null, cx)
        });

        assert!(app_id.is_some());
        workspace.read_with(cx, |ws: &Workspace, _cx| {
            let layout = ws.project("p1").unwrap().layout.as_ref().unwrap();
            assert!(matches!(layout, LayoutNode::App { app_kind, .. } if app_kind == "kruh"));
        });
    }

    #[gpui::test]
    fn test_replace_pane_preserves_siblings(cx: &mut gpui::TestAppContext) {
        // When replacing inside a Split, siblings remain untouched
        let layout = LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            sizes: vec![50.0, 50.0],
            children: vec![terminal_node("t1"), terminal_node("t2")],
        };
        let project = make_project_with_layout("p1", layout);
        let data = make_workspace_data(vec![project], vec!["p1"]);
        let workspace = cx.new(|_cx| Workspace::new(data));

        let app_id = workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.replace_pane_with_app("p1", &[0], "kruh", serde_json::Value::Null, cx)
        });

        assert!(app_id.is_some());
        workspace.read_with(cx, |ws: &Workspace, _cx| {
            let layout = ws.project("p1").unwrap().layout.as_ref().unwrap();
            match layout {
                LayoutNode::Split { children, .. } => {
                    assert_eq!(children.len(), 2);
                    assert!(matches!(&children[0], LayoutNode::App { app_kind, .. } if app_kind == "kruh"));
                    assert!(matches!(&children[1], LayoutNode::Terminal { terminal_id: Some(id), .. } if id == "t2"));
                }
                _ => panic!("Expected Split, got {:?}", layout),
            }
        });
    }
}
