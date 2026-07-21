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

/// Split weights are shares of 100, one per child. Every mutation goes through
/// [`LayoutNode::normalize`], which restores that invariant.
pub const TOTAL_PANE_WEIGHT: f32 = 100.0;

/// Smallest share a pane may hold, matching the 5% floor the resize drag clamps
/// to — so any layout the user can build by dragging survives normalization.
/// Splits with more than 20 children fall back to an equal share instead.
pub const MIN_PANE_WEIGHT: f32 = 5.0;

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
            LayoutNode::Terminal { minimized, detached, .. } => *minimized || *detached,
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

    /// Recursively flip every `Split` in this subtree between horizontal and
    /// vertical. `Tabs` and `Terminal` nodes are unaffected (tabs have no
    /// orientation), but the walk descends into both so nested splits inside
    /// tab groups are transposed too. `sizes` are preserved as-is — they are
    /// fractions along the split axis, so a 60/40 horizontal split becomes a
    /// 60/40 vertical split.
    pub fn transpose(&mut self) {
        match self {
            LayoutNode::Terminal { .. } => {}
            LayoutNode::Split { direction, children, .. } => {
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

    /// Clear terminal IDs except those in the `keep` set (e.g. hook terminals).
    /// Kept terminals preserve their ID, minimized, and detached state.
    pub fn clear_terminal_ids_except(&mut self, keep: &HashSet<&str>) {
        match self {
            LayoutNode::Terminal { terminal_id, minimized, detached, .. } => {
                let should_keep = terminal_id.as_deref()
                    .is_some_and(|id| keep.contains(id));
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
            LayoutNode::Split { children, .. } | LayoutNode::Tabs { children, .. } => {
                children.iter().find_map(|c| c.find_terminal_node(target_id))
            }
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
            LayoutNode::Split { children, sizes, .. } => {
                children.push(node);
                let n = children.len();
                *sizes = vec![1.0 / n as f32; n];
            }
            LayoutNode::Tabs { children, active_tab } => {
                children.push(node);
                *active_tab = children.len() - 1;
            }
            LayoutNode::Terminal { .. } => {
                let existing = std::mem::replace(
                    self,
                    LayoutNode::Tabs { children: Vec::new(), active_tab: 0 },
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

    fn collect_inactive_tabs_recursive(&self, result: &mut HashSet<String>, is_behind_inactive_tab: bool) {
        match self {
            LayoutNode::Terminal { terminal_id, .. } => {
                if is_behind_inactive_tab
                    && let Some(id) = terminal_id {
                        result.insert(id.clone());
                    }
            }
            LayoutNode::Split { children, .. } => {
                for child in children {
                    child.collect_inactive_tabs_recursive(result, is_behind_inactive_tab);
                }
            }
            LayoutNode::Tabs { children, active_tab } => {
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
                if inside_tab_group
                    && let Some(id) = terminal_id {
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
            LayoutNode::Tabs { children, active_tab } => {
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

    fn collect_minimized_recursive(&self, result: &mut Vec<(String, Vec<usize>)>, current_path: Vec<usize>) {
        match self {
            LayoutNode::Terminal { terminal_id, minimized, .. } => {
                if *minimized
                    && let Some(id) = terminal_id {
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

    fn collect_detached_recursive(&self, result: &mut Vec<(String, Vec<usize>)>, current_path: Vec<usize>) {
        match self {
            LayoutNode::Terminal { terminal_id, detached, .. } => {
                if *detached
                    && let Some(id) = terminal_id {
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

    fn find_terminal_path_recursive_impl(&self, current_path: Vec<usize>, follow_active_tab: bool) -> Vec<usize> {
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
            LayoutNode::Tabs { children, active_tab, .. } => {
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
    /// For split parents, transfers the removed weight to the next sibling, or
    /// the previous sibling when removing the last child.
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
                    let removed_size = sizes.remove(child_index);
                    if let Some(recipient) = Self::weight_recipient(children, child_index)
                        && let Some(slot) = sizes.get_mut(recipient) {
                        *slot += removed_size;
                    }
                }
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

    /// Append `node` as a new pane of this split, giving it an equal share of
    /// the new total and scaling the existing weights down proportionally so
    /// their ratios survive.
    ///
    /// Returns `false` when `self` is not a `Split`, leaving it untouched — the
    /// caller decides how to wrap a bare terminal or a tab group.
    pub fn push_split_child(&mut self, node: LayoutNode) -> bool {
        let LayoutNode::Split { children, sizes, .. } = self else {
            return false;
        };
        Self::sanitize_weights(sizes, children.len());

        let share = TOTAL_PANE_WEIGHT / (children.len() + 1) as f32;
        let keep = 1.0 - share / TOTAL_PANE_WEIGHT;
        for size in sizes.iter_mut() {
            *size *= keep;
        }
        children.push(node);
        sizes.push(share);
        true
    }

    /// Pick which sibling absorbs a removed pane's weight.
    ///
    /// Prefers the pane *before* the removed one. `split_terminal` always
    /// inserts the new pane directly after the one it split, so crediting the
    /// previous sibling is what makes split-then-close restore the original
    /// layout exactly; crediting the next one instead shrank the pane you split
    /// from on every cycle.
    ///
    /// Hidden panes are skipped in both directions: the renderer normalizes
    /// over visible children only, so weight parked on a minimized or detached
    /// pane leaves the visible budget entirely and comes back oversized when
    /// that pane is restored.
    ///
    /// `children` is already post-removal, so the previous sibling sits at
    /// `removed_index - 1` and the next one at `removed_index`.
    fn weight_recipient(children: &[LayoutNode], removed_index: usize) -> Option<usize> {
        if children.is_empty() {
            return None;
        }
        let split_at = removed_index.min(children.len());

        if let Some(previous) = children[..split_at]
            .iter()
            .rposition(|child| !child.is_all_hidden())
        {
            return Some(previous);
        }
        if let Some(offset) = children[split_at..]
            .iter()
            .position(|child| !child.is_all_hidden())
        {
            return Some(split_at + offset);
        }
        // Every sibling is hidden — keep the weight in the vec anyway so the
        // total is preserved for whenever one of them is restored.
        Some(split_at.min(children.len() - 1))
    }

    /// Bring a split's weights back to the model's invariant: exactly one
    /// finite, positive weight per child, summing to 100, none below
    /// [`MIN_PANE_WEIGHT`].
    ///
    /// Renormalizing to a fixed total is what makes the scale uniform — callers
    /// that write weights on a 0..1 scale converge here instead of rendering a
    /// pane at ~1% next to a 0..100 sibling.
    ///
    /// This replaces an older repair that reset *every* weight in the split to
    /// equal as soon as it saw an adjacent pair summing under 10% of the total.
    /// That discarded deliberate layouts wholesale, and because the resize drag
    /// clamps each pane to 5%, two panes dragged to the minimum landed exactly
    /// on the reject threshold — the smallest arrangement the UI let you build
    /// was wiped by the next mutation. Rescaling keeps the user's proportions.
    fn sanitize_weights(sizes: &mut Vec<f32>, child_count: usize) {
        if child_count == 0 {
            sizes.clear();
            return;
        }
        let equal = TOTAL_PANE_WEIGHT / child_count as f32;

        sizes.truncate(child_count);
        while sizes.len() < child_count {
            sizes.push(equal);
        }

        // A non-finite or non-positive weight means the vector can't be trusted
        // — fall back to an equal split rather than trying to honour garbage.
        if sizes.iter().any(|size| !size.is_finite() || *size <= 0.0) {
            log::warn!("Layout has invalid split sizes {:?}, resetting to equal", sizes);
            sizes.fill(equal);
            return;
        }

        let total: f32 = sizes.iter().sum();
        let scale = TOTAL_PANE_WEIGHT / total;
        for size in sizes.iter_mut() {
            *size *= scale;
        }

        // Raise anything under the floor, taking the difference from the panes
        // that have room in proportion to their surplus. The total stays 100.
        let floor = MIN_PANE_WEIGHT.min(equal);
        let deficit: f32 = sizes.iter().map(|size| (floor - *size).max(0.0)).sum();
        if deficit <= 0.0 {
            return;
        }
        let surplus: f32 = sizes.iter().map(|size| (*size - floor).max(0.0)).sum();
        if surplus <= deficit {
            sizes.fill(equal);
            return;
        }
        let keep = (surplus - deficit) / surplus;
        for size in sizes.iter_mut() {
            *size = floor + (*size - floor).max(0.0) * keep;
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

        if let LayoutNode::Tabs { children, active_tab } = self {
            *active_tab = (*active_tab).min(children.len().saturating_sub(1));
        }

        // Reconcile lengths and scale BEFORE flattening: the flatten below
        // indexes `sizes[i]` per child and divides the parent slot across the
        // grandchildren, so it needs one sane weight per child to work from.
        if let LayoutNode::Split { sizes, children, .. } = self {
            Self::sanitize_weights(sizes, children.len());
        }

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
                        *self = LayoutNode::new_terminal();
                    }
                }
                _ => {}
            }
            return;
        }

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

        // Re-apply the invariant after flattening: splitting a parent slot
        // across grandchildren can push a pane under the floor even when both
        // the parent and child weights were fine on their own.
        if let LayoutNode::Split { sizes, children, .. } = self {
            Self::sanitize_weights(sizes, children.len());
        }
    }

    /// Clone the layout structure but clear all terminal IDs.
    /// Used when creating worktree projects to duplicate layout with fresh terminals.
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
                LayoutNode::Split { direction: l_dir, sizes: l_sizes, children: l_children, .. },
            ) if s_dir == l_dir => {
                let mapping = LayoutNode::matching_child_indices(s_children, l_children);
                let merged_children = LayoutNode::merge_mapped_children(
                    s_children,
                    l_children,
                    mapping.as_deref(),
                );
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
                LayoutNode::Tabs { children: l_children, active_tab: l_active, .. },
            ) => {
                let mapping = LayoutNode::matching_child_indices(s_children, l_children);
                let merged_children = LayoutNode::merge_mapped_children(
                    s_children,
                    l_children,
                    mapping.as_deref(),
                );
                LayoutNode::Tabs {
                    children: merged_children,
                    active_tab: LayoutNode::merged_active_tab(
                        s_children,
                        *s_active,
                        l_children,
                        *l_active,
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
    fn matching_child_indices(
        server: &[LayoutNode],
        local: &[LayoutNode],
    ) -> Option<Vec<usize>> {
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
        let selected_ids: HashSet<String> = local_child
            .collect_terminal_ids()
            .into_iter()
            .collect();
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
                children: children.iter().map(|c| c.to_api_with_sizes(sizes)).collect(),
            },
            LayoutNode::Tabs {
                children,
                active_tab,
            } => okena_core::api::ApiLayoutNode::Tabs {
                children: children.iter().map(|c| c.to_api_with_sizes(sizes)).collect(),
                active_tab: *active_tab,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{LayoutNode, SplitDirection, MIN_PANE_WEIGHT, TOTAL_PANE_WEIGHT};
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

        let LayoutNode::Split { direction, children, .. } = &tree else {
            panic!("expected split");
        };
        assert_eq!(*direction, SplitDirection::Vertical);
        let LayoutNode::Tabs { children: tab_children, active_tab } = &children[1] else {
            panic!("expected tabs");
        };
        assert_eq!(*active_tab, 0, "tab structure preserved");
        let LayoutNode::Split { direction: inner, .. } = &tab_children[0] else {
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
        let LayoutNode::Split { children, sizes, .. } = &tree else {
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
        let LayoutNode::Tabs { children, active_tab } = &tree else {
            panic!("expected tabs");
        };
        assert_eq!(children.len(), 3);
        assert_eq!(*active_tab, 2, "new tab activated");
    }

    #[test]
    fn append_to_root_bare_terminal_wraps_in_split() {
        let mut tree = terminal("a");
        tree.append_to_root(terminal("b"));
        let LayoutNode::Split { children, sizes, .. } = &tree else {
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
    fn clear_terminal_ids_resets_all() {
        let mut node = hsplit(vec![
            terminal_minimized("t1"),
            terminal_detached("t2"),
        ]);
        node.clear_terminal_ids_except(&HashSet::new());
        assert!(node.collect_terminal_ids().is_empty());
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
        let node = hsplit(vec![
            terminal_detached("t1"),
            terminal("t2"),
        ]);
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
        if let LayoutNode::Split { children, direction, sizes } = &node {
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
    fn normalize_out_of_range_active_tab_clamps() {
        let mut node = LayoutNode::Tabs {
            children: vec![terminal("t1"), terminal("t2")],
            active_tab: 5,
        };
        node.normalize();
        if let LayoutNode::Tabs { children, active_tab } = &node {
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
            children: vec![terminal("t1"), terminal("t2"), terminal("t3"), terminal("t4")],
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
    fn normalize_raises_a_tiny_pane_to_the_floor_and_keeps_the_rest() {
        let mut node = LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            sizes: vec![90.0, 1.0, 9.0],
            children: vec![terminal("t1"), terminal("t2"), terminal("t3")],
        };
        node.normalize();
        if let LayoutNode::Split { sizes, .. } = &node {
            assert_eq!(sizes.len(), 3);
            // The pane under the floor is raised to it and the two with room
            // give up the difference in proportion to their surplus — so the
            // layout still reads as "one big pane, one small, one minimum".
            // The old repair reset all three to 33.3 and threw it away.
            assert!((sizes[1] - MIN_PANE_WEIGHT).abs() < 1e-3, "got {:?}", sizes);
            assert!(sizes[0] > sizes[2] && sizes[2] > sizes[1], "got {:?}", sizes);
            assert!(sizes[0] > 80.0, "the dominant pane stays dominant: {:?}", sizes);
            assert!((sizes.iter().sum::<f32>() - TOTAL_PANE_WEIGHT).abs() < 1e-3);
        } else {
            panic!("Expected split");
        }
    }

    #[test]
    fn normalize_rechecks_tiny_sizes_after_flattening_same_direction_split() {
        let mut node = LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            sizes: vec![33.333_332, 1.041_666_6],
            children: vec![
                terminal("t1"),
                LayoutNode::Split {
                    direction: SplitDirection::Horizontal,
                    sizes: vec![50.0, 50.0],
                    children: vec![terminal("t2"), terminal("t3")],
                },
            ],
        };

        node.normalize();

        let LayoutNode::Split { sizes, children, .. } = node else {
            panic!("Expected flattened split");
        };
        assert_eq!(children.len(), 3);
        // Flattening divides the nested split's slot across its grandchildren,
        // which can drop them under the floor. They get raised to it and the
        // big pane absorbs the cost — nobody's layout is discarded.
        assert!((sizes[1] - MIN_PANE_WEIGHT).abs() < 1e-3, "got {:?}", sizes);
        assert!((sizes[2] - MIN_PANE_WEIGHT).abs() < 1e-3, "got {:?}", sizes);
        assert!((sizes.iter().sum::<f32>() - TOTAL_PANE_WEIGHT).abs() < 1e-3);
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
    fn normalize_rescales_relative_sizes_to_the_standard_total() {
        let original = [26.8_f32, 9.47, 17.6];
        let mut node = LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            sizes: original.to_vec(),
            children: vec![terminal("t1"), terminal("t2"), terminal("t3")],
        };
        node.normalize();
        if let LayoutNode::Split { sizes, .. } = &node {
            // Weights are shares of 100 whatever scale the writer used, so a
            // vec written on one scale can never render against another.
            assert!((sizes.iter().sum::<f32>() - TOTAL_PANE_WEIGHT).abs() < 1e-3);
            let total: f32 = original.iter().sum();
            for (size, source) in sizes.iter().zip(original) {
                let expected = source / total * TOTAL_PANE_WEIGHT;
                assert!((size - expected).abs() < 1e-3, "got {:?}", sizes);
            }
        } else {
            panic!("Expected split");
        }
    }

    #[test]
    fn normalize_converges_weights_written_on_a_fractional_scale() {
        let mut node = LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            sizes: vec![0.25, 0.75],
            children: vec![terminal("t1"), terminal("t2")],
        };
        node.normalize();
        if let LayoutNode::Split { sizes, .. } = &node {
            assert_eq!(sizes.as_slice(), &[25.0, 75.0]);
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
                assert!(matches!(&children[1], LayoutNode::Tabs { children, .. } if children.len() == 2));
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
            LayoutNode::Split { children, sizes, .. } => {
                assert_eq!(children.len(), 2);
                assert_eq!(
                    sizes.as_slice(),
                    &[66.0, 34.0],
                    "the freed weight goes to the pane before it — the one a split would have grown from",
                );
            }
            _ => panic!("Expected split with 2 children"),
        }
    }

    #[test]
    fn remove_at_path_credits_the_next_pane_when_removing_the_first() {
        let mut node = LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            sizes: vec![33.0, 33.0, 34.0],
            children: vec![terminal("t1"), terminal("t2"), terminal("t3")],
        };
        node.remove_at_path(&[0]);
        match &node {
            LayoutNode::Split { sizes, .. } => {
                assert_eq!(sizes.as_slice(), &[66.0, 34.0], "no previous sibling to credit");
            }
            _ => panic!("Expected split with 2 children"),
        }
    }

    #[test]
    fn remove_at_path_skips_hidden_siblings_when_crediting_weight() {
        let mut node = LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            sizes: vec![25.0, 25.0, 50.0],
            children: vec![terminal("t1"), terminal_minimized("t2"), terminal("t3")],
        };
        // Removing t3 would positionally credit the minimized t2, parking the
        // weight where the renderer cannot see it. Skip to the visible t1.
        node.remove_at_path(&[2]);
        match &node {
            LayoutNode::Split { sizes, .. } => {
                assert_eq!(sizes.as_slice(), &[75.0, 25.0]);
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
        assert_eq!(zoom_level, 1.0, "client zoom is not daemon-owned wire state");
    }

    #[test]
    fn merge_different_terminals_uses_server() {
        let server = terminal("t1");
        let local = terminal_minimized("t2");
        let merged = LayoutNode::merge_visual_state(&server, &local);
        match merged {
            LayoutNode::Terminal { terminal_id, minimized, .. } => {
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
                assert!((sizes[0] - 30.0).abs() < f32::EPSILON, "local sizes should be preserved");
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
            LayoutNode::Split { children, sizes, .. } => {
                assert_eq!(children.len(), 3, "server child count should win");
                assert!((sizes[0] - 33.0).abs() < f32::EPSILON, "server sizes should be used");
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
                assert_eq!(children.len(), 2, "server structure should win on type mismatch");
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
            LayoutNode::Split { sizes, children, .. } => {
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
                        assert!(*minimized, "minimized state should be preserved after split");
                        assert_eq!(*zoom_level, 1.5, "zoom should survive structure changes");
                    }
                    _ => panic!("Expected terminal"),
                }
                match &children[1] {
                    LayoutNode::Terminal { terminal_id, minimized, .. } => {
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
            LayoutNode::Split { children, .. } => {
                match &children[0] {
                    LayoutNode::Terminal { detached, .. } => {
                        assert!(*detached, "detached state should be preserved");
                    }
                    _ => panic!("Expected terminal"),
                }
            }
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
