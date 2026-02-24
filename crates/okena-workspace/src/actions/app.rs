//! App pane workspace actions
//!
//! Actions for managing app panes within projects.

use crate::state::{LayoutNode, SplitDirection, Workspace};
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
