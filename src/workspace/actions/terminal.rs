//! Terminal-specific workspace actions
//!
//! Actions for managing individual terminals within projects.

use crate::terminal::shell_config::ShellType;
use crate::workspace::state::{DetachedTerminalState, LayoutNode, Workspace};
use gpui::*;

impl Workspace {
    /// Set terminal ID at a layout path and assign a default name based on directory
    pub fn set_terminal_id(
        &mut self,
        project_id: &str,
        path: &[usize],
        terminal_id: String,
        cx: &mut Context<Self>,
    ) {
        let tid = terminal_id.clone();
        self.with_project(project_id, cx, |project| {
            // Set terminal ID in layout node
            if let Some(ref mut layout) = project.layout {
                if let Some(node) = layout.get_at_path_mut(path) {
                    if let LayoutNode::Terminal { terminal_id: id, .. } = node {
                        *id = Some(terminal_id);

                        // Set default name based on directory if not already set
                        if !project.terminal_names.contains_key(&tid) {
                            let base_name = std::path::Path::new(&project.path)
                                .file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or("Terminal")
                                .to_string();

                            // Check if name already exists and add counter if needed
                            let existing_names: Vec<_> = project.terminal_names.values().collect();
                            let default_name = if existing_names.contains(&&base_name) {
                                // Find next available number
                                let mut counter = 2;
                                loop {
                                    let candidate = format!("{} ({})", base_name, counter);
                                    if !existing_names.contains(&&candidate) {
                                        break candidate;
                                    }
                                    counter += 1;
                                }
                            } else {
                                base_name
                            };

                            project.terminal_names.insert(tid, default_name);
                        }
                        return true;
                    }
                }
            }
            false
        });
    }

    /// Set shell type for a terminal at a layout path
    pub fn set_terminal_shell(
        &mut self,
        project_id: &str,
        path: &[usize],
        shell_type: ShellType,
        cx: &mut Context<Self>,
    ) {
        self.with_layout_node(project_id, path, cx, |node| {
            if let LayoutNode::Terminal { shell_type: st, .. } = node {
                *st = shell_type;
                return true;
            }
            false
        });
    }

    /// Get shell type for a terminal at a layout path
    pub fn get_terminal_shell(&self, project_id: &str, path: &[usize]) -> Option<ShellType> {
        let project = self.project(project_id)?;
        if let Some(LayoutNode::Terminal { shell_type, .. }) = project.layout.as_ref().and_then(|l| l.get_at_path(path)) {
            Some(shell_type.clone())
        } else {
            None
        }
    }

    /// Toggle terminal minimized state
    pub fn toggle_terminal_minimized(
        &mut self,
        project_id: &str,
        path: &[usize],
        cx: &mut Context<Self>,
    ) {
        self.with_layout_node(project_id, path, cx, |node| {
            if let LayoutNode::Terminal { minimized, .. } = node {
                *minimized = !*minimized;
                true
            } else {
                false
            }
        });
    }

    /// Rename a terminal
    pub fn rename_terminal(
        &mut self,
        project_id: &str,
        terminal_id: &str,
        new_name: String,
        cx: &mut Context<Self>,
    ) {
        let terminal_id = terminal_id.to_string();
        self.with_project(project_id, cx, |project| {
            project.terminal_names.insert(terminal_id, new_name);
            true
        });
    }

    /// Set terminal hidden state
    #[allow(dead_code)] // API for future terminal visibility control
    pub fn set_terminal_hidden(
        &mut self,
        project_id: &str,
        terminal_id: &str,
        hidden: bool,
        cx: &mut Context<Self>,
    ) {
        let terminal_id = terminal_id.to_string();
        self.with_project(project_id, cx, |project| {
            project.hidden_terminals.insert(terminal_id, hidden);
            true
        });
    }

    /// Restore (un-minimize) a terminal at a path
    pub fn restore_terminal(&mut self, project_id: &str, path: &[usize], cx: &mut Context<Self>) {
        self.with_layout_node(project_id, path, cx, |node| {
            if let LayoutNode::Terminal { minimized, .. } = node {
                *minimized = false;
                true
            } else {
                false
            }
        });
    }

