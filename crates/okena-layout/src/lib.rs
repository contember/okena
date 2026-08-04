#![cfg_attr(not(test), warn(clippy::unwrap_used, clippy::expect_used))]

//! okena-layout — Layout tree algorithms.
//!
//! The `LayoutNode` recursive enum models terminal panes as a tree of
//! `Terminal`, `Split`, and `Tabs` nodes. This crate owns the type and all
//! pure tree algorithms (navigation, mutation, normalization, structure
//! merging) — no GPUI, no workspace state, no hook execution.

use okena_core::shell::ShellType;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

pub use okena_core::types::SplitDirection;

fn default_zoom_level() -> f32 {
    1.0
}

/// Recursive layout tree node
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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

impl LayoutNode {
    /// Returns true if this node is effectively hidden (all terminals within it are minimized or detached).
    pub fn is_all_hidden(&self) -> bool {
        match self {
            LayoutNode::Terminal {
                minimized,
                detached,
                ..
            } => *minimized || *detached,
            LayoutNode::Split { children, .. } | LayoutNode::Tabs { children, .. } => {
                children.iter().all(|c| c.is_all_hidden())
            }
        }
    }

    /// Replace a terminal ID in the layout tree (for hook rerun).
    pub fn replace_terminal_id(&mut self, old_id: &str, new_id: &str) {
        match self {
            LayoutNode::Terminal { terminal_id, .. } => {
                if terminal_id.as_deref() == Some(old_id) {
                    *terminal_id = Some(new_id.to_string());
                }
            }
            LayoutNode::Split { children, .. } | LayoutNode::Tabs { children, .. } => {
                for child in children {
                    child.replace_terminal_id(old_id, new_id);
                }
            }
        }
    }

    /// Remove terminal leaves with matching IDs, collapsing their parents.
    ///
    /// The root is wrapped in `Option` because removing its last terminal leaves
    /// no representable layout node. Returns the number of removed leaves.
    pub fn remove_terminal_ids(layout: &mut Option<Self>, terminal_ids: &HashSet<&str>) -> usize {
        let mut removed = 0;
        for terminal_id in terminal_ids {
            let Some(root) = layout.as_mut() else {
                break;
            };
            let Some(path) = root.find_terminal_path(terminal_id) else {
                continue;
            };
            if path.is_empty() {
                *layout = None;
                removed += 1;
            } else if root.remove_at_path(&path).is_some() {
                removed += 1;
            }
        }
        removed
    }

