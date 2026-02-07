use crate::theme::FolderColor;
use crate::workspace::focus::FocusManager;
use gpui::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A folder that groups projects in the sidebar
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FolderData {
    pub id: String,
    pub name: String,
    /// Ordered project IDs inside this folder
    pub project_ids: Vec<String>,
    #[serde(default)]
    pub collapsed: bool,
    #[serde(default)]
    pub folder_color: FolderColor,
}

/// The main workspace data structure (serializable)
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkspaceData {
    /// Schema version for migration support
    #[serde(default = "default_workspace_version")]
    pub version: u32,
    pub projects: Vec<ProjectData>,
    pub project_order: Vec<String>,
    /// Project column widths as percentages (project_id -> width %)
    #[serde(default)]
    pub project_widths: HashMap<String, f32>,
    /// Folders for grouping projects
    #[serde(default)]
    pub folders: Vec<FolderData>,
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
    #[serde(default = "default_true")]
    pub is_visible: bool,
    /// Layout tree for terminal panes. None means project is a bookmark without terminals.
    pub layout: Option<LayoutNode>,
    #[serde(default)]
    pub terminal_names: HashMap<String, String>,
    #[serde(default)]
    pub hidden_terminals: HashMap<String, bool>,
    /// Optional worktree metadata (only set for worktree projects)
    #[serde(default)]
    pub worktree_info: Option<WorktreeMetadata>,
    /// Folder icon color for this project
    #[serde(default)]
    pub folder_color: FolderColor,
    /// Per-project lifecycle hooks (overrides global settings)
    #[serde(default)]
    pub hooks: crate::workspace::persistence::HooksConfig,
}

use crate::terminal::shell_config::ShellType;

fn default_workspace_version() -> u32 {
    0 // pre-versioning workspace files
}

fn default_true() -> bool {
    true
}

fn default_zoom_level() -> f32 {
    1.0
}

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
        #[serde(default = "default_zoom_level")]
        zoom_level: f32,
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

/// State for focused terminal (for visual indicator)
#[derive(Clone, Debug, PartialEq)]
pub struct FocusedTerminalState {
    pub project_id: String,
    pub layout_path: Vec<usize>,
}

/// Global workspace wrapper for app-wide access (used by quit handler)
#[derive(Clone)]
pub struct GlobalWorkspace(pub Entity<Workspace>);

impl Global for GlobalWorkspace {}

/// GPUI Entity for workspace state
pub struct Workspace {
    pub(crate) data: WorkspaceData,
    /// Unified focus manager for the workspace
    pub focus_manager: FocusManager,
    /// Last access time for each project (for sorting in project switcher)
    pub project_access_times: HashMap<String, std::time::Instant>,
    /// Monotonic counter incremented only on persistent data mutations.
    /// The auto-save observer compares this to skip saves for UI-only changes.
    data_version: u64,
}

impl Workspace {
    pub fn new(data: WorkspaceData) -> Self {
        Self {
            data,
            focus_manager: FocusManager::new(),
            project_access_times: HashMap::new(),
            data_version: 0,
        }
    }

    /// Current data version (incremented on persistent data mutations)
    pub fn data_version(&self) -> u64 {
        self.data_version
    }

    /// Read-only access to persistent workspace data.
    pub fn data(&self) -> &WorkspaceData {
        &self.data
    }

    /// Notify that persistent data changed. Bumps version and calls cx.notify().
    /// Use this instead of cx.notify() when mutating `self.data`.
    pub fn notify_data(&mut self, cx: &mut Context<Self>) {
        self.data_version += 1;
        cx.notify();
    }

    /// Replace workspace data wholesale (e.g. from disk reload).
    /// Does NOT bump data_version — the data came from disk, not a user edit.
    pub fn replace_data(&mut self, data: WorkspaceData, cx: &mut Context<Self>) {
        self.data = data;
        self.focus_manager.clear_all();
        cx.notify();
    }

    /// Record that a project was accessed (for sorting by recency)
    pub fn touch_project(&mut self, project_id: &str) {
        self.project_access_times.insert(project_id.to_string(), std::time::Instant::now());
    }

    /// Get projects sorted by last access time (most recent first)
    pub fn projects_by_recency(&self) -> Vec<&ProjectData> {
        let mut projects: Vec<&ProjectData> = self.data.projects.iter().collect();
        projects.sort_by(|a, b| {
            let time_a = self.project_access_times.get(&a.id);
            let time_b = self.project_access_times.get(&b.id);
            match (time_a, time_b) {
                (Some(ta), Some(tb)) => tb.cmp(ta), // Most recent first
                (Some(_), None) => std::cmp::Ordering::Less, // Accessed projects first
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            }
        });
        projects
    }

    pub fn projects(&self) -> &[ProjectData] {
        &self.data.projects
    }

    /// Get the currently focused/zoomed project ID.
    /// Delegates to FocusManager (single source of truth).
    pub fn focused_project_id(&self) -> Option<&String> {
        self.focus_manager.focused_project_id()
    }

    /// Get visible projects in order, expanding folders into their contained projects
    pub fn visible_projects(&self) -> Vec<&ProjectData> {
        let focused = self.focused_project_id();
        let mut result = Vec::new();
        for id in &self.data.project_order {
            if let Some(folder) = self.data.folders.iter().find(|f| f.id == *id) {
                // Folder: include its projects
                for pid in &folder.project_ids {
                    if let Some(p) = self.data.projects.iter().find(|p| p.id == *pid) {
                        if focused.map_or(p.is_visible, |fid| &p.id == fid) {
                            result.push(p);
                        }
                    }
                }
            } else if let Some(p) = self.data.projects.iter().find(|p| p.id == *id) {
                if focused.map_or(p.is_visible, |fid| &p.id == fid) {
                    result.push(p);
                }
            }
        }
        result
    }

