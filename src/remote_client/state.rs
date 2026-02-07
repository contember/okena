use crate::remote::types::{ApiLayoutNode, StateResponse};
use std::collections::HashSet;

/// Represents the differences between two remote state snapshots.
pub struct StateDiff {
    /// Terminal IDs that appeared in the new state but not the old
    pub added_terminals: Vec<String>,
    /// Terminal IDs that were in the old state but not the new
    pub removed_terminals: Vec<String>,
    /// Project IDs whose layouts changed between old and new states
    pub changed_projects: Vec<String>,
}

/// Compute the diff between two StateResponse snapshots.
///
/// Used when a `state_changed` WebSocket event arrives: the client re-fetches
/// `/v1/state` and diffs against the cached snapshot to determine which
/// terminals need to be subscribed/unsubscribed and which projects changed.
pub fn diff_states(old: &StateResponse, new: &StateResponse) -> StateDiff {
    let old_terminals = collect_all_terminal_ids(old);
    let new_terminals = collect_all_terminal_ids(new);

    let added_terminals: Vec<String> = new_terminals
        .difference(&old_terminals)
        .cloned()
        .collect();

    let removed_terminals: Vec<String> = old_terminals
        .difference(&new_terminals)
        .cloned()
        .collect();

    // Detect projects with layout changes by comparing serialized layouts
    let mut changed_projects = Vec::new();
    let old_projects: std::collections::HashMap<&str, _> = old
        .projects
        .iter()
        .map(|p| (p.id.as_str(), p))
        .collect();

    for new_proj in &new.projects {
        let changed = match old_projects.get(new_proj.id.as_str()) {
            Some(old_proj) => {
                // Compare layouts by serialization (simple but correct)
                let old_layout = serde_json::to_string(&old_proj.layout).unwrap_or_default();
                let new_layout = serde_json::to_string(&new_proj.layout).unwrap_or_default();
                old_layout != new_layout
            }
            None => true, // entirely new project
        };
        if changed {
            changed_projects.push(new_proj.id.clone());
        }
    }

    StateDiff {
        added_terminals,
        removed_terminals,
        changed_projects,
    }
}

/// Collect all terminal IDs from all projects in a StateResponse.
fn collect_all_terminal_ids(state: &StateResponse) -> HashSet<String> {
    let mut ids = HashSet::new();
    for project in &state.projects {
        if let Some(ref layout) = project.layout {
            collect_layout_terminal_ids(layout, &mut ids);
        }
    }
    ids
}

/// Recursively collect terminal IDs from an API layout node tree.
fn collect_layout_terminal_ids(node: &ApiLayoutNode, ids: &mut HashSet<String>) {
    match node {
        ApiLayoutNode::Terminal { terminal_id, .. } => {
            if let Some(id) = terminal_id {
                ids.insert(id.clone());
            }
        }
        ApiLayoutNode::Split { children, .. } | ApiLayoutNode::Tabs { children, .. } => {
            for child in children {
                collect_layout_terminal_ids(child, ids);
            }
        }
    }
}