    /// Toggle terminal minimized state by terminal ID (finds path automatically)
    pub fn toggle_terminal_minimized_by_id(
        &mut self,
        project_id: &str,
        terminal_id: &str,
        cx: &mut Context<Self>,
    ) {
        if let Some(project) = self.project_mut(project_id) {
            if let Some(ref mut layout) = project.layout {
                if let Some(path) = layout.find_terminal_path(terminal_id) {
                    if let Some(node) = layout.get_at_path_mut(&path) {
                        if let LayoutNode::Terminal { minimized, .. } = node {
                            *minimized = !*minimized;
                            cx.notify();
                        }
                    }
                }
            }
        }
    }

    /// Check if a terminal is minimized by ID
    pub fn is_terminal_minimized(&self, project_id: &str, terminal_id: &str) -> bool {
        if let Some(project) = self.project(project_id) {
            if let Some(ref layout) = project.layout {
                if let Some(path) = layout.find_terminal_path(terminal_id) {
                    if let Some(LayoutNode::Terminal { minimized, .. }) = layout.get_at_path(&path) {
                        return *minimized;
                    }
                }
            }
        }
        false
    }

    /// Detach a terminal to a separate window
    /// Returns the detached state for window creation
    pub fn detach_terminal(
        &mut self,
        project_id: &str,
        path: &[usize],
        cx: &mut Context<Self>,
    ) -> Option<DetachedTerminalState> {
        // Get terminal ID from the layout node
        let terminal_id = if let Some(project) = self.project(project_id) {
            if let Some(ref layout) = project.layout {
                if let Some(LayoutNode::Terminal { terminal_id: Some(id), .. }) = layout.get_at_path(path) {
                    id.clone()
                } else {
                    return None;
                }
            } else {
                return None;
            }
        } else {
            return None;
        };

        // Check if already detached
        if self.detached_terminals.iter().any(|d| d.terminal_id == terminal_id) {
            return None;
        }

        // Mark terminal as detached in layout
        self.with_layout_node(project_id, path, cx, |node| {
            if let LayoutNode::Terminal { detached, .. } = node {
                *detached = true;
                true
            } else {
                false
            }
        });

        // Create detached state
        let state = DetachedTerminalState {
            terminal_id: terminal_id.clone(),
            project_id: project_id.to_string(),
            layout_path: path.to_vec(),
        };

        self.detached_terminals.push(state.clone());
        cx.notify();

        Some(state)
    }

    /// Re-attach a detached terminal back to its original location
    pub fn attach_terminal(&mut self, terminal_id: &str, cx: &mut Context<Self>) {
        // Find and remove from detached list
        let detached = self.detached_terminals.iter()
            .position(|d| d.terminal_id == terminal_id)
            .map(|i| self.detached_terminals.remove(i));

        if let Some(state) = detached {
            // Mark terminal as not detached in layout
            self.with_layout_node(&state.project_id, &state.layout_path, cx, |node| {
                if let LayoutNode::Terminal { detached, .. } = node {
                    *detached = false;
                    true
                } else {
                    false
                }
            });
        }

        cx.notify();
    }

    /// Check if a terminal is detached
    pub fn is_terminal_detached(&self, terminal_id: &str) -> bool {
        self.detached_terminals.iter().any(|d| d.terminal_id == terminal_id)
    }

    /// Get the zoom level for a terminal at the given path
    pub fn get_terminal_zoom(&self, project_id: &str, path: &[usize]) -> f32 {
        self.project(project_id)
            .and_then(|p| p.layout.as_ref())
            .and_then(|l| l.get_at_path(path))
            .and_then(|node| {
                if let LayoutNode::Terminal { zoom_level, .. } = node {
                    Some(*zoom_level)
                } else {
                    None
                }
            })
            .unwrap_or(1.0)
    }

    /// Set the zoom level for a terminal at the given path
    pub fn set_terminal_zoom(
        &mut self,
        project_id: &str,
        path: &[usize],
        zoom: f32,
        cx: &mut Context<Self>,
    ) {
        let clamped = zoom.clamp(0.5, 3.0);
        self.with_layout_node(project_id, path, cx, |node| {
            if let LayoutNode::Terminal { zoom_level, .. } = node {
                *zoom_level = clamped;
                true
            } else {
                false
            }
        });
    }

    /// Get detached terminal state by terminal ID
    #[allow(dead_code)] // API for detached terminal access
    pub fn get_detached_terminal(&self, terminal_id: &str) -> Option<&DetachedTerminalState> {
        self.detached_terminals.iter().find(|d| d.terminal_id == terminal_id)
    }
}
