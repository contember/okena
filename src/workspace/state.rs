use crate::workspace::focus::FocusManager;
use gpui::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// The main workspace data structure (serializable)
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkspaceData {
    pub projects: Vec<ProjectData>,
    pub project_order: Vec<String>,
    /// Project column widths as percentages (project_id -> width %)
    #[serde(default)]
    pub project_widths: HashMap<String, f32>,
}

/// Metadata for worktree projects
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorktreeMetadata {
    /// ID of the main repo project
    pub parent_project_id: String,
    /// Path to main repository
    pub main_repo_path: String,
}

/// A single project with its layout tree
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProjectData {
    pub id: String,
    pub name: String,
    pub path: String,
    pub is_visible: bool,
    pub layout: LayoutNode,
    #[serde(default)]
    pub terminal_names: HashMap<String, String>,
    #[serde(default)]
    pub hidden_terminals: HashMap<String, bool>,
    /// Optional worktree metadata (only set for worktree projects)
    #[serde(default)]
    pub worktree_info: Option<WorktreeMetadata>,
}

use crate::terminal::shell_config::ShellType;

/// Recursive layout tree node
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum LayoutNode {
    Terminal {
        terminal_id: Option<String>,
        #[serde(default)]
        minimized: bool,
        #[serde(default)]
        detached: bool,
        #[serde(default)]
        shell_type: ShellType,
    },
    Split {
        direction: SplitDirection,
        sizes: Vec<f32>,
        children: Vec<LayoutNode>,
    },
    Tabs {
        children: Vec<LayoutNode>,
        #[serde(default)]
        active_tab: usize,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SplitDirection {
    Horizontal,
    Vertical,
}

/// State for fullscreen terminal mode
#[derive(Clone, Debug)]
pub struct FullscreenState {
    pub project_id: String,
    pub terminal_id: String,
}

/// State for a detached terminal (opened in separate window)
#[derive(Clone, Debug)]
pub struct DetachedTerminalState {
    pub terminal_id: String,
    pub project_id: String,
    pub layout_path: Vec<usize>,
}

/// State for focused terminal (for visual indicator)
#[derive(Clone, Debug, PartialEq)]
pub struct FocusedTerminalState {
    pub project_id: String,
    pub layout_path: Vec<usize>,
}

/// Request to show worktree dialog
#[derive(Clone, Debug)]
pub struct WorktreeDialogRequest {
    pub project_id: String,
    pub project_path: String,
}

/// Request to show context menu at a position
#[derive(Clone, Debug)]
pub struct ContextMenuRequest {
    pub project_id: String,
    pub position: gpui::Point<gpui::Pixels>,
}

/// Request to show shell selector overlay
#[derive(Clone, Debug)]
pub struct ShellSelectorRequest {
    pub project_id: String,
    pub terminal_id: String,
    pub current_shell: crate::terminal::shell_config::ShellType,
}

/// GPUI Entity for workspace state
pub struct Workspace {
    pub data: WorkspaceData,
    pub focused_project_id: Option<String>,
    pub fullscreen_terminal: Option<FullscreenState>,
    /// Currently focused terminal (for visual indicator).
    ///
    /// **DEPRECATED**: Use `focus_manager.focused_terminal_state()` instead.
    /// This field is kept in sync with FocusManager for backward compatibility
    /// but should not be accessed directly in new code.
    pub focused_terminal: Option<FocusedTerminalState>,
    /// Currently detached terminals (opened in separate windows)
    pub detached_terminals: Vec<DetachedTerminalState>,
    /// Unified focus manager for the workspace
    pub focus_manager: FocusManager,
    /// Pending request to show worktree dialog
    pub worktree_dialog_request: Option<WorktreeDialogRequest>,
    /// Pending request to show context menu
    pub context_menu_request: Option<ContextMenuRequest>,
    /// Pending request to show shell selector
    pub shell_selector_request: Option<ShellSelectorRequest>,
}

impl Workspace {
    pub fn new(data: WorkspaceData) -> Self {
        Self {
            data,
            focused_project_id: None,
            fullscreen_terminal: None,
            focused_terminal: None,
            detached_terminals: Vec::new(),
            focus_manager: FocusManager::new(),
            worktree_dialog_request: None,
            context_menu_request: None,
            shell_selector_request: None,
        }
    }

    pub fn projects(&self) -> &[ProjectData] {
        &self.data.projects
    }

    /// Get visible projects in order
    pub fn visible_projects(&self) -> Vec<&ProjectData> {
        self.data
            .project_order
            .iter()
            .filter_map(|id| self.data.projects.iter().find(|p| &p.id == id))
            .filter(|p| {
                // If focused, only show focused project
                if let Some(focused_id) = &self.focused_project_id {
                    &p.id == focused_id
                } else {
                    p.is_visible
                }
            })
            .collect()
    }

    /// Get a project by ID
    pub fn project(&self, id: &str) -> Option<&ProjectData> {
        self.data.projects.iter().find(|p| p.id == id)
    }

    /// Get a mutable project by ID
    pub fn project_mut(&mut self, id: &str) -> Option<&mut ProjectData> {
        self.data.projects.iter_mut().find(|p| p.id == id)
    }

    /// Helper to mutate a layout node at a path, with automatic notify.
    /// Returns true if the mutation was applied.
    pub fn with_layout_node<F>(&mut self, project_id: &str, path: &[usize], cx: &mut Context<Self>, f: F) -> bool
    where
        F: FnOnce(&mut LayoutNode) -> bool,
    {
        if let Some(project) = self.project_mut(project_id) {
            if let Some(node) = project.layout.get_at_path_mut(path) {
                if f(node) {
                    cx.notify();
                    return true;
                }
            }
        }
        false
    }

    /// Helper to mutate a project, with automatic notify.
    /// Returns true if the mutation was applied.
    pub fn with_project<F>(&mut self, project_id: &str, cx: &mut Context<Self>, f: F) -> bool
    where
        F: FnOnce(&mut ProjectData) -> bool,
    {
        if let Some(project) = self.project_mut(project_id) {
            if f(project) {
                cx.notify();
                return true;
            }
        }
        false
    }
}

impl LayoutNode {
    /// Create a new empty terminal node
    pub fn new_terminal() -> Self {
        LayoutNode::Terminal {
            terminal_id: None,
            minimized: false,
            detached: false,
            shell_type: ShellType::Default,
        }
    }

    /// Get the layout node at a given path
    pub fn get_at_path(&self, path: &[usize]) -> Option<&LayoutNode> {
        if path.is_empty() {
            return Some(self);
        }

        match self {
            LayoutNode::Terminal { .. } => None,
            LayoutNode::Split { children, .. } | LayoutNode::Tabs { children, .. } => {
                children.get(path[0])?.get_at_path(&path[1..])
            }
        }
    }

    /// Get a mutable reference to the layout node at a given path
    pub fn get_at_path_mut(&mut self, path: &[usize]) -> Option<&mut LayoutNode> {
        if path.is_empty() {
            return Some(self);
        }

        match self {
            LayoutNode::Terminal { .. } => None,
            LayoutNode::Split { children, .. } | LayoutNode::Tabs { children, .. } => {
                children.get_mut(path[0])?.get_at_path_mut(&path[1..])
            }
        }
    }

    /// Collect all terminal IDs in this layout tree
    pub fn collect_terminal_ids(&self) -> Vec<String> {
        let mut ids = Vec::new();
        self.collect_terminal_ids_recursive(&mut ids);
        ids
    }

    fn collect_terminal_ids_recursive(&self, ids: &mut Vec<String>) {
        match self {
            LayoutNode::Terminal { terminal_id, .. } => {
                if let Some(id) = terminal_id {
                    ids.push(id.clone());
                }
            }
            LayoutNode::Split { children, .. } | LayoutNode::Tabs { children, .. } => {
                for child in children {
                    child.collect_terminal_ids_recursive(ids);
                }
            }
        }
    }

    /// Clear all terminal IDs in this layout tree (used on app restart)
    /// Also resets minimized and detached state since terminals need to be created first
    pub fn clear_terminal_ids(&mut self) {
        match self {
            LayoutNode::Terminal { terminal_id, minimized, detached, .. } => {
                *terminal_id = None;
                *minimized = false;
                *detached = false;
            }
            LayoutNode::Split { children, .. } | LayoutNode::Tabs { children, .. } => {
                for child in children {
                    child.clear_terminal_ids();
                }
            }
        }
    }

    /// Find the layout path to a terminal by its ID
    pub fn find_terminal_path(&self, target_id: &str) -> Option<Vec<usize>> {
        self.find_terminal_path_recursive(target_id, vec![])
    }

    fn find_terminal_path_recursive(&self, target_id: &str, current_path: Vec<usize>) -> Option<Vec<usize>> {
        match self {
            LayoutNode::Terminal { terminal_id, .. } => {
                if terminal_id.as_deref() == Some(target_id) {
                    Some(current_path)
                } else {
                    None
                }
            }
            LayoutNode::Split { children, .. } | LayoutNode::Tabs { children, .. } => {
                for (i, child) in children.iter().enumerate() {
                    let mut child_path = current_path.clone();
                    child_path.push(i);
                    if let Some(found_path) = child.find_terminal_path_recursive(target_id, child_path) {
                        return Some(found_path);
                    }
                }
                None
            }
        }
    }

    /// Collect all minimized terminal IDs in this layout tree
    pub fn collect_minimized_terminals(&self) -> Vec<(String, Vec<usize>)> {
        let mut result = Vec::new();
        self.collect_minimized_recursive(&mut result, vec![]);
        result
    }

    fn collect_minimized_recursive(&self, result: &mut Vec<(String, Vec<usize>)>, current_path: Vec<usize>) {
        match self {
            LayoutNode::Terminal { terminal_id, minimized, .. } => {
                if *minimized {
                    if let Some(id) = terminal_id {
                        result.push((id.clone(), current_path));
                    }
                }
            }
            LayoutNode::Split { children, .. } | LayoutNode::Tabs { children, .. } => {
                for (i, child) in children.iter().enumerate() {
                    let mut child_path = current_path.clone();
                    child_path.push(i);
                    child.collect_minimized_recursive(result, child_path);
                }
            }
        }
    }

    /// Collect all detached terminal IDs in this layout tree
    pub fn collect_detached_terminals(&self) -> Vec<(String, Vec<usize>)> {
        let mut result = Vec::new();
        self.collect_detached_recursive(&mut result, vec![]);
        result
    }

    fn collect_detached_recursive(&self, result: &mut Vec<(String, Vec<usize>)>, current_path: Vec<usize>) {
        match self {
            LayoutNode::Terminal { terminal_id, detached, .. } => {
                if *detached {
                    if let Some(id) = terminal_id {
                        result.push((id.clone(), current_path));
                    }
                }
            }
            LayoutNode::Split { children, .. } | LayoutNode::Tabs { children, .. } => {
                for (i, child) in children.iter().enumerate() {
                    let mut child_path = current_path.clone();
                    child_path.push(i);
                    child.collect_detached_recursive(result, child_path);
                }
            }
        }
    }

    /// Find the path to the first terminal in this layout subtree
    pub fn find_first_terminal_path(&self) -> Vec<usize> {
        self.find_first_terminal_path_recursive(vec![])
    }

    fn find_first_terminal_path_recursive(&self, current_path: Vec<usize>) -> Vec<usize> {
        match self {
            LayoutNode::Terminal { .. } => current_path,
            LayoutNode::Split { children, .. } | LayoutNode::Tabs { children, .. } => {
                if let Some(first_child) = children.first() {
                    let mut child_path = current_path;
                    child_path.push(0);
                    first_child.find_first_terminal_path_recursive(child_path)
                } else {
                    current_path
                }
            }
        }
    }

    /// Clone the layout structure but clear all terminal IDs
    /// Used when creating worktree projects to duplicate layout with fresh terminals
    pub fn clone_structure(&self) -> Self {
        match self {
            LayoutNode::Terminal { shell_type, .. } => LayoutNode::Terminal {
                terminal_id: None,
                minimized: false,
                detached: false,
                shell_type: shell_type.clone(),
            },
            LayoutNode::Split { direction, sizes, children } => LayoutNode::Split {
                direction: *direction,
                sizes: sizes.clone(),
                children: children.iter().map(|c| c.clone_structure()).collect(),
            },
            LayoutNode::Tabs { children, active_tab } => LayoutNode::Tabs {
                children: children.iter().map(|c| c.clone_structure()).collect(),
                active_tab: *active_tab,
            },
        }
    }
}