    /// Recursively flip every `Split` in this subtree between horizontal and
    /// vertical. `Tabs` and `Terminal` nodes are unaffected (tabs have no
    /// orientation), but the walk descends into both so nested splits inside
    /// tab groups are transposed too. `sizes` are preserved as-is — they are
    /// fractions along the split axis, so a 60/40 horizontal split becomes a
    /// 60/40 vertical split.
    pub fn transpose(&mut self) {
        match self {
            LayoutNode::Terminal { .. } => {}
            LayoutNode::Split {
                direction,
                children,
                ..
            } => {
                *direction = direction.flipped();
                for child in children {
                    child.transpose();
                }
            }
            LayoutNode::Tabs { children, .. } => {
                for child in children {
                    child.transpose();
                }
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

    /// Create a terminal node that runs a specific command with env vars
    pub fn new_terminal_with_command(
        command: &str,
        env_vars: &std::collections::HashMap<String, String>,
    ) -> Self {
        let env_prefix = env_vars
            .iter()
            .map(|(k, v)| format!("{}='{}'", k, v.replace('\'', "'\\''")))
            .collect::<Vec<_>>()
            .join(" ");
        let full_cmd = if env_prefix.is_empty() {
            command.to_string()
        } else {
            format!("{} {}", env_prefix, command)
        };

        LayoutNode::Terminal {
            terminal_id: None,
            minimized: false,
            detached: false,
            shell_type: ShellType::for_command(full_cmd),
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

    /// Whether any leaf still carries a terminal ID. Allocation-free counterpart
    /// to `collect_terminal_ids().is_empty()` for render-path checks.
    pub fn has_terminal_ids(&self) -> bool {
        match self {
            LayoutNode::Terminal { terminal_id, .. } => terminal_id.is_some(),
            LayoutNode::Split { children, .. } | LayoutNode::Tabs { children, .. } => {
                children.iter().any(LayoutNode::has_terminal_ids)
            }
        }
    }

    /// Clear terminal IDs except those in the `keep` set (e.g. hook terminals).
    /// Kept terminals preserve their ID, minimized, and detached state.
    pub fn clear_terminal_ids_except(&mut self, keep: &HashSet<&str>) {
        match self {
            LayoutNode::Terminal {
                terminal_id,
                minimized,
                detached,
                ..
            } => {
                let should_keep = terminal_id.as_deref().is_some_and(|id| keep.contains(id));
                if !should_keep {
                    *terminal_id = None;
                    *minimized = false;
                    *detached = false;
                }
            }
            LayoutNode::Split { children, .. } | LayoutNode::Tabs { children, .. } => {
                for child in children {
                    child.clear_terminal_ids_except(keep);
                }
            }
        }
    }

    /// Find the layout path to a terminal by its ID
    pub fn find_terminal_path(&self, target_id: &str) -> Option<Vec<usize>> {
        self.find_terminal_path_recursive(target_id, vec![])
    }

    fn find_terminal_path_recursive(
        &self,
        target_id: &str,
        current_path: Vec<usize>,
    ) -> Option<Vec<usize>> {
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
                    if let Some(found_path) =
                        child.find_terminal_path_recursive(target_id, child_path)
                    {
                        return Some(found_path);
                    }
                }
                None
            }
        }
    }

    /// Terminal that should take focus once the node at `path` is closed.
    ///
    /// Mirrors what [`Self::remove_at_path`] will do to the tree, one level at
    /// a time:
    /// - **Tabs parent** — hand focus to whichever tab becomes active after the
    ///   removal (the next tab, or the previous one when the last tab closes).
    /// - **Split parent** — hand focus to the neighbouring pane (previous, or
    ///   next when closing the first).
    /// - **Container left with a single child** — it collapses into that child,
    ///   so the rule re-applies one level up (closing the last tab of a group
    ///   lands on the neighbouring pane, not inside the dying group).
    ///
    /// Returns `None` when the closed node is the whole tree (nothing left to
    /// focus) or when the resolved terminal has no id yet (still spawning).
    pub fn terminal_to_focus_after_closing(&self, path: &[usize]) -> Option<String> {
        let (&child_index, parent_path) = path.split_last()?;
        let children = match self.get_at_path(parent_path)? {
            LayoutNode::Terminal { .. } => return None,
            LayoutNode::Split { children, .. } | LayoutNode::Tabs { children, .. } => children,
        };
        if child_index >= children.len() {
            return None;
        }
        if children.len() <= 2 {
            // The parent collapses into its surviving child, which then sits at
            // `parent_path`. With exactly one sibling that sibling IS the
            // target; with none, the whole container disappears and the rule
            // re-applies one level up.
            return match children.len() {
                2 => children[1 - child_index].visible_terminal_id(),
                _ => self.terminal_to_focus_after_closing(parent_path),
            };
        }

        // Index (in the *pre-removal* child list) of the node that ends up
        // focused once the removal lands.
        let sibling_index = match self.get_at_path(parent_path)? {
            LayoutNode::Tabs { active_tab, .. } if *active_tab != child_index => *active_tab,
            // The active tab is the one going away: the next tab slides into
            // its index, except for the last tab where `remove_at_path` clamps
            // `active_tab` back onto the previous one.
            LayoutNode::Tabs { .. } if child_index + 1 < children.len() => child_index + 1,
            LayoutNode::Tabs { .. } => child_index - 1,
            // Splits keep every pane visible — pick the neighbour.
            _ if child_index > 0 => child_index - 1,
            _ => 1,
        };
        children.get(sibling_index)?.visible_terminal_id()
    }

    /// Id of the terminal that is visible in this subtree (follows active tabs).
    /// `None` for a pane whose PTY hasn't been assigned an id yet.
    pub fn visible_terminal_id(&self) -> Option<String> {
        let path = self.find_visible_terminal_path();
        match self.get_at_path(&path)? {
            LayoutNode::Terminal { terminal_id, .. } => terminal_id.clone(),
            _ => None,
        }
    }

    /// Find the `Terminal` node with the given id, returning a reference to it.
    /// Unlike `find_terminal_path`, this hands back the node itself so callers
    /// can clone its visual state (shell_type, zoom_level) — used by soft-close
    /// undo to reconstruct a closed pane.
    pub fn find_terminal_node(&self, target_id: &str) -> Option<&LayoutNode> {
        match self {
            LayoutNode::Terminal { terminal_id, .. } => {
                if terminal_id.as_deref() == Some(target_id) {
                    Some(self)
                } else {
                    None
                }
            }
            LayoutNode::Split { children, .. } | LayoutNode::Tabs { children, .. } => children
                .iter()
                .find_map(|c| c.find_terminal_node(target_id)),
        }
    }

    /// Append a node as a sibling at the root of this layout.
    ///
    /// Used by soft-close undo when the original position can no longer be
    /// restored (the tree changed during the grace window): rather than guess a
    /// merge, the recovered terminal is dropped into the top-level group.
    /// - Split: pushed as a new child, sizes rebalanced equally.
    /// - Tabs: pushed as a new tab and activated.
    /// - bare Terminal: wrapped together with `node` into a horizontal Split.
    pub fn append_to_root(&mut self, node: LayoutNode) {
        match self {
            LayoutNode::Split {
                children, sizes, ..
            } => {
                children.push(node);
                let n = children.len();
                *sizes = vec![1.0 / n as f32; n];
            }
            LayoutNode::Tabs {
                children,
                active_tab,
            } => {
                children.push(node);
                *active_tab = children.len() - 1;
            }
            LayoutNode::Terminal { .. } => {
                let existing = std::mem::replace(
                    self,
                    LayoutNode::Tabs {
                        children: Vec::new(),
                        active_tab: 0,
                    },
                );
                *self = LayoutNode::Split {
                    direction: SplitDirection::Horizontal,
                    sizes: vec![0.5, 0.5],
                    children: vec![existing, node],
                };
            }
        }
    }

    /// Collect terminal IDs that are behind a non-active tab.
    /// A terminal is "inactive" if any ancestor Tabs node has it in a non-active child.
    pub fn collect_inactive_tab_terminal_ids(&self) -> HashSet<String> {
        let mut result = HashSet::new();
        self.collect_inactive_tabs_recursive(&mut result, false);
        result
    }

    fn collect_inactive_tabs_recursive(
        &self,
        result: &mut HashSet<String>,
        is_behind_inactive_tab: bool,
    ) {
        match self {
            LayoutNode::Terminal { terminal_id, .. } => {
                if is_behind_inactive_tab && let Some(id) = terminal_id {
                    result.insert(id.clone());
                }
            }
            LayoutNode::Split { children, .. } => {
                for child in children {
                    child.collect_inactive_tabs_recursive(result, is_behind_inactive_tab);
                }
            }
            LayoutNode::Tabs {
                children,
                active_tab,
            } => {
                for (i, child) in children.iter().enumerate() {
                    let inactive = is_behind_inactive_tab || i != *active_tab;
                    child.collect_inactive_tabs_recursive(result, inactive);
                }
            }
        }
    }

    /// Collect terminal IDs that belong to a Tabs node with 2+ children.
    /// These terminals are visually grouped in the sidebar with a vertical line.
    pub fn collect_tab_group_terminal_ids(&self) -> HashSet<String> {
        let mut result = HashSet::new();
        self.collect_tab_group_recursive(&mut result, false);
        result
    }

    fn collect_tab_group_recursive(&self, result: &mut HashSet<String>, inside_tab_group: bool) {
        match self {
            LayoutNode::Terminal { terminal_id, .. } => {
                if inside_tab_group && let Some(id) = terminal_id {
                    result.insert(id.clone());
                }
            }
            LayoutNode::Split { children, .. } => {
                for child in children {
                    child.collect_tab_group_recursive(result, inside_tab_group);
                }
            }
            LayoutNode::Tabs { children, .. } => {
                let is_group = children.len() >= 2;
                for child in children {
                    child.collect_tab_group_recursive(result, is_group || inside_tab_group);
                }
            }
        }
    }

    /// Activate tabs along the given path so the target terminal becomes visible.
    /// For each Tabs node encountered along the path, sets its active_tab to the
    /// path index that leads toward the target.
    pub fn activate_tabs_along_path(&mut self, path: &[usize]) {
        if path.is_empty() {
            return;
        }
        match self {
            LayoutNode::Terminal { .. } => {}
            LayoutNode::Split { children, .. } => {
                if let Some(child) = children.get_mut(path[0]) {
                    child.activate_tabs_along_path(&path[1..]);
                }
            }
            LayoutNode::Tabs {
                children,
                active_tab,
            } => {
                *active_tab = path[0];
                if let Some(child) = children.get_mut(path[0]) {
                    child.activate_tabs_along_path(&path[1..]);
                }
            }
        }
    }

    /// Collect all minimized terminal IDs in this layout tree
    pub fn collect_minimized_terminals(&self) -> Vec<(String, Vec<usize>)> {
        let mut result = Vec::new();
        self.collect_minimized_recursive(&mut result, vec![]);
        result
    }

    fn collect_minimized_recursive(
        &self,
        result: &mut Vec<(String, Vec<usize>)>,
        current_path: Vec<usize>,
    ) {
        match self {
            LayoutNode::Terminal {
                terminal_id,
                minimized,
                ..
            } => {
                if *minimized && let Some(id) = terminal_id {
                    result.push((id.clone(), current_path));
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

    fn collect_detached_recursive(
        &self,
        result: &mut Vec<(String, Vec<usize>)>,
        current_path: Vec<usize>,
    ) {
        match self {
            LayoutNode::Terminal {
                terminal_id,
                detached,
                ..
            } => {
                if *detached && let Some(id) = terminal_id {
                    result.push((id.clone(), current_path));
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

    fn find_uninitialized_terminal_path_recursive(
        &self,
        current_path: Vec<usize>,
    ) -> Option<Vec<usize>> {
        match self {
            LayoutNode::Terminal {
                terminal_id: None, ..
            } => Some(current_path),
            LayoutNode::Terminal { .. } => None,
            LayoutNode::Split { children, .. } | LayoutNode::Tabs { children, .. } => {
                for (i, child) in children.iter().enumerate() {
                    let mut child_path = current_path.clone();
                    child_path.push(i);
                    if let Some(path) = child.find_uninitialized_terminal_path_recursive(child_path)
                    {
                        return Some(path);
                    }
                }
                None
            }
        }
    }

    /// Find the path to the first terminal in this layout subtree
    pub fn find_first_terminal_path(&self) -> Vec<usize> {
        self.find_terminal_path_by_strategy(false)
    }

    /// Find path to the first visible terminal (follows active tabs).
    pub fn find_visible_terminal_path(&self) -> Vec<usize> {
        self.find_terminal_path_by_strategy(true)
    }

    /// Shared implementation: when `follow_active_tab` is true, Tabs nodes
    /// pick the active child; otherwise they always pick child 0.
    fn find_terminal_path_by_strategy(&self, follow_active_tab: bool) -> Vec<usize> {
        self.find_terminal_path_recursive_impl(vec![], follow_active_tab)
    }

    fn find_terminal_path_recursive_impl(
        &self,
        current_path: Vec<usize>,
        follow_active_tab: bool,
    ) -> Vec<usize> {
        match self {
            LayoutNode::Terminal { .. } => current_path,
            LayoutNode::Split { children, .. } => {
                if let Some(first_child) = children.first() {
                    let mut child_path = current_path;
                    child_path.push(0);
                    first_child.find_terminal_path_recursive_impl(child_path, follow_active_tab)
                } else {
                    current_path
                }
            }
            LayoutNode::Tabs {
                children,
                active_tab,
                ..
            } => {
                let idx = if follow_active_tab {
                    (*active_tab).min(children.len().saturating_sub(1))
                } else {
                    0
                };
                if let Some(child) = children.get(idx) {
                    let mut child_path = current_path;
                    child_path.push(idx);
                    child.find_terminal_path_recursive_impl(child_path, follow_active_tab)
                } else {
                    current_path
                }
            }
        }
    }

    /// Remove a child node at the given path.
    /// For split parents, transfers removed weight to the previous sibling, falling back to the next.
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
            LayoutNode::Split {
                children, sizes, ..
            } => {
                if child_index >= children.len() {
                    return None;
                }
                let removed = children.remove(child_index);
                if child_index < sizes.len() {
                    let removed_size = sizes.remove(child_index);
                    let recipient = child_index.saturating_sub(1);
                    if let Some(size) = sizes.get_mut(recipient) {
                        *size += removed_size;
                    }
                }
                if children.len() == 1 {
                    let remaining = children.remove(0);
                    *parent = remaining;
                }
                Some(removed)
            }
            LayoutNode::Tabs {
                children,
                active_tab,
            } => {
                if child_index >= children.len() {
                    return None;
                }
                let removed = children.remove(child_index);
                if *active_tab >= children.len() {
                    *active_tab = children.len().saturating_sub(1);
                }
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
        match self {
            LayoutNode::Terminal { .. } => return,
            LayoutNode::Split { children, .. } | LayoutNode::Tabs { children, .. } => {
                for child in children.iter_mut() {
                    child.normalize();
                }
            }
        }

        if let LayoutNode::Tabs {
            children,
            active_tab,
        } = self
        {
            *active_tab = (*active_tab).min(children.len().saturating_sub(1));
        }

        if let LayoutNode::Split {
            sizes, children, ..
        } = self
            && sizes.len() != children.len()
        {
            sizes.truncate(children.len());
            while sizes.len() < children.len() {
                sizes.push(100.0 / children.len() as f32);
            }
        }

        // Sizes are relative weights — the tiny-pair threshold is 10% of the total
        // sum so the check works regardless of overall scale.
        if let LayoutNode::Split {
            sizes, children, ..
        } = self
        {
            let has_invalid = sizes.iter().any(|s| *s <= 0.0 || !s.is_finite());
            let total: f32 = sizes.iter().sum();
            let min_resize = total * 0.1;
            let has_tiny_pair = sizes.windows(2).any(|w| w[0] + w[1] <= min_resize);
            if has_invalid || has_tiny_pair {
                log::warn!(
                    "Layout has invalid/too-small sizes {:?}, resetting to equal",
                    sizes
                );
                let equal = 100.0 / children.len() as f32;
                for s in sizes.iter_mut() {
                    *s = equal;
                }
            }
        }

        let should_unwrap = match self {
            LayoutNode::Split { children, .. } | LayoutNode::Tabs { children, .. } => {
                children.len() <= 1
            }
            _ => false,
        };
        if should_unwrap {
            match self {
                LayoutNode::Split { children, .. } | LayoutNode::Tabs { children, .. } => {
                    if children.len() == 1 {
                        *self = children.remove(0);
                    } else {
                        *self = LayoutNode::new_terminal();
                    }
                }
                _ => {}
            }
            return;
        }

        if let LayoutNode::Split {
            direction,
            sizes,
            children,
        } = self
        {
            let has_same_dir_child = children
                .iter()
                .any(|c| matches!(c, LayoutNode::Split { direction: d, .. } if d == direction));
            if has_same_dir_child {
                let dir = *direction;
                let mut new_children = Vec::new();
                let mut new_sizes = Vec::new();

                for (i, child) in children.drain(..).enumerate() {
                    let parent_size = sizes[i];
                    match child {
                        LayoutNode::Split {
                            direction: child_dir,
                            sizes: child_sizes,
                            children: grandchildren,
                        } if child_dir == dir => {
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

    /// Clone the layout structure but clear all terminal IDs.
    /// Used when creating worktree projects to duplicate layout with fresh terminals.
    pub fn clone_structure(&self) -> Self {
        match self {
            LayoutNode::Terminal {
                shell_type,
                zoom_level,
                ..
            } => LayoutNode::Terminal {
                terminal_id: None,
                minimized: false,
                detached: false,
                shell_type: shell_type.clone(),
                zoom_level: *zoom_level,
            },
            LayoutNode::Split {
                direction,
                sizes,
                children,
            } => LayoutNode::Split {
                direction: *direction,
                sizes: sizes.clone(),
                children: children.iter().map(|c| c.clone_structure()).collect(),
            },
            LayoutNode::Tabs {
                children,
                active_tab,
            } => LayoutNode::Tabs {
                children: children.iter().map(|c| c.clone_structure()).collect(),
                active_tab: *active_tab,
            },
        }
    }

    /// Merge server layout structure with locally-preserved visual state.
    ///
    /// Takes the structural layout from `server` (terminals, splits, tabs) but
    /// preserves local visual state from `local` where the structure matches.
    /// Children and selected tabs are reconciled by terminal identity so a
    /// daemon-side reorder cannot attach presentation to a different pane.
    pub fn merge_visual_state(server: &LayoutNode, local: &LayoutNode) -> LayoutNode {
        let mut result = LayoutNode::merge_container_visual_state(server, local);
        let mut visual_states = HashMap::new();
        local.collect_terminal_visual_state(&mut visual_states);
        result.apply_terminal_visual_state(&visual_states);
        result
    }

    fn merge_container_visual_state(server: &LayoutNode, local: &LayoutNode) -> LayoutNode {
        match (server, local) {
            (LayoutNode::Terminal { .. }, _) => server.clone(),
            (
                LayoutNode::Split {
                    direction: s_dir,
                    sizes: s_sizes,
                    children: s_children,
                },
                LayoutNode::Split {
                    direction: l_dir,
                    sizes: l_sizes,
                    children: l_children,
                    ..
                },
            ) if s_dir == l_dir => {
                let mapping = LayoutNode::matching_child_indices(s_children, l_children);
                let merged_children =
                    LayoutNode::merge_mapped_children(s_children, l_children, mapping.as_deref());
                let sizes = mapping
                    .filter(|indices| l_sizes.len() == indices.len())
                    .map(|indices| indices.into_iter().map(|index| l_sizes[index]).collect())
                    .unwrap_or_else(|| s_sizes.clone());
                LayoutNode::Split {
                    direction: *s_dir,
                    sizes,
                    children: merged_children,
                }
            }
            (
                LayoutNode::Tabs {
                    children: s_children,
                    active_tab: s_active,
                },
                LayoutNode::Tabs {
                    children: l_children,
                    active_tab: l_active,
                    ..
                },
            ) => {
                let mapping = LayoutNode::matching_child_indices(s_children, l_children);
                let merged_children =
                    LayoutNode::merge_mapped_children(s_children, l_children, mapping.as_deref());
                LayoutNode::Tabs {
                    children: merged_children,
                    active_tab: LayoutNode::merged_active_tab(
                        s_children, *s_active, l_children, *l_active,
                    ),
                }
            }
            _ => server.clone(),
        }
    }

    fn merge_mapped_children(
        server: &[LayoutNode],
        local: &[LayoutNode],
        mapping: Option<&[usize]>,
    ) -> Vec<LayoutNode> {
        server
            .iter()
            .enumerate()
            .map(|(server_index, server_child)| {
                mapping
                    .and_then(|indices| indices.get(server_index))
                    .and_then(|local_index| local.get(*local_index))
                    .map(|local_child| {
                        LayoutNode::merge_container_visual_state(server_child, local_child)
                    })
                    .unwrap_or_else(|| server_child.clone())
            })
            .collect()
    }

    /// Map every server child to the corresponding local child. Exact subtree
    /// identities handle reorder; positional overlap handles a child that grew.
    fn matching_child_indices(server: &[LayoutNode], local: &[LayoutNode]) -> Option<Vec<usize>> {
        let server_ids: Vec<HashSet<String>> = server
            .iter()
            .map(|child| child.collect_terminal_ids().into_iter().collect())
            .collect();
        let local_ids: Vec<HashSet<String>> = local
            .iter()
            .map(|child| child.collect_terminal_ids().into_iter().collect())
            .collect();

        let mut used = HashSet::new();
        let exact: Option<Vec<usize>> = server_ids
            .iter()
            .map(|ids| {
                if ids.is_empty() {
                    return None;
                }
                let index = local_ids
                    .iter()
                    .enumerate()
                    .find(|(index, candidate)| !used.contains(index) && *candidate == ids)
                    .map(|(index, _)| index)?;
                used.insert(index);
                Some(index)
            })
            .collect();
        if exact.is_some() {
            return exact;
        }

        server_ids
            .iter()
            .zip(&local_ids)
            .all(|(server, local)| {
                (server.is_empty() && local.is_empty())
                    || server.iter().any(|id| local.contains(id))
            })
            .then(|| (0..server.len()).collect())
    }

    fn merged_active_tab(
        server_children: &[LayoutNode],
        server_active: usize,
        local_children: &[LayoutNode],
        local_active: usize,
    ) -> usize {
        let fallback = server_active.min(server_children.len().saturating_sub(1));
        let Some(local_child) = local_children.get(local_active) else {
            return fallback;
        };
        let selected_ids: HashSet<String> =
            local_child.collect_terminal_ids().into_iter().collect();
        if selected_ids.is_empty() {
            return local_active.min(server_children.len().saturating_sub(1));
        }

        server_children
            .iter()
            .position(|child| {
                child
                    .collect_terminal_ids()
                    .iter()
                    .any(|id| selected_ids.contains(id))
            })
            .unwrap_or(fallback)
    }

    /// Collect client-owned terminal presentation from this tree.
    fn collect_terminal_visual_state(&self, states: &mut HashMap<String, (bool, bool, f32)>) {
        match self {
            LayoutNode::Terminal {
                terminal_id: Some(id),
                minimized,
                detached,
                zoom_level,
                ..
            } => {
                states.insert(id.clone(), (*minimized, *detached, *zoom_level));
            }
            LayoutNode::Split { children, .. } | LayoutNode::Tabs { children, .. } => {
                for child in children {
                    child.collect_terminal_visual_state(states);
                }
            }
            _ => {}
        }
    }

    /// Apply client-owned presentation to matching terminals.
    fn apply_terminal_visual_state(&mut self, states: &HashMap<String, (bool, bool, f32)>) {
        match self {
            LayoutNode::Terminal {
                terminal_id: Some(id),
                minimized,
                detached,
                zoom_level,
                ..
            } => {
                if let Some(&(m, d, zoom)) = states.get(id) {
                    *minimized = m;
                    *detached = d;
                    *zoom_level = zoom;
                }
            }
            LayoutNode::Split { children, .. } | LayoutNode::Tabs { children, .. } => {
                for child in children {
                    child.apply_terminal_visual_state(states);
                }
            }
            _ => {}
        }
    }

    /// Convert from API layout node.
    #[allow(dead_code)]
    pub fn from_api(api: &okena_core::api::ApiLayoutNode) -> Self {
        match api {
            okena_core::api::ApiLayoutNode::Terminal {
                terminal_id,
                minimized,
                detached,
                shell_type,
                ..
            } => LayoutNode::Terminal {
                terminal_id: terminal_id.clone(),
                minimized: *minimized,
                detached: *detached,
                shell_type: shell_type.clone(),
                zoom_level: 1.0,
            },
            okena_core::api::ApiLayoutNode::Split {
                direction,
                sizes,
                children,
            } => LayoutNode::Split {
                direction: *direction,
                sizes: sizes.clone(),
                children: children.iter().map(LayoutNode::from_api).collect(),
            },
            okena_core::api::ApiLayoutNode::Tabs {
                children,
                active_tab,
            } => LayoutNode::Tabs {
                children: children.iter().map(LayoutNode::from_api).collect(),
                active_tab: *active_tab,
            },
        }
    }

    /// Convert from API, prefixing all terminal IDs with the given prefix.
    /// Used for remote projects where terminals are registered with prefixed IDs.
    pub fn from_api_prefixed(api: &okena_core::api::ApiLayoutNode, prefix: &str) -> Self {
        match api {
            okena_core::api::ApiLayoutNode::Terminal {
                terminal_id,
                minimized,
                detached,
                shell_type,
                ..
            } => LayoutNode::Terminal {
                terminal_id: terminal_id.as_ref().map(|id| format!("{}:{}", prefix, id)),
                minimized: *minimized,
                detached: *detached,
                shell_type: shell_type.clone(),
                zoom_level: 1.0,
            },
            okena_core::api::ApiLayoutNode::Split {
                direction,
                sizes,
                children,
            } => LayoutNode::Split {
                direction: *direction,
                sizes: sizes.clone(),
                children: children
                    .iter()
                    .map(|c| LayoutNode::from_api_prefixed(c, prefix))
                    .collect(),
            },
            okena_core::api::ApiLayoutNode::Tabs {
                children,
                active_tab,
            } => LayoutNode::Tabs {
                children: children
                    .iter()
                    .map(|c| LayoutNode::from_api_prefixed(c, prefix))
                    .collect(),
                active_tab: *active_tab,
            },
        }
    }

    /// Convert to API layout node.
    pub fn to_api(&self) -> okena_core::api::ApiLayoutNode {
        self.to_api_with_sizes(&std::collections::HashMap::new())
    }

    /// Convert to API, populating terminal `cols`/`rows` from the given size map.
    pub fn to_api_with_sizes(
        &self,
        sizes: &std::collections::HashMap<String, (u16, u16)>,
    ) -> okena_core::api::ApiLayoutNode {
        match self {
            LayoutNode::Terminal {
                terminal_id,
                minimized,
                detached,
                shell_type,
                ..
            } => {
                let (cols, rows) = terminal_id
                    .as_ref()
                    .and_then(|id| sizes.get(id))
                    .map(|&(c, r)| (Some(c), Some(r)))
                    .unwrap_or((None, None));
                okena_core::api::ApiLayoutNode::Terminal {
                    terminal_id: terminal_id.clone(),
                    minimized: *minimized,
                    detached: *detached,
                    shell_type: shell_type.clone(),
                    cols,
                    rows,
                }
            }
            LayoutNode::Split {
                direction,
                sizes: split_sizes,
                children,
            } => okena_core::api::ApiLayoutNode::Split {
                direction: *direction,
                sizes: split_sizes.clone(),
                children: children
                    .iter()
                    .map(|c| c.to_api_with_sizes(sizes))
                    .collect(),
            },
            LayoutNode::Tabs {
                children,
                active_tab,
            } => okena_core::api::ApiLayoutNode::Tabs {
                children: children
                    .iter()
                    .map(|c| c.to_api_with_sizes(sizes))
                    .collect(),
                active_tab: *active_tab,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{LayoutNode, SplitDirection};
    use okena_core::shell::ShellType;
    use std::collections::HashSet;

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

    #[test]
    fn transpose_flips_nested_splits_through_tabs() {
        // Every Split flips H<->V recursively, descending through Tabs (which
        // have no orientation of their own). Terminals are untouched.
        let mut tree = hsplit(vec![
            terminal("a"),
            LayoutNode::Tabs {
                children: vec![vsplit(vec![terminal("b"), terminal("c")])],
                active_tab: 0,
            },
        ]);
        tree.transpose();

        let LayoutNode::Split {
            direction,
            children,
            ..
        } = &tree
        else {
            panic!("expected split");
        };
        assert_eq!(*direction, SplitDirection::Vertical);
        let LayoutNode::Tabs {
            children: tab_children,
            active_tab,
        } = &children[1]
        else {
            panic!("expected tabs");
        };
        assert_eq!(*active_tab, 0, "tab structure preserved");
        let LayoutNode::Split {
            direction: inner, ..
        } = &tab_children[0]
        else {
            panic!("expected nested split inside tabs");
        };
        assert_eq!(*inner, SplitDirection::Horizontal, "nested split flipped");
    }

    #[test]
    fn find_terminal_node_returns_node_with_visual_state() {
        let tree = hsplit(vec![
            terminal("a"),
            LayoutNode::Terminal {
                terminal_id: Some("b".to_string()),
                minimized: false,
                detached: false,
                shell_type: ShellType::Default,
                zoom_level: 2.5,
            },
        ]);
        let node = tree.find_terminal_node("b").expect("b present");
        let LayoutNode::Terminal { zoom_level, .. } = node else {
            panic!("expected terminal");
        };
        assert_eq!(*zoom_level, 2.5);
        assert!(tree.find_terminal_node("missing").is_none());
    }

    #[test]
    fn append_to_root_split_pushes_and_rebalances() {
        let mut tree = hsplit(vec![terminal("a"), terminal("b")]);
        tree.append_to_root(terminal("c"));
        let LayoutNode::Split {
            children, sizes, ..
        } = &tree
        else {
            panic!("expected split");
        };
        assert_eq!(children.len(), 3);
        assert_eq!(sizes.len(), 3);
        for s in sizes {
            assert!((s - 1.0 / 3.0).abs() < 1e-6, "sizes rebalanced equally");
        }
        assert_eq!(tree.find_terminal_path("c"), Some(vec![2]));
    }

    #[test]
    fn append_to_root_tabs_pushes_and_activates() {
        let mut tree = LayoutNode::Tabs {
            children: vec![terminal("a"), terminal("b")],
            active_tab: 0,
        };
        tree.append_to_root(terminal("c"));
        let LayoutNode::Tabs {
            children,
            active_tab,
        } = &tree
        else {
            panic!("expected tabs");
        };
        assert_eq!(children.len(), 3);
        assert_eq!(*active_tab, 2, "new tab activated");
    }

    #[test]
    fn append_to_root_bare_terminal_wraps_in_split() {
        let mut tree = terminal("a");
        tree.append_to_root(terminal("b"));
        let LayoutNode::Split {
            children, sizes, ..
        } = &tree
        else {
            panic!("expected split after wrapping a bare terminal");
        };
        assert_eq!(children.len(), 2);
        assert_eq!(sizes.len(), 2);
        assert_eq!(tree.find_terminal_path("a"), Some(vec![0]));
        assert_eq!(tree.find_terminal_path("b"), Some(vec![1]));
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

    fn tabs_active(children: Vec<LayoutNode>, active_tab: usize) -> LayoutNode {
        LayoutNode::Tabs {
            children,
            active_tab,
        }
    }

    /// For a tab group, `terminal_to_focus_after_closing` must name exactly the
    /// tab the user will be looking at once the close lands — otherwise the
    /// client focuses a pane that isn't on screen. Replays the daemon's own
    /// mutation (`Workspace::close_terminal`: `remove_at_path` plus the
    /// active-tab fix-up for a tab removed before the active one) and reads
    /// back what became visible where the group used to be.
    fn assert_matches_removal(layout: &LayoutNode, path: &[usize]) {
        let predicted = layout.terminal_to_focus_after_closing(path);
        let (&child_index, parent_path) = path.split_last().expect("path must name a child");

        let mut after = layout.clone();
        let prev_active = match after.get_at_path(parent_path) {
            Some(LayoutNode::Tabs { active_tab, .. }) if child_index < *active_tab => {
                Some(*active_tab)
            }
            Some(LayoutNode::Tabs { .. }) => None,
            _ => panic!("oracle only models Tabs parents"),
        };
        after.remove_at_path(path);
        if let Some(prev_active) = prev_active
            && let Some(LayoutNode::Tabs {
                children,
                active_tab,
            }) = after.get_at_path_mut(parent_path)
        {
            *active_tab = (prev_active - 1).min(children.len().saturating_sub(1));
        }

        let surviving = after
            .get_at_path(parent_path)
            .expect("the parent slot survives the removal");
        assert_eq!(
            predicted,
            surviving.visible_terminal_id(),
            "focus prediction disagrees with the post-removal tree for path {path:?}"
        );
    }

    #[test]
    fn focus_after_closing_active_tab_follows_the_next_tab() {
        let layout = tabs(vec![terminal("t1"), terminal("t2"), terminal("t3")]);
        assert_eq!(
            layout.terminal_to_focus_after_closing(&[0]),
            Some("t2".to_string())
        );
        assert_matches_removal(&layout, &[0]);
    }

    #[test]
    fn focus_after_closing_last_active_tab_falls_back_to_previous() {
        let layout = tabs_active(vec![terminal("t1"), terminal("t2"), terminal("t3")], 2);
        assert_eq!(
            layout.terminal_to_focus_after_closing(&[2]),
            Some("t2".to_string())
        );
        assert_matches_removal(&layout, &[2]);
    }

    #[test]
    fn focus_after_closing_background_tab_keeps_the_active_one() {
        let layout = tabs_active(vec![terminal("t1"), terminal("t2"), terminal("t3")], 1);
        // Closing a tab before the active one shifts indices but not what the
        // user is looking at.
        assert_eq!(
            layout.terminal_to_focus_after_closing(&[0]),
            Some("t2".to_string())
        );
        assert_matches_removal(&layout, &[0]);
        // ...and closing one after it changes nothing at all.
        assert_eq!(
            layout.terminal_to_focus_after_closing(&[2]),
            Some("t2".to_string())
        );
        assert_matches_removal(&layout, &[2]);
    }

    #[test]
    fn focus_after_closing_second_to_last_tab_dissolves_onto_the_survivor() {
        let layout = tabs(vec![terminal("t1"), terminal("t2")]);
        assert_eq!(
            layout.terminal_to_focus_after_closing(&[0]),
            Some("t2".to_string())
        );
        assert_matches_removal(&layout, &[0]);
        assert_eq!(
            layout.terminal_to_focus_after_closing(&[1]),
            Some("t1".to_string())
        );
        assert_matches_removal(&layout, &[1]);
    }

    #[test]
    fn focus_after_closing_last_tab_of_a_group_leaves_for_the_neighbouring_pane() {
        // Split[ pane, Tabs[t2, t3] ]: closing t2 stays inside the group,
        // closing the survivor afterwards hands focus to the sibling pane.
        let layout = hsplit(vec![
            terminal("t1"),
            tabs(vec![terminal("t2"), terminal("t3")]),
        ]);
        assert_eq!(
            layout.terminal_to_focus_after_closing(&[1, 0]),
            Some("t3".to_string())
        );
        assert_matches_removal(&layout, &[1, 0]);

        let collapsed = hsplit(vec![terminal("t1"), terminal("t3")]);
        assert_eq!(
            collapsed.terminal_to_focus_after_closing(&[1]),
            Some("t1".to_string())
        );
    }

    #[test]
    fn focus_after_closing_split_pane_prefers_the_previous_neighbour() {
        let layout = hsplit(vec![terminal("t1"), terminal("t2"), terminal("t3")]);
        assert_eq!(
            layout.terminal_to_focus_after_closing(&[1]),
            Some("t1".to_string())
        );
        // Closing the first pane has no previous neighbour — take the next.
        assert_eq!(
            layout.terminal_to_focus_after_closing(&[0]),
            Some("t2".to_string())
        );
    }

    #[test]
    fn focus_after_closing_descends_into_the_neighbours_visible_tab() {
        let layout = hsplit(vec![
            tabs_active(vec![terminal("t1"), terminal("t2")], 1),
            terminal("t3"),
        ]);
        assert_eq!(
            layout.terminal_to_focus_after_closing(&[1]),
            Some("t2".to_string())
        );
    }

    #[test]
    fn focus_after_closing_the_only_terminal_is_none() {
        let layout = terminal("t1");
        assert_eq!(layout.terminal_to_focus_after_closing(&[]), None);
    }

    #[test]
    fn focus_after_closing_out_of_range_path_is_none() {
        let layout = hsplit(vec![terminal("t1"), terminal("t2")]);
        assert_eq!(layout.terminal_to_focus_after_closing(&[5]), None);
        assert_eq!(layout.terminal_to_focus_after_closing(&[0, 0]), None);
    }

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

    #[test]
    fn has_terminal_ids_matches_collect() {
        let nested = hsplit(vec![
            LayoutNode::new_terminal(),
            tabs(vec![LayoutNode::new_terminal(), terminal("t1")]),
        ]);
        assert!(nested.has_terminal_ids());

        let mut cleared = nested;
        cleared.clear_terminal_ids_except(&HashSet::new());
        assert!(
            !cleared.has_terminal_ids(),
            "a tree stripped for worktree teardown carries no ids",
        );
    }

    #[test]
    fn clear_terminal_ids_resets_all() {
        let mut node = hsplit(vec![terminal_minimized("t1"), terminal_detached("t2")]);
        node.clear_terminal_ids_except(&HashSet::new());
        assert!(node.collect_terminal_ids().is_empty());
        match &node {
            LayoutNode::Split { children, .. } => {
                for child in children {
                    if let LayoutNode::Terminal {
                        minimized,
                        detached,
                        ..
                    } = child
                    {
                        assert!(!minimized);
                        assert!(!detached);
                    }
                }
            }
            _ => panic!("Expected split"),
        }
    }

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
        let node = hsplit(vec![
            terminal("t1"),
            vsplit(vec![terminal_minimized("t2"), terminal_minimized("t3")]),
        ]);
        assert!(!node.is_all_hidden());
    }

    #[test]
    fn is_all_hidden_nested_all_hidden() {
        let node = hsplit(vec![
            terminal_minimized("t1"),
            vsplit(vec![terminal_minimized("t2"), terminal_detached("t3")]),
        ]);
        assert!(node.is_all_hidden());
    }

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

    #[test]
    fn collect_detached_terminals_finds_correct() {
        let node = hsplit(vec![terminal_detached("t1"), terminal("t2")]);
        let detached = node.collect_detached_terminals();
        assert_eq!(detached.len(), 1);
        assert_eq!(detached[0].0, "t1");
        assert_eq!(detached[0].1, vec![0]);
    }

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
        if let LayoutNode::Split {
            children,
            direction,
            sizes,
        } = &node
        {
            assert_eq!(*direction, SplitDirection::Horizontal);
            assert_eq!(children.len(), 3);
            assert_eq!(sizes.len(), 3);
            assert!((sizes[0] - 25.0).abs() < 0.01);
            assert!((sizes[1] - 25.0).abs() < 0.01);
            assert!((sizes[2] - 50.0).abs() < 0.01);
        } else {
            panic!("Expected flattened horizontal split");
        }
    }

    #[test]
    fn normalize_different_direction_preserved() {
        let mut node = LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            sizes: vec![50.0, 50.0],
            children: vec![vsplit(vec![terminal("t1"), terminal("t2")]), terminal("t3")],
        };
        node.normalize();
        if let LayoutNode::Split {
            children,
            direction,
            ..
        } = &node
        {
            assert_eq!(*direction, SplitDirection::Horizontal);
            assert_eq!(children.len(), 2);
            assert!(matches!(
                &children[0],
                LayoutNode::Split {
                    direction: SplitDirection::Vertical,
                    ..
                }
            ));
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
    fn normalize_out_of_range_active_tab_clamps() {
        let mut node = LayoutNode::Tabs {
            children: vec![terminal("t1"), terminal("t2")],
            active_tab: 5,
        };
        node.normalize();
        if let LayoutNode::Tabs {
            children,
            active_tab,
        } = &node
        {
            assert_eq!(*active_tab, children.len() - 1);
        } else {
            panic!("Expected tabs after normalize");
        }
    }

    #[test]
    fn normalize_deep_recursive() {
        let mut node = hsplit(vec![hsplit(vec![hsplit(vec![terminal("t1")])])]);
        node.normalize();
        match &node {
            LayoutNode::Terminal { terminal_id, .. } => {
                assert_eq!(terminal_id.as_deref(), Some("t1"));
            }
            _ => panic!("Expected terminal after deep normalize"),
        }
    }

    #[test]
    fn normalize_negative_sizes_reset_to_equal() {
        let mut node = LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            sizes: vec![5.0, 2.5, 2.5, -12.0],
            children: vec![
                terminal("t1"),
                terminal("t2"),
                terminal("t3"),
                terminal("t4"),
            ],
        };
        node.normalize();
        if let LayoutNode::Split { sizes, .. } = &node {
            assert_eq!(sizes.len(), 4);
            let expected = 100.0 / 4.0;
            for s in sizes {
                assert!((*s - expected).abs() < f32::EPSILON);
            }
        } else {
            panic!("Expected split");
        }
    }

    #[test]
    fn normalize_zero_size_reset_to_equal() {
        let mut node = LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            sizes: vec![5.0, 0.0],
            children: vec![terminal("t1"), terminal("t2")],
        };
        node.normalize();
        if let LayoutNode::Split { sizes, .. } = &node {
            assert_eq!(sizes.len(), 2);
            assert!((sizes[0] - 50.0).abs() < f32::EPSILON);
            assert!((sizes[1] - 50.0).abs() < f32::EPSILON);
        } else {
            panic!("Expected split");
        }
    }

    #[test]
    fn normalize_tiny_adjacent_sizes_reset_to_equal() {
        let mut node = LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            sizes: vec![90.0, 1.0, 9.0],
            children: vec![terminal("t1"), terminal("t2"), terminal("t3")],
        };
        node.normalize();
        if let LayoutNode::Split { sizes, .. } = &node {
            assert_eq!(sizes.len(), 3);
            let expected = 100.0 / 3.0;
            for s in sizes {
                assert!((*s - expected).abs() < f32::EPSILON);
            }
        } else {
            panic!("Expected split");
        }
    }

    #[test]
    fn normalize_valid_sizes_untouched() {
        let mut node = LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            sizes: vec![50.0, 50.0],
            children: vec![terminal("t1"), terminal("t2")],
        };
        node.normalize();
        if let LayoutNode::Split { sizes, .. } = &node {
            assert!((sizes[0] - 50.0).abs() < f32::EPSILON);
            assert!((sizes[1] - 50.0).abs() < f32::EPSILON);
        } else {
            panic!("Expected split");
        }
    }

    #[test]
    fn normalize_relative_sizes_untouched() {
        let mut node = LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            sizes: vec![26.8, 9.47, 17.6],
            children: vec![terminal("t1"), terminal("t2"), terminal("t3")],
        };
        node.normalize();
        if let LayoutNode::Split { sizes, .. } = &node {
            assert!((sizes[0] - 26.8).abs() < f32::EPSILON);
            assert!((sizes[1] - 9.47).abs() < f32::EPSILON);
            assert!((sizes[2] - 17.6).abs() < f32::EPSILON);
        } else {
            panic!("Expected split");
        }
    }

    #[test]
    fn clone_structure_clears_ids_preserves_shape() {
        let node = hsplit(vec![
            terminal("t1"),
            tabs(vec![terminal("t2"), terminal("t3")]),
        ]);
        let cloned = node.clone_structure();
        assert!(cloned.collect_terminal_ids().is_empty());
        match &cloned {
            LayoutNode::Split { children, .. } => {
                assert_eq!(children.len(), 2);
                assert!(matches!(&children[0], LayoutNode::Terminal { .. }));
                assert!(
                    matches!(&children[1], LayoutNode::Tabs { children, .. } if children.len() == 2)
                );
            }
            _ => panic!("Expected split"),
        }
    }

    #[test]
    fn remove_at_path_from_2_child_split_collapses() {
        let mut node = hsplit(vec![terminal("t1"), terminal("t2")]);
        let removed = node.remove_at_path(&[0]);
        assert!(removed.is_some());
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
            LayoutNode::Split {
                children, sizes, ..
            } => {
                assert_eq!(children.len(), 2);
                assert_eq!(sizes, &[66.0, 34.0]);
            }
            _ => panic!("Expected split with 2 children"),
        }
    }

    #[test]
    fn remove_at_path_transfers_first_child_weight_to_next_sibling() {
        let mut node = LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            sizes: vec![20.0, 30.0, 50.0],
            children: vec![terminal("t1"), terminal("t2"), terminal("t3")],
        };
        node.remove_at_path(&[0]);
        match &node {
            LayoutNode::Split { sizes, .. } => assert_eq!(sizes, &[50.0, 50.0]),
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
        let mut node = hsplit(vec![
            terminal("t1"),
            vsplit(vec![terminal("t2"), terminal("t3")]),
        ]);
        let removed = node.remove_at_path(&[1, 0]);
        assert!(removed.is_some());
        match &node {
            LayoutNode::Split { children, .. } => {
                assert_eq!(children.len(), 2);
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

    #[test]
    fn remove_terminal_ids_removes_leaves_and_collapses_layout() {
        let mut layout = Some(LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            sizes: vec![0.5, 0.5],
            children: vec![terminal("keep"), terminal("stale")],
        });
        let ids = HashSet::from(["stale"]);

        assert_eq!(LayoutNode::remove_terminal_ids(&mut layout, &ids), 1);
        assert_eq!(layout, Some(terminal("keep")));
    }

    #[test]
    fn remove_terminal_ids_clears_a_matching_root() {
        let mut layout = Some(terminal("stale"));
        let ids = HashSet::from(["stale"]);

        assert_eq!(LayoutNode::remove_terminal_ids(&mut layout, &ids), 1);
        assert!(layout.is_none());
    }

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

    #[test]
    fn merge_matching_terminals_preserves_visual_flags() {
        let server = LayoutNode::Terminal {
            terminal_id: Some("t1".to_string()),
            minimized: false,
            detached: false,
            shell_type: ShellType::Custom {
                path: "/bin/zsh".to_string(),
                args: Vec::new(),
            },
            zoom_level: 1.0,
        };
        let local = LayoutNode::Terminal {
            terminal_id: Some("t1".to_string()),
            minimized: true,
            detached: true,
            shell_type: ShellType::Default,
            zoom_level: 1.75,
        };
        let merged = LayoutNode::merge_visual_state(&server, &local);
        match merged {
            LayoutNode::Terminal {
                minimized,
                detached,
                terminal_id,
                shell_type,
                zoom_level,
            } => {
                assert_eq!(terminal_id.as_deref(), Some("t1"));
                assert!(minimized, "local minimized should be preserved");
                assert!(detached, "local detached should be preserved");
                assert_eq!(zoom_level, 1.75, "local zoom should be preserved");
                assert_eq!(
                    shell_type,
                    ShellType::Custom {
                        path: "/bin/zsh".to_string(),
                        args: Vec::new(),
                    },
                    "server shell should remain authoritative"
                );
            }
            _ => panic!("Expected terminal"),
        }
    }

    #[test]
    fn api_layout_preserves_daemon_shell_type() {
        let node = LayoutNode::Terminal {
            terminal_id: Some("t1".to_string()),
            minimized: false,
            detached: false,
            shell_type: ShellType::Custom {
                path: "/bin/fish".to_string(),
                args: vec!["--private".to_string()],
            },
            zoom_level: 2.0,
        };

        let restored = LayoutNode::from_api(&node.to_api());
        let LayoutNode::Terminal {
            shell_type,
            zoom_level,
            ..
        } = restored
        else {
            panic!("Expected terminal");
        };
        assert_eq!(
            shell_type,
            ShellType::Custom {
                path: "/bin/fish".to_string(),
                args: vec!["--private".to_string()],
            }
        );
        assert_eq!(
            zoom_level, 1.0,
            "client zoom is not daemon-owned wire state"
        );
    }

    #[test]
    fn merge_different_terminals_uses_server() {
        let server = terminal("t1");
        let local = terminal_minimized("t2");
        let merged = LayoutNode::merge_visual_state(&server, &local);
        match merged {
            LayoutNode::Terminal {
                terminal_id,
                minimized,
                ..
            } => {
                assert_eq!(terminal_id.as_deref(), Some("t1"));
                assert!(!minimized, "server state should win on ID mismatch");
            }
            _ => panic!("Expected terminal"),
        }
    }

    #[test]
    fn merge_matching_split_preserves_sizes() {
        let server = LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            sizes: vec![50.0, 50.0],
            children: vec![terminal("t1"), terminal("t2")],
        };
        let local = LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            sizes: vec![30.0, 70.0],
            children: vec![terminal("t1"), terminal("t2")],
        };
        let merged = LayoutNode::merge_visual_state(&server, &local);
        match merged {
            LayoutNode::Split { sizes, .. } => {
                assert!(
                    (sizes[0] - 30.0).abs() < f32::EPSILON,
                    "local sizes should be preserved"
                );
                assert!((sizes[1] - 70.0).abs() < f32::EPSILON);
            }
            _ => panic!("Expected split"),
        }
    }

    #[test]
    fn merge_split_child_count_mismatch_uses_server() {
        let server = LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            sizes: vec![33.0, 33.0, 34.0],
            children: vec![terminal("t1"), terminal("t2"), terminal("t3")],
        };
        let local = LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            sizes: vec![30.0, 70.0],
            children: vec![terminal("t1"), terminal("t2")],
        };
        let merged = LayoutNode::merge_visual_state(&server, &local);
        match merged {
            LayoutNode::Split {
                children, sizes, ..
            } => {
                assert_eq!(children.len(), 3, "server child count should win");
                assert!(
                    (sizes[0] - 33.0).abs() < f32::EPSILON,
                    "server sizes should be used"
                );
            }
            _ => panic!("Expected split"),
        }
    }

    #[test]
    fn merge_matching_tabs_preserves_active_tab() {
        let server = LayoutNode::Tabs {
            children: vec![terminal("t1"), terminal("t2")],
            active_tab: 0,
        };
        let local = LayoutNode::Tabs {
            children: vec![terminal("t1"), terminal("t2")],
            active_tab: 1,
        };
        let merged = LayoutNode::merge_visual_state(&server, &local);
        match merged {
            LayoutNode::Tabs { active_tab, .. } => {
                assert_eq!(active_tab, 1, "local active_tab should be preserved");
            }
            _ => panic!("Expected tabs"),
        }
    }

    #[test]
    fn merge_reordered_tabs_preserves_presentation_by_terminal_identity() {
        let server = LayoutNode::Tabs {
            children: vec![terminal("t2"), terminal("t1")],
            active_tab: 1,
        };
        let local = LayoutNode::Tabs {
            children: vec![
                LayoutNode::Terminal {
                    terminal_id: Some("t1".to_string()),
                    minimized: false,
                    detached: false,
                    shell_type: ShellType::Default,
                    zoom_level: 1.75,
                },
                terminal_minimized("t2"),
            ],
            active_tab: 0,
        };

        let merged = LayoutNode::merge_visual_state(&server, &local);
        let LayoutNode::Tabs {
            children,
            active_tab,
        } = merged
        else {
            panic!("expected tabs");
        };
        assert_eq!(active_tab, 1, "selected terminal should follow the reorder");
        assert!(matches!(
            &children[0],
            LayoutNode::Terminal {
                terminal_id: Some(id),
                minimized: true,
                ..
            } if id == "t2"
        ));
        assert!(matches!(
            &children[1],
            LayoutNode::Terminal {
                terminal_id: Some(id),
                zoom_level,
                ..
            } if id == "t1" && (*zoom_level - 1.75).abs() < f32::EPSILON
        ));
    }

    #[test]
    fn merge_reordered_split_keeps_sizes_with_their_panes() {
        let server = LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            sizes: vec![50.0, 50.0],
            children: vec![terminal("t2"), terminal("t1")],
        };
        let local = LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            sizes: vec![25.0, 75.0],
            children: vec![terminal("t1"), terminal("t2")],
        };

        let merged = LayoutNode::merge_visual_state(&server, &local);
        let LayoutNode::Split { sizes, .. } = merged else {
            panic!("expected split");
        };
        assert_eq!(sizes, vec![75.0, 25.0]);
    }

    #[test]
    fn merge_closed_tab_keeps_the_selected_terminal_active() {
        let server = LayoutNode::Tabs {
            children: vec![terminal("t1"), terminal("t3")],
            active_tab: 0,
        };
        let local = LayoutNode::Tabs {
            children: vec![terminal("t1"), terminal("t2"), terminal("t3")],
            active_tab: 2,
        };

        let merged = LayoutNode::merge_visual_state(&server, &local);
        let LayoutNode::Tabs { active_tab, .. } = merged else {
            panic!("expected tabs");
        };
        assert_eq!(active_tab, 1);
    }

    #[test]
    fn merge_type_mismatch_uses_server() {
        let server = hsplit(vec![terminal("t1"), terminal("t2")]);
        let local = terminal("t1");
        let merged = LayoutNode::merge_visual_state(&server, &local);
        match merged {
            LayoutNode::Split { children, .. } => {
                assert_eq!(
                    children.len(),
                    2,
                    "server structure should win on type mismatch"
                );
            }
            _ => panic!("Expected split"),
        }
    }

    #[test]
    fn merge_recursive_preserves_nested_state() {
        let server = hsplit(vec![
            terminal("t1"),
            LayoutNode::Tabs {
                children: vec![terminal("t2"), terminal("t3")],
                active_tab: 0,
            },
        ]);
        let local = LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            sizes: vec![25.0, 75.0],
            children: vec![
                LayoutNode::Terminal {
                    terminal_id: Some("t1".to_string()),
                    minimized: true,
                    detached: false,
                    shell_type: ShellType::Default,
                    zoom_level: 1.0,
                },
                LayoutNode::Tabs {
                    children: vec![terminal("t2"), terminal("t3")],
                    active_tab: 1,
                },
            ],
        };
        let merged = LayoutNode::merge_visual_state(&server, &local);
        match &merged {
            LayoutNode::Split {
                sizes, children, ..
            } => {
                assert!((sizes[0] - 25.0).abs() < f32::EPSILON);
                assert!((sizes[1] - 75.0).abs() < f32::EPSILON);
                match &children[0] {
                    LayoutNode::Terminal { minimized, .. } => assert!(*minimized),
                    _ => panic!("Expected terminal"),
                }
                match &children[1] {
                    LayoutNode::Tabs { active_tab, .. } => assert_eq!(*active_tab, 1),
                    _ => panic!("Expected tabs"),
                }
            }
            _ => panic!("Expected split"),
        }
    }

    #[test]
    fn merge_split_from_terminal_preserves_minimized() {
        let server = hsplit(vec![terminal("t1"), terminal("t2")]);
        let local = LayoutNode::Terminal {
            terminal_id: Some("t1".to_string()),
            minimized: true,
            detached: false,
            shell_type: ShellType::Default,
            zoom_level: 1.5,
        };
        let merged = LayoutNode::merge_visual_state(&server, &local);
        match &merged {
            LayoutNode::Split { children, .. } => {
                assert_eq!(children.len(), 2);
                match &children[0] {
                    LayoutNode::Terminal {
                        terminal_id,
                        minimized,
                        zoom_level,
                        ..
                    } => {
                        assert_eq!(terminal_id.as_deref(), Some("t1"));
                        assert!(
                            *minimized,
                            "minimized state should be preserved after split"
                        );
                        assert_eq!(*zoom_level, 1.5, "zoom should survive structure changes");
                    }
                    _ => panic!("Expected terminal"),
                }
                match &children[1] {
                    LayoutNode::Terminal {
                        terminal_id,
                        minimized,
                        ..
                    } => {
                        assert_eq!(terminal_id.as_deref(), Some("t2"));
                        assert!(!*minimized, "new terminal should not be minimized");
                    }
                    _ => panic!("Expected terminal"),
                }
            }
            _ => panic!("Expected split"),
        }
    }

    #[test]
    fn merge_structure_change_preserves_detached() {
        let server = hsplit(vec![terminal("t1"), terminal("t2")]);
        let local = terminal_detached("t1");
        let merged = LayoutNode::merge_visual_state(&server, &local);
        match &merged {
            LayoutNode::Split { children, .. } => match &children[0] {
                LayoutNode::Terminal { detached, .. } => {
                    assert!(*detached, "detached state should be preserved");
                }
                _ => panic!("Expected terminal"),
            },
            _ => panic!("Expected split"),
        }
    }

    #[test]
    fn merge_split_child_count_change_preserves_visual_state() {
        let server = hsplit(vec![terminal("t1"), terminal("t2"), terminal("t3")]);
        let local = LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            sizes: vec![30.0, 70.0],
            children: vec![terminal_minimized("t1"), terminal("t2")],
        };
        let merged = LayoutNode::merge_visual_state(&server, &local);
        match &merged {
            LayoutNode::Split { children, .. } => {
                assert_eq!(children.len(), 3);
                match &children[0] {
                    LayoutNode::Terminal { minimized, .. } => {
                        assert!(*minimized, "t1 minimized should be preserved");
                    }
                    _ => panic!("Expected terminal"),
                }
            }
            _ => panic!("Expected split"),
        }
    }
}