    /// Get a project by ID
    pub fn project(&self, id: &str) -> Option<&ProjectData> {
        self.data.projects.iter().find(|p| p.id == id)
    }

    /// Get a mutable project by ID
    pub(crate) fn project_mut(&mut self, id: &str) -> Option<&mut ProjectData> {
        self.data.projects.iter_mut().find(|p| p.id == id)
    }

    /// Get a folder by ID
    pub fn folder(&self, id: &str) -> Option<&FolderData> {
        self.data.folders.iter().find(|f| f.id == id)
    }

    /// Get a mutable folder by ID
    pub(crate) fn folder_mut(&mut self, id: &str) -> Option<&mut FolderData> {
        self.data.folders.iter_mut().find(|f| f.id == id)
    }

    /// Check if an ID in project_order refers to a folder
    #[allow(dead_code)]
    pub fn is_folder(&self, id: &str) -> bool {
        self.data.folders.iter().any(|f| f.id == id)
    }

    /// Find which folder (if any) contains a given project
    #[allow(dead_code)]
    pub fn folder_for_project(&self, project_id: &str) -> Option<&FolderData> {
        self.data.folders.iter().find(|f| f.project_ids.contains(&project_id.to_string()))
    }

    /// Collect all detached terminals across all projects by traversing layout trees.
    /// Returns (terminal_id, project_id, layout_path) tuples.
    pub fn collect_all_detached_terminals(&self) -> Vec<(String, String, Vec<usize>)> {
        let mut result = Vec::new();
        for project in &self.data.projects {
            if let Some(ref layout) = project.layout {
                for (terminal_id, layout_path) in layout.collect_detached_terminals() {
                    result.push((terminal_id, project.id.clone(), layout_path));
                }
            }
        }
        result
    }

    /// Helper to mutate a layout node at a path, with automatic notify.
    /// Returns true if the mutation was applied.
    pub fn with_layout_node<F>(&mut self, project_id: &str, path: &[usize], cx: &mut Context<Self>, f: F) -> bool
    where
        F: FnOnce(&mut LayoutNode) -> bool,
    {
        if let Some(project) = self.project_mut(project_id) {
            if let Some(ref mut layout) = project.layout {
                if let Some(node) = layout.get_at_path_mut(path) {
                    if f(node) {
                        self.notify_data(cx);
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Helper to mutate a layout node at a path, normalize the root layout, then notify.
    /// Use this instead of `with_layout_node` when the mutation may create nested splits
    /// that should be flattened (e.g. splitting a terminal).
    /// Returns true if the mutation was applied.
    pub fn with_layout_node_normalized<F>(&mut self, project_id: &str, path: &[usize], cx: &mut Context<Self>, f: F) -> bool
    where
        F: FnOnce(&mut LayoutNode) -> bool,
    {
        if let Some(project) = self.project_mut(project_id) {
            if let Some(ref mut layout) = project.layout {
                if let Some(node) = layout.get_at_path_mut(path) {
                    if f(node) {
                        layout.normalize();
                        self.notify_data(cx);
                        return true;
                    }
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
                self.notify_data(cx);
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
            zoom_level: 1.0,
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

    /// Normalize the layout tree in-place:
    /// - Flatten nested splits with the same direction (merging sizes proportionally)
    /// - Unwrap splits/tabs with a single child
    /// - Remove empty containers
    pub fn normalize(&mut self) {
        // First, recursively normalize children
        match self {
            LayoutNode::Terminal { .. } => return,
            LayoutNode::Split { children, .. } | LayoutNode::Tabs { children, .. } => {
                for child in children.iter_mut() {
                    child.normalize();
                }
            }
        }

        // Unwrap single-child or empty containers
        let should_unwrap = match self {
            LayoutNode::Split { children, .. } | LayoutNode::Tabs { children, .. } => children.len() <= 1,
            _ => false,
        };
        if should_unwrap {
            match self {
                LayoutNode::Split { children, .. } | LayoutNode::Tabs { children, .. } => {
                    if children.len() == 1 {
                        *self = children.remove(0);
                    } else {
                        // Empty container - replace with a default terminal
                        *self = LayoutNode::new_terminal();
                    }
                }
                _ => {}
            }
            return;
        }

        // Flatten nested splits with the same direction
        if let LayoutNode::Split { direction, sizes, children } = self {
            let has_same_dir_child = children.iter().any(|c| matches!(c, LayoutNode::Split { direction: d, .. } if d == direction));
            if has_same_dir_child {
                let dir = *direction;
                let mut new_children = Vec::new();
                let mut new_sizes = Vec::new();

                for (i, child) in children.drain(..).enumerate() {
                    let parent_size = sizes[i];
                    match child {
                        LayoutNode::Split { direction: child_dir, sizes: child_sizes, children: grandchildren } if child_dir == dir => {
                            let child_total: f32 = child_sizes.iter().sum();
                            for (j, grandchild) in grandchildren.into_iter().enumerate() {
                                new_children.push(grandchild);
                                new_sizes.push(parent_size * child_sizes[j] / child_total);
                            }
                        }
                        other => {
                            new_children.push(other);
                            new_sizes.push(parent_size);
                        }
                    }
                }

                *children = new_children;
                *sizes = new_sizes;
            }
        }
    }

    /// Clone the layout structure but clear all terminal IDs
    /// Used when creating worktree projects to duplicate layout with fresh terminals
    pub fn clone_structure(&self) -> Self {
        match self {
            LayoutNode::Terminal { shell_type, zoom_level, .. } => LayoutNode::Terminal {
                terminal_id: None,
                minimized: false,
                detached: false,
                shell_type: shell_type.clone(),
                zoom_level: *zoom_level,
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
