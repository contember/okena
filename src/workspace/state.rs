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

pub use okena_core::types::SplitDirection;

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
    /// Returns true if this node is effectively hidden (all terminals within it are minimized or detached).
    pub fn is_all_hidden(&self) -> bool {
        match self {
            LayoutNode::Terminal { minimized, detached, .. } => *minimized || *detached,
            LayoutNode::Split { children, .. } | LayoutNode::Tabs { children, .. } => {
                children.iter().all(|c| c.is_all_hidden())
            }
        }
    }

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

    /// Find the path to the first uninitialized terminal (terminal_id: None) in this subtree.
    pub fn find_uninitialized_terminal_path(&self) -> Option<Vec<usize>> {
        self.find_uninitialized_terminal_path_recursive(vec![])
    }

    fn find_uninitialized_terminal_path_recursive(&self, current_path: Vec<usize>) -> Option<Vec<usize>> {
        match self {
            LayoutNode::Terminal { terminal_id: None, .. } => Some(current_path),
            LayoutNode::Terminal { .. } => None,
            LayoutNode::Split { children, .. } | LayoutNode::Tabs { children, .. } => {
                for (i, child) in children.iter().enumerate() {
                    let mut child_path = current_path.clone();
                    child_path.push(i);
                    if let Some(path) = child.find_uninitialized_terminal_path_recursive(child_path) {
                        return Some(path);
                    }
                }
                None
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

    /// Remove a child node at the given path.
    /// If the parent has only one child left after removal, collapses the parent to that child.
    /// Returns the removed node, or None if the path is invalid.
    pub fn remove_at_path(&mut self, path: &[usize]) -> Option<LayoutNode> {
        if path.is_empty() {
            return None;
        }

        let parent_path = &path[..path.len() - 1];
        let child_index = path[path.len() - 1];

        let parent = self.get_at_path_mut(parent_path)?;

        match parent {
            LayoutNode::Terminal { .. } => None,
            LayoutNode::Split { children, sizes, .. } => {
                if child_index >= children.len() {
                    return None;
                }
                let removed = children.remove(child_index);
                if child_index < sizes.len() {
                    sizes.remove(child_index);
                }
                // Collapse if only one child remains
                if children.len() == 1 {
                    let remaining = children.remove(0);
                    *parent = remaining;
                }
                Some(removed)
            }
            LayoutNode::Tabs { children, active_tab } => {
                if child_index >= children.len() {
                    return None;
                }
                let removed = children.remove(child_index);
                // Adjust active_tab
                if *active_tab >= children.len() {
                    *active_tab = children.len().saturating_sub(1);
                }
                // Collapse if only one child remains
                if children.len() == 1 {
                    let remaining = children.remove(0);
                    *parent = remaining;
                }
                Some(removed)
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

        // Fix sizes/children mismatch (can happen from stale workspace.json)
        if let LayoutNode::Split { sizes, children, .. } = self {
            if sizes.len() != children.len() {
                sizes.truncate(children.len());
                while sizes.len() < children.len() {
                    sizes.push(100.0 / children.len() as f32);
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

#[cfg(test)]
mod tests {
    use crate::workspace::state::{LayoutNode, SplitDirection};
    use crate::terminal::shell_config::ShellType;

    // === Helper constructors ===

    fn terminal(id: &str) -> LayoutNode {
        LayoutNode::Terminal {
            terminal_id: Some(id.to_string()),
            minimized: false,
            detached: false,
            shell_type: ShellType::Default,
            zoom_level: 1.0,
        }
    }

    fn terminal_minimized(id: &str) -> LayoutNode {
        LayoutNode::Terminal {
            terminal_id: Some(id.to_string()),
            minimized: true,
            detached: false,
            shell_type: ShellType::Default,
            zoom_level: 1.0,
        }
    }

    fn terminal_detached(id: &str) -> LayoutNode {
        LayoutNode::Terminal {
            terminal_id: Some(id.to_string()),
            minimized: false,
            detached: true,
            shell_type: ShellType::Default,
            zoom_level: 1.0,
        }
    }

    fn hsplit(children: Vec<LayoutNode>) -> LayoutNode {
        let count = children.len();
        LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            sizes: vec![100.0 / count as f32; count],
            children,
        }
    }

    fn vsplit(children: Vec<LayoutNode>) -> LayoutNode {
        let count = children.len();
        LayoutNode::Split {
            direction: SplitDirection::Vertical,
            sizes: vec![100.0 / count as f32; count],
            children,
        }
    }

    fn tabs(children: Vec<LayoutNode>) -> LayoutNode {
        LayoutNode::Tabs {
            children,
            active_tab: 0,
        }
    }

    // === get_at_path ===

    #[test]
    fn get_at_path_empty_returns_self() {
        let node = terminal("t1");
        assert!(node.get_at_path(&[]).is_some());
    }

    #[test]
    fn get_at_path_terminal_with_non_empty_returns_none() {
        let node = terminal("t1");
        assert!(node.get_at_path(&[0]).is_none());
    }

    #[test]
    fn get_at_path_single_index() {
        let node = hsplit(vec![terminal("t1"), terminal("t2")]);
        let child = node.get_at_path(&[1]).unwrap();
        match child {
            LayoutNode::Terminal { terminal_id, .. } => {
                assert_eq!(terminal_id.as_deref(), Some("t2"));
            }
            _ => panic!("Expected terminal"),
        }
    }

    #[test]
    fn get_at_path_nested() {
        let node = hsplit(vec![
            terminal("t1"),
            vsplit(vec![terminal("t2"), terminal("t3")]),
        ]);
        let child = node.get_at_path(&[1, 0]).unwrap();
        match child {
            LayoutNode::Terminal { terminal_id, .. } => {
                assert_eq!(terminal_id.as_deref(), Some("t2"));
            }
            _ => panic!("Expected terminal"),
        }
    }

    #[test]
    fn get_at_path_out_of_bounds() {
        let node = hsplit(vec![terminal("t1")]);
        assert!(node.get_at_path(&[5]).is_none());
    }

    // === collect_terminal_ids ===

    #[test]
    fn collect_terminal_ids_single() {
        let node = terminal("t1");
        assert_eq!(node.collect_terminal_ids(), vec!["t1"]);
    }

    #[test]
    fn collect_terminal_ids_nested() {
        let node = hsplit(vec![
            terminal("t1"),
            vsplit(vec![terminal("t2"), terminal("t3")]),
        ]);
        let ids = node.collect_terminal_ids();
        assert_eq!(ids, vec!["t1", "t2", "t3"]);
    }

    #[test]
    fn collect_terminal_ids_tabs() {
        let node = tabs(vec![terminal("a"), terminal("b")]);
        assert_eq!(node.collect_terminal_ids(), vec!["a", "b"]);
    }

    #[test]
    fn collect_terminal_ids_skips_none() {
        let node = hsplit(vec![LayoutNode::new_terminal(), terminal("t1")]);
        assert_eq!(node.collect_terminal_ids(), vec!["t1"]);
    }

    // === clear_terminal_ids ===

    #[test]
    fn clear_terminal_ids_resets_all() {
        let mut node = hsplit(vec![
            terminal_minimized("t1"),
            terminal_detached("t2"),
        ]);
        node.clear_terminal_ids();
        assert!(node.collect_terminal_ids().is_empty());
        // Also check minimized/detached reset
        match &node {
            LayoutNode::Split { children, .. } => {
                for child in children {
                    if let LayoutNode::Terminal { minimized, detached, .. } = child {
                        assert!(!minimized);
                        assert!(!detached);
                    }
                }
            }
            _ => panic!("Expected split"),
        }
    }

    // === find_terminal_path ===

    #[test]
    fn find_terminal_path_existing() {
        let node = hsplit(vec![
            terminal("t1"),
            vsplit(vec![terminal("t2"), terminal("t3")]),
        ]);
        assert_eq!(node.find_terminal_path("t3"), Some(vec![1, 1]));
    }

    #[test]
    fn find_terminal_path_root() {
        let node = terminal("t1");
        assert_eq!(node.find_terminal_path("t1"), Some(vec![]));
    }

    #[test]
    fn find_terminal_path_missing() {
        let node = terminal("t1");
        assert_eq!(node.find_terminal_path("nonexistent"), None);
    }

    // === is_all_hidden ===

    #[test]
    fn is_all_hidden_single_terminal() {
        assert!(!terminal("t1").is_all_hidden());
        assert!(terminal_minimized("t1").is_all_hidden());
        assert!(terminal_detached("t1").is_all_hidden());
    }

    #[test]
    fn is_all_hidden_split_mixed() {
        let node = hsplit(vec![terminal("t1"), terminal_minimized("t2")]);
        assert!(!node.is_all_hidden());
    }

    #[test]
    fn is_all_hidden_split_all_minimized() {
        let node = hsplit(vec![terminal_minimized("t1"), terminal_minimized("t2")]);
        assert!(node.is_all_hidden());
    }

    #[test]
    fn is_all_hidden_nested_split() {
        // Outer split where inner split has all minimized children
        let node = hsplit(vec![
            terminal("t1"),
            vsplit(vec![terminal_minimized("t2"), terminal_minimized("t3")]),
        ]);
        assert!(!node.is_all_hidden()); // t1 is still visible
    }

    #[test]
    fn is_all_hidden_nested_all_hidden() {
        let node = hsplit(vec![
            terminal_minimized("t1"),
            vsplit(vec![terminal_minimized("t2"), terminal_detached("t3")]),
        ]);
        assert!(node.is_all_hidden());
    }

    // === collect_minimized_terminals ===

    #[test]
    fn collect_minimized_terminals_finds_correct() {
        let node = hsplit(vec![
            terminal("t1"),
            terminal_minimized("t2"),
            terminal("t3"),
        ]);
        let minimized = node.collect_minimized_terminals();
        assert_eq!(minimized.len(), 1);
        assert_eq!(minimized[0].0, "t2");
        assert_eq!(minimized[0].1, vec![1]);
    }

    // === collect_detached_terminals ===

    #[test]
    fn collect_detached_terminals_finds_correct() {
        let node = hsplit(vec![
            terminal_detached("t1"),
            terminal("t2"),
        ]);
        let detached = node.collect_detached_terminals();
        assert_eq!(detached.len(), 1);
        assert_eq!(detached[0].0, "t1");
        assert_eq!(detached[0].1, vec![0]);
    }

    // === find_first_terminal_path ===

    #[test]
    fn find_first_terminal_path_terminal() {
        let node = terminal("t1");
        let empty: Vec<usize> = vec![];
        assert_eq!(node.find_first_terminal_path(), empty);
    }

    #[test]
    fn find_first_terminal_path_split() {
        let node = hsplit(vec![terminal("t1"), terminal("t2")]);
        assert_eq!(node.find_first_terminal_path(), vec![0]);
    }

    #[test]
    fn find_first_terminal_path_nested() {
        let node = hsplit(vec![
            vsplit(vec![terminal("t1"), terminal("t2")]),
            terminal("t3"),
        ]);
        assert_eq!(node.find_first_terminal_path(), vec![0, 0]);
    }

    #[test]
    fn find_first_terminal_path_tabs() {
        let node = tabs(vec![terminal("t1"), terminal("t2")]);
        assert_eq!(node.find_first_terminal_path(), vec![0]);
    }

    // === normalize ===

    #[test]
    fn normalize_single_child_split_unwraps() {
        let mut node = hsplit(vec![terminal("t1")]);
        node.normalize();
        match &node {
            LayoutNode::Terminal { terminal_id, .. } => {
                assert_eq!(terminal_id.as_deref(), Some("t1"));
            }
            _ => panic!("Expected terminal after normalizing single-child split"),
        }
    }

    #[test]
    fn normalize_empty_split_becomes_terminal() {
        let mut node = LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            sizes: vec![],
            children: vec![],
        };
        node.normalize();
        assert!(matches!(node, LayoutNode::Terminal { .. }));
    }

    #[test]
    fn normalize_nested_same_direction_flattens() {
        // H[H[t1, t2], t3] -> H[t1, t2, t3]
        let mut node = LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            sizes: vec![50.0, 50.0],
            children: vec![
                LayoutNode::Split {
                    direction: SplitDirection::Horizontal,
                    sizes: vec![50.0, 50.0],
                    children: vec![terminal("t1"), terminal("t2")],
                },
                terminal("t3"),
            ],
        };
        node.normalize();
        if let LayoutNode::Split { children, direction, sizes } = &node {
            assert_eq!(*direction, SplitDirection::Horizontal);
            assert_eq!(children.len(), 3);
            assert_eq!(sizes.len(), 3);
            // Inner split had 50% of parent (50.0), each child is 50/100 of that
            assert!((sizes[0] - 25.0).abs() < 0.01);
            assert!((sizes[1] - 25.0).abs() < 0.01);
            assert!((sizes[2] - 50.0).abs() < 0.01);
        } else {
            panic!("Expected flattened horizontal split");
        }
    }

    #[test]
    fn normalize_different_direction_preserved() {
        // H[V[t1, t2], t3] stays as H[V[t1, t2], t3]
        let mut node = LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            sizes: vec![50.0, 50.0],
            children: vec![
                vsplit(vec![terminal("t1"), terminal("t2")]),
                terminal("t3"),
            ],
        };
        node.normalize();
        if let LayoutNode::Split { children, direction, .. } = &node {
            assert_eq!(*direction, SplitDirection::Horizontal);
            assert_eq!(children.len(), 2);
            assert!(matches!(&children[0], LayoutNode::Split { direction: SplitDirection::Vertical, .. }));
        } else {
            panic!("Expected horizontal split with nested vertical");
        }
    }

    #[test]
    fn normalize_single_child_tabs_unwraps() {
        let mut node = tabs(vec![terminal("t1")]);
        node.normalize();
        assert!(matches!(node, LayoutNode::Terminal { .. }));
    }

    #[test]
    fn normalize_deep_recursive() {
        // H[H[H[t1]]] -> t1
        let mut node = hsplit(vec![hsplit(vec![hsplit(vec![terminal("t1")])])]);
        node.normalize();
        match &node {
            LayoutNode::Terminal { terminal_id, .. } => {
                assert_eq!(terminal_id.as_deref(), Some("t1"));
            }
            _ => panic!("Expected terminal after deep normalize"),
        }
    }

    // === clone_structure ===

    #[test]
    fn clone_structure_clears_ids_preserves_shape() {
        let node = hsplit(vec![
            terminal("t1"),
            tabs(vec![terminal("t2"), terminal("t3")]),
        ]);
        let cloned = node.clone_structure();
        // All IDs should be None
        assert!(cloned.collect_terminal_ids().is_empty());
        // Shape preserved
        match &cloned {
            LayoutNode::Split { children, .. } => {
                assert_eq!(children.len(), 2);
                assert!(matches!(&children[0], LayoutNode::Terminal { .. }));
                assert!(matches!(&children[1], LayoutNode::Tabs { children, .. } if children.len() == 2));
            }
            _ => panic!("Expected split"),
        }
    }

    // === remove_at_path ===

    #[test]
    fn remove_at_path_from_2_child_split_collapses() {
        let mut node = hsplit(vec![terminal("t1"), terminal("t2")]);
        let removed = node.remove_at_path(&[0]);
        assert!(removed.is_some());
        // Parent should collapse to remaining child
        match &node {
            LayoutNode::Terminal { terminal_id, .. } => {
                assert_eq!(terminal_id.as_deref(), Some("t2"));
            }
            _ => panic!("Expected terminal after collapsing 2-child split"),
        }
    }

    #[test]
    fn remove_at_path_from_3_child_split_keeps_2() {
        let mut node = LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            sizes: vec![33.0, 33.0, 34.0],
            children: vec![terminal("t1"), terminal("t2"), terminal("t3")],
        };
        let removed = node.remove_at_path(&[1]);
        assert!(removed.is_some());
        match &node {
            LayoutNode::Split { children, sizes, .. } => {
                assert_eq!(children.len(), 2);
                assert_eq!(sizes.len(), 2);
            }
            _ => panic!("Expected split with 2 children"),
        }
    }

    #[test]
    fn remove_at_path_from_tabs_collapses_if_1() {
        let mut node = tabs(vec![terminal("t1"), terminal("t2")]);
        let removed = node.remove_at_path(&[0]);
        assert!(removed.is_some());
        match &node {
            LayoutNode::Terminal { terminal_id, .. } => {
                assert_eq!(terminal_id.as_deref(), Some("t2"));
            }
            _ => panic!("Expected terminal after collapsing 2-child tabs"),
        }
    }

    #[test]
    fn remove_at_path_invalid_index_returns_none() {
        let mut node = hsplit(vec![terminal("t1"), terminal("t2")]);
        let removed = node.remove_at_path(&[5]);
        assert!(removed.is_none());
    }

    #[test]
    fn remove_at_path_empty_returns_none() {
        let mut node = terminal("t1");
        let removed = node.remove_at_path(&[]);
        assert!(removed.is_none());
    }

    #[test]
    fn remove_at_path_nested() {
        // H[t1, V[t2, t3]] -> remove t2 at [1, 0] -> H[t1, t3]
        let mut node = hsplit(vec![
            terminal("t1"),
            vsplit(vec![terminal("t2"), terminal("t3")]),
        ]);
        let removed = node.remove_at_path(&[1, 0]);
        assert!(removed.is_some());
        match &node {
            LayoutNode::Split { children, .. } => {
                assert_eq!(children.len(), 2);
                // Second child should now be t3 (vsplit collapsed)
                match &children[1] {
                    LayoutNode::Terminal { terminal_id, .. } => {
                        assert_eq!(terminal_id.as_deref(), Some("t3"));
                    }
                    _ => panic!("Expected terminal t3"),
                }
            }
            _ => panic!("Expected split"),
        }
    }

    // === Serialization round-trip ===

    #[test]
    fn serde_round_trip_terminal() {
        let node = terminal("t1");
        let json = serde_json::to_string(&node).unwrap();
        let deserialized: LayoutNode = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.collect_terminal_ids(), vec!["t1"]);
    }

    #[test]
    fn serde_round_trip_complex() {
        let node = hsplit(vec![
            terminal("t1"),
            vsplit(vec![terminal("t2"), terminal("t3")]),
            tabs(vec![terminal("t4"), terminal("t5")]),
        ]);
        let json = serde_json::to_string(&node).unwrap();
        let deserialized: LayoutNode = serde_json::from_str(&json).unwrap();
        assert_eq!(
            deserialized.collect_terminal_ids(),
            vec!["t1", "t2", "t3", "t4", "t5"]
        );
    }
}

#[cfg(test)]
mod workspace_tests {
    use crate::workspace::state::{
        FolderData, LayoutNode, ProjectData, SplitDirection, Workspace, WorkspaceData,
    };
    use crate::terminal::shell_config::ShellType;
    use crate::theme::FolderColor;
    use crate::workspace::settings::HooksConfig;
    use std::collections::HashMap;

    fn make_project(id: &str, visible: bool) -> ProjectData {
        ProjectData {
            id: id.to_string(),
            name: format!("Project {}", id),
            path: "/tmp/test".to_string(),
            is_visible: visible,
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
            folder_color: FolderColor::default(),
            hooks: HooksConfig::default(),
        }
    }

    fn make_workspace_data(projects: Vec<ProjectData>, order: Vec<&str>) -> WorkspaceData {
        WorkspaceData {
            version: 1,
            projects,
            project_order: order.into_iter().map(String::from).collect(),
            project_widths: HashMap::new(),
            folders: Vec::new(),
        }
    }

    #[test]
    fn test_visible_projects_filters_hidden() {
        let data = make_workspace_data(
            vec![make_project("p1", true), make_project("p2", false), make_project("p3", true)],
            vec!["p1", "p2", "p3"],
        );
        let ws = Workspace::new(data);

        let visible = ws.visible_projects();
        assert_eq!(visible.len(), 2);
        assert_eq!(visible[0].id, "p1");
        assert_eq!(visible[1].id, "p3");
    }

    #[test]
    fn test_visible_projects_with_focused_project() {
        let data = make_workspace_data(
            vec![make_project("p1", true), make_project("p2", true), make_project("p3", false)],
            vec!["p1", "p2", "p3"],
        );
        let mut ws = Workspace::new(data);

        // Focus on p3 (which is hidden) — should show only p3
        ws.focus_manager.set_focused_project_id(Some("p3".to_string()));

        let visible = ws.visible_projects();
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].id, "p3");
    }

    #[test]
    fn test_visible_projects_with_folder() {
        let mut data = make_workspace_data(
            vec![make_project("p1", true), make_project("p2", true)],
            vec!["f1"],
        );
        data.folders = vec![FolderData {
            id: "f1".to_string(),
            name: "Folder".to_string(),
            project_ids: vec!["p1".to_string(), "p2".to_string()],
            collapsed: false,
            folder_color: FolderColor::default(),
        }];

        let ws = Workspace::new(data);

        let visible = ws.visible_projects();
        assert_eq!(visible.len(), 2);
        assert_eq!(visible[0].id, "p1");
        assert_eq!(visible[1].id, "p2");
    }

    #[test]
    fn test_projects_by_recency() {
        let data = make_workspace_data(
            vec![make_project("p1", true), make_project("p2", true), make_project("p3", true)],
            vec!["p1", "p2", "p3"],
        );
        let mut ws = Workspace::new(data);

        // Touch p3, then p1 (p1 will be most recent)
        ws.touch_project("p3");
        ws.touch_project("p1");

        let recency = ws.projects_by_recency();
        // p1 (most recent), p3, p2 (never touched)
        assert_eq!(recency[0].id, "p1");
        assert_eq!(recency[1].id, "p3");
        assert_eq!(recency[2].id, "p2");
    }

    #[test]
    fn test_collect_all_detached_terminals() {
        let mut project = make_project("p1", true);
        project.layout = Some(LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            sizes: vec![50.0, 50.0],
            children: vec![
                LayoutNode::Terminal {
                    terminal_id: Some("t1".to_string()),
                    minimized: false,
                    detached: true,
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
        let ws = Workspace::new(data);

        let detached = ws.collect_all_detached_terminals();
        assert_eq!(detached.len(), 1);
        assert_eq!(detached[0].0, "t1");
        assert_eq!(detached[0].1, "p1");
        assert_eq!(detached[0].2, vec![0]);
    }

    #[test]
    fn test_folder_for_project() {
        let mut data = make_workspace_data(
            vec![make_project("p1", true), make_project("p2", true)],
            vec!["f1", "p2"],
        );
        data.folders = vec![FolderData {
            id: "f1".to_string(),
            name: "Folder".to_string(),
            project_ids: vec!["p1".to_string()],
            collapsed: false,
            folder_color: FolderColor::default(),
        }];
        let ws = Workspace::new(data);

        assert_eq!(ws.folder_for_project("p1").unwrap().id, "f1");
        assert!(ws.folder_for_project("p2").is_none());
    }
}

#[cfg(test)]
mod gpui_tests {
    use gpui::AppContext as _;
    use crate::workspace::state::{LayoutNode, ProjectData, SplitDirection, Workspace, WorkspaceData};
    use crate::workspace::settings::HooksConfig;
    use crate::terminal::shell_config::ShellType;
    use crate::theme::FolderColor;
    use std::collections::HashMap;

    fn make_project(id: &str) -> ProjectData {
        ProjectData {
            id: id.to_string(),
            name: format!("Project {}", id),
            path: "/tmp/test".to_string(),
            is_visible: true,
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
            folder_color: FolderColor::default(),
            hooks: HooksConfig::default(),
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
    fn test_with_layout_node_applies_mutation(cx: &mut gpui::TestAppContext) {
        let data = make_workspace_data(vec![make_project("p1")], vec!["p1"]);
        let workspace = cx.new(|_cx| Workspace::new(data));

        let result = workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.with_layout_node("p1", &[], cx, |node| {
                if let LayoutNode::Terminal { minimized, .. } = node {
                    *minimized = true;
                    true
                } else {
                    false
                }
            })
        });
        assert!(result);

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            let layout = ws.project("p1").unwrap().layout.as_ref().unwrap();
            match layout {
                LayoutNode::Terminal { minimized, .. } => assert!(*minimized),
                _ => panic!("Expected terminal"),
            }
            assert_eq!(ws.data_version(), 1);
        });
    }

    #[gpui::test]
    fn test_with_layout_node_invalid_path_returns_false(cx: &mut gpui::TestAppContext) {
        let data = make_workspace_data(vec![make_project("p1")], vec!["p1"]);
        let workspace = cx.new(|_cx| Workspace::new(data));

        let result = workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.with_layout_node("p1", &[99], cx, |_node| true)
        });
        assert!(!result);

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            // Version should NOT have been bumped
            assert_eq!(ws.data_version(), 0);
        });
    }

    #[gpui::test]
    fn test_with_layout_node_invalid_project_returns_false(cx: &mut gpui::TestAppContext) {
        let data = make_workspace_data(vec![make_project("p1")], vec!["p1"]);
        let workspace = cx.new(|_cx| Workspace::new(data));

        let result = workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.with_layout_node("nonexistent", &[], cx, |_node| true)
        });
        assert!(!result);

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert_eq!(ws.data_version(), 0);
        });
    }

    #[gpui::test]
    fn test_with_layout_node_normalized_flattens_nested_split(cx: &mut gpui::TestAppContext) {
        // Start with a horizontal split
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

        // Split t1 horizontally — this creates a nested horizontal split that should flatten
        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.with_layout_node_normalized("p1", &[0], cx, |node| {
                let old = node.clone();
                *node = LayoutNode::Split {
                    direction: SplitDirection::Horizontal,
                    sizes: vec![50.0, 50.0],
                    children: vec![old, LayoutNode::new_terminal()],
                };
                true
            });
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            let layout = ws.project("p1").unwrap().layout.as_ref().unwrap();
            match layout {
                LayoutNode::Split { direction, children, .. } => {
                    assert_eq!(*direction, SplitDirection::Horizontal);
                    // Should be flattened: t1, new_terminal, t2 (3 children, not nested)
                    assert_eq!(children.len(), 3);
                }
                _ => panic!("Expected flattened split"),
            }
        });
    }

    #[gpui::test]
    fn test_replace_data_resets_focus(cx: &mut gpui::TestAppContext) {
        let data = make_workspace_data(vec![make_project("p1")], vec!["p1"]);
        let workspace = cx.new(|_cx| Workspace::new(data));

        // Set focus
        workspace.update(cx, |ws: &mut Workspace, _cx| {
            ws.focus_manager.set_focused_project_id(Some("p1".to_string()));
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert!(ws.focused_project_id().is_some());
        });

        // Replace data
        let new_data = make_workspace_data(vec![make_project("p2")], vec!["p2"]);
        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.replace_data(new_data, cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            // Focus should be cleared
            assert!(ws.focused_project_id().is_none());
            // New data should be in place
            assert_eq!(ws.data().projects.len(), 1);
            assert_eq!(ws.data().projects[0].id, "p2");
        });
    }

    #[gpui::test]
    fn test_visible_projects_gpui(cx: &mut gpui::TestAppContext) {
        let mut p1 = make_project("p1");
        let p2 = make_project("p2");
        let mut p3 = make_project("p3");
        p1.is_visible = false;
        p3.is_visible = false;
        let data = make_workspace_data(vec![p1, p2, p3], vec!["p1", "p2", "p3"]);
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            let visible = ws.visible_projects();
            assert_eq!(visible.len(), 1);
            assert_eq!(visible[0].id, "p2");
        });

        // After toggling visibility
        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.toggle_project_visibility("p1", cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            let visible = ws.visible_projects();
            assert_eq!(visible.len(), 2);
            assert_eq!(visible[0].id, "p1");
            assert_eq!(visible[1].id, "p2");
        });
    }
}
