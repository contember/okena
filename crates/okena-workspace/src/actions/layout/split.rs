//! Split operations: `split_terminal`, split-size updates, equalize.

use crate::focus::FocusManager;
use crate::state::{LayoutNode, SplitDirection, Workspace};
use gpui::*;

impl Workspace {
    /// Split a terminal at a path
    pub fn split_terminal(
        &mut self,
        focus_manager: &mut FocusManager,
        project_id: &str,
        path: &[usize],
        direction: SplitDirection,
        cx: &mut Context<Self>,
    ) {
        log::info!("Workspace::split_terminal called for project {} at path {:?}", project_id, path);

        // If the target node is inside a Tabs container, split the Tabs container
        // instead of splitting inside the tab. This avoids nested splits within tabs
        // which creates a clunky UI.
        let split_path = if let Some(project) = self.project(project_id) {
            if let Some(ref layout) = project.layout {
                if path.len() >= 1 {
                    let parent_path = &path[..path.len() - 1];
                    if let Some(LayoutNode::Tabs { .. }) = layout.get_at_path(parent_path) {
                        parent_path.to_vec()
                    } else {
                        path.to_vec()
                    }
                } else {
                    path.to_vec()
                }
            } else {
                path.to_vec()
            }
        } else {
            path.to_vec()
        };

        // Perform the split and find the new terminal's path after normalization.
        let new_path = if let Some(project) = self.project_mut(project_id) {
            if let Some(ref mut layout) = project.layout {
                if let Some(node) = layout.get_at_path_mut(&split_path) {
                    log::info!("Found node at path {:?}, splitting...", split_path);
                    let old_node = node.clone();
                    *node = LayoutNode::Split {
                        direction,
                        sizes: vec![50.0, 50.0],
                        children: vec![old_node, LayoutNode::new_terminal()],
                    };
                    layout.normalize();
                    log::info!("Split complete");
                    // The newly created terminal has terminal_id: None — find its path
                    layout.find_uninitialized_terminal_path()
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };

        self.notify_data(cx);

        if let Some(new_path) = new_path {
            self.set_focused_terminal(focus_manager, project_id.to_string(), new_path, cx);
        }
    }

    /// Update split sizes at a path
    pub fn update_split_sizes(
        &mut self,
        project_id: &str,
        path: &[usize],
        new_sizes: Vec<f32>,
        cx: &mut Context<Self>,
    ) {
        self.with_layout_node(project_id, path, cx, |node| {
            if let LayoutNode::Split { sizes, .. } = node {
                *sizes = new_sizes;
                true
            } else {
                false
            }
        });
    }

    /// Update split sizes without bumping data_version (UI-only notify).
    /// Use during interactive drag to avoid auto-save spam; call `update_split_sizes`
    /// on mouse-up to persist the final sizes.
    pub fn update_split_sizes_ui_only(
        &mut self,
        project_id: &str,
        path: &[usize],
        new_sizes: Vec<f32>,
        cx: &mut Context<Self>,
    ) {
        if let Some(project) = self.project_mut(project_id) {
            if let Some(ref mut layout) = project.layout {
                if let Some(node) = layout.get_at_path_mut(path) {
                    if let LayoutNode::Split { sizes, .. } = node {
                        *sizes = new_sizes;
                        self.notify_ui_only(cx);
                    }
                }
            }
        }
    }

    /// Equalize pane sizes in the focused terminal's parent split.
    pub fn equalize_focused_split(&mut self, focus_manager: &FocusManager, cx: &mut Context<Self>) {
        if let Some(target) = focus_manager.focused_terminal_state() {
            if let Some(project) = self.project_mut(&target.project_id) {
                if let Some(ref mut layout) = project.layout {
                    let parent_path = if target.layout_path.is_empty() {
                        &target.layout_path[..]
                    } else {
                        &target.layout_path[..target.layout_path.len() - 1]
                    };
                    if let Some(node) = layout.get_at_path_mut(parent_path) {
                        if let LayoutNode::Split { sizes, children, .. } = node {
                            let n = children.len();
                            *sizes = vec![100.0 / n as f32; n];
                        }
                    }
                }
            }
        }
        self.notify_data(cx);
    }
}
