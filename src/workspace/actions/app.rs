//! App pane workspace actions
//!
//! Actions for managing app panes within projects.

use crate::workspace::state::{LayoutNode, SplitDirection, Workspace};
use gpui::*;

impl Workspace {
    /// Create a new app pane and insert it into the project's layout.
    /// Returns the generated app_id if successful.
    pub fn add_app(
        &mut self,
        project_id: &str,
        kind: impl Into<String>,
        config: serde_json::Value,
        cx: &mut Context<Self>,
    ) -> Option<String> {
        let app_id = uuid::Uuid::new_v4().to_string();
        let new_node = LayoutNode::App {
            app_id: Some(app_id.clone()),
            app_kind: kind.into(),
            app_config: config,
        };

        let project = self.project_mut(project_id)?;
        if let Some(ref old_layout) = project.layout {
            let old_layout = old_layout.clone();
            project.layout = Some(LayoutNode::Split {
                direction: SplitDirection::Horizontal,
                sizes: vec![50.0, 50.0],
                children: vec![old_layout, new_node],
            });
        } else {
            project.layout = Some(new_node);
        }
        self.notify_data(cx);
        Some(app_id)
    }

    /// Set the app_id on an existing App node at the given path.
    #[allow(dead_code)]
    pub fn set_app_id(
        &mut self,
        project_id: &str,
        path: &[usize],
        app_id: String,
        cx: &mut Context<Self>,
    ) {
        self.with_layout_node(project_id, path, cx, |node| {
            if let LayoutNode::App { app_id: id, .. } = node {
                *id = Some(app_id);
                return true;
            }
            false
        });
    }

    /// Create a new app pane and insert it as a tab alongside the pane at `path`.
    ///
    /// If the parent of `path` is already a `Tabs` container, the app is appended.
    /// Otherwise, the node at `path` is wrapped in a new `Tabs` container.
    /// Returns the generated app_id if successful.
    pub fn add_app_as_tab(
        &mut self,
        project_id: &str,
        path: &[usize],
        kind: impl Into<String>,
        config: serde_json::Value,
        cx: &mut Context<Self>,
    ) -> Option<String> {
        let app_id = uuid::Uuid::new_v4().to_string();
        let new_node = LayoutNode::App {
            app_id: Some(app_id.clone()),
            app_kind: kind.into(),
            app_config: config,
        };

        // Check if parent is a Tabs container
        if !path.is_empty() {
            let parent_path = &path[..path.len() - 1];
            if let Some(project) = self.project(project_id) {
                if let Some(ref layout) = project.layout {
                    if let Some(LayoutNode::Tabs { .. }) = layout.get_at_path(parent_path) {
                        // Append to existing Tabs
                        let mut new_tab_index = 0;
                        self.with_layout_node(project_id, parent_path, cx, |node| {
                            if let LayoutNode::Tabs { children, active_tab } = node {
                                children.push(new_node);
                                *active_tab = children.len() - 1;
                                new_tab_index = *active_tab;
                                true
                            } else {
                                false
                            }
                        });
                        let mut new_path = parent_path.to_vec();
                        new_path.push(new_tab_index);
                        self.set_focused_terminal(project_id.to_string(), new_path, cx);
                        return Some(app_id);
                    }
                }
            }
        }

        // Wrap in new Tabs container
        self.with_layout_node(project_id, path, cx, |node| {
            let old_node = node.clone();
            *node = LayoutNode::Tabs {
                children: vec![old_node, new_node],
                active_tab: 1,
            };
            true
        });

        let mut new_path = path.to_vec();
        new_path.push(1);
        self.set_focused_terminal(project_id.to_string(), new_path, cx);
        Some(app_id)
    }

    /// Replace the node at `path` with a new App pane.
    /// Returns the generated app_id if successful.
    pub fn replace_pane_with_app(
        &mut self,
        project_id: &str,
        path: &[usize],
        kind: impl Into<String>,
        config: serde_json::Value,
        cx: &mut Context<Self>,
    ) -> Option<String> {
        let app_id = uuid::Uuid::new_v4().to_string();
        let new_node = LayoutNode::App {
            app_id: Some(app_id.clone()),
            app_kind: kind.into(),
            app_config: config,
        };

        let replaced = self.with_layout_node(project_id, path, cx, |node| {
            *node = new_node;
            true
        });

        if replaced {
            self.set_focused_terminal(project_id.to_string(), path.to_vec(), cx);
            Some(app_id)
        } else {
            None
        }
    }

    /// Find and remove an App node by app_id.
    /// Returns true if the app was found and removed.
    pub fn close_app(
        &mut self,
        project_id: &str,
        app_id: &str,
        cx: &mut Context<Self>,
    ) -> bool {
        let path = {
            let project = self.project(project_id);
            match project {
                Some(p) => p.layout.as_ref().and_then(|l| l.find_app_path(app_id)),
                None => None,
            }
        };

        let Some(path) = path else {
            return false;
        };

        if path.is_empty() {
            // App is root — remove layout entirely
            if let Some(project) = self.project_mut(project_id) {
                project.layout = None;
            }
            self.notify_data(cx);
            return true;
        }

        // Use remove_at_path which handles collapsing parent containers
        if let Some(project) = self.project_mut(project_id) {
            if let Some(ref mut layout) = project.layout {
                if layout.remove_at_path(&path).is_some() {
                    self.notify_data(cx);
                    return true;
                }
            }
        }

        false
    }
}

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
            is_visible: true,
            layout: Some(layout),
            terminal_names: HashMap::new(),
            hidden_terminals: HashMap::new(),
            worktree_info: None,
            folder_color: FolderColor::default(),
            hooks: HooksConfig::default(),
            is_remote: false,
            connection_id: None,
        }
    }

    fn make_workspace_data(projects: Vec<ProjectData>, order: Vec<&str>) -> WorkspaceData {
        WorkspaceData {
            version: 1,
            projects,
            project_order: order.into_iter().map(String::from).collect(),
            project_widths: HashMap::new(),
            folders: vec![],
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
