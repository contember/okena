use crate::api::{ApiLayoutNode, StateResponse};
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

/// Collect all terminal IDs from all projects in a StateResponse (as a HashSet).
pub fn collect_all_terminal_ids(state: &StateResponse) -> HashSet<String> {
    let mut ids = HashSet::new();
    for project in &state.projects {
        if let Some(ref layout) = project.layout {
            collect_layout_terminal_ids(layout, &mut ids);
        }
    }
    ids
}

/// Collect all terminal IDs from a StateResponse (as a Vec).
pub fn collect_state_terminal_ids(state: &StateResponse) -> Vec<String> {
    let mut ids = Vec::new();
    for project in &state.projects {
        if let Some(ref layout) = project.layout {
            collect_layout_ids_vec(layout, &mut ids);
        }
    }
    ids
}

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

fn collect_layout_ids_vec(node: &ApiLayoutNode, ids: &mut Vec<String>) {
    match node {
        ApiLayoutNode::Terminal { terminal_id, .. } => {
            if let Some(id) = terminal_id {
                ids.push(id.clone());
            }
        }
        ApiLayoutNode::Split { children, .. } | ApiLayoutNode::Tabs { children, .. } => {
            for child in children {
                collect_layout_ids_vec(child, ids);
            }
        }
    }
}

/// Collect terminal sizes from all projects in a StateResponse.
///
/// Returns a map of terminal_id → (cols, rows) for terminals that have
/// size information in the layout tree.
pub fn collect_terminal_sizes(state: &StateResponse) -> std::collections::HashMap<String, (u16, u16)> {
    let mut sizes = std::collections::HashMap::new();
    for project in &state.projects {
        if let Some(ref layout) = project.layout {
            collect_layout_terminal_sizes(layout, &mut sizes);
        }
    }
    sizes
}

fn collect_layout_terminal_sizes(
    node: &ApiLayoutNode,
    sizes: &mut std::collections::HashMap<String, (u16, u16)>,
) {
    match node {
        ApiLayoutNode::Terminal {
            terminal_id,
            cols,
            rows,
            ..
        } => {
            if let (Some(id), Some(c), Some(r)) = (terminal_id, cols, rows) {
                sizes.insert(id.clone(), (*c, *r));
            }
        }
        ApiLayoutNode::Split { children, .. } | ApiLayoutNode::Tabs { children, .. } => {
            for child in children {
                collect_layout_terminal_sizes(child, sizes);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{ApiLayoutNode, ApiProject, StateResponse};
    use crate::theme::FolderColor;
    use crate::types::SplitDirection;

    fn make_state(projects: Vec<ApiProject>) -> StateResponse {
        StateResponse {
            state_version: 1,
            projects,
            focused_project_id: None,
            fullscreen_terminal: None,
            folders: Vec::new(),
            project_order: Vec::new(),
        }
    }

    fn make_project(id: &str, terminal_ids: Vec<&str>) -> ApiProject {
        let layout = if terminal_ids.is_empty() {
            None
        } else if terminal_ids.len() == 1 {
            Some(ApiLayoutNode::Terminal {
                terminal_id: Some(terminal_ids[0].to_string()),
                minimized: false,
                detached: false,
                cols: None,
                rows: None,
            })
        } else {
            Some(ApiLayoutNode::Split {
                direction: SplitDirection::Horizontal,
                sizes: vec![50.0; terminal_ids.len()],
                children: terminal_ids
                    .iter()
                    .map(|tid| ApiLayoutNode::Terminal {
                        terminal_id: Some(tid.to_string()),
                        minimized: false,
                        detached: false,
                        cols: None,
                        rows: None,
                    })
                    .collect(),
            })
        };
        ApiProject {
            id: id.to_string(),
            name: id.to_string(),
            path: "/tmp".to_string(),
            is_visible: true,
            layout,
            terminal_names: Default::default(),
            folder_color: FolderColor::default(),
        }
    }

    #[test]
    fn diff_states_detects_added_terminals() {
        let old = make_state(vec![make_project("p1", vec!["t1"])]);
        let new = make_state(vec![make_project("p1", vec!["t1", "t2"])]);
        let diff = diff_states(&old, &new);
        assert_eq!(diff.added_terminals, vec!["t2"]);
        assert!(diff.removed_terminals.is_empty());
    }

    #[test]
    fn diff_states_detects_removed_terminals() {
        let old = make_state(vec![make_project("p1", vec!["t1", "t2"])]);
        let new = make_state(vec![make_project("p1", vec!["t1"])]);
        let diff = diff_states(&old, &new);
        assert!(diff.added_terminals.is_empty());
        assert_eq!(diff.removed_terminals, vec!["t2"]);
    }

    #[test]
    fn diff_states_detects_changed_projects() {
        let old = make_state(vec![make_project("p1", vec!["t1"])]);
        let new = make_state(vec![make_project("p1", vec!["t1", "t2"])]);
        let diff = diff_states(&old, &new);
        assert_eq!(diff.changed_projects, vec!["p1"]);
    }

    #[test]
    fn diff_states_empty_to_empty() {
        let old = make_state(vec![]);
        let new = make_state(vec![]);
        let diff = diff_states(&old, &new);
        assert!(diff.added_terminals.is_empty());
        assert!(diff.removed_terminals.is_empty());
        assert!(diff.changed_projects.is_empty());
    }

    #[test]
    fn collect_terminal_sizes_extracts_from_layout() {
        let state = make_state(vec![ApiProject {
            id: "p1".into(),
            name: "p1".into(),
            path: "/tmp".into(),
            is_visible: true,
            layout: Some(ApiLayoutNode::Split {
                direction: SplitDirection::Horizontal,
                sizes: vec![50.0, 50.0],
                children: vec![
                    ApiLayoutNode::Terminal {
                        terminal_id: Some("t1".into()),
                        minimized: false,
                        detached: false,
                        cols: Some(120),
                        rows: Some(40),
                    },
                    ApiLayoutNode::Terminal {
                        terminal_id: Some("t2".into()),
                        minimized: false,
                        detached: false,
                        cols: None,
                        rows: None,
                    },
                ],
            }),
            terminal_names: Default::default(),
            folder_color: FolderColor::default(),
        }]);
        let sizes = collect_terminal_sizes(&state);
        assert_eq!(sizes.get("t1"), Some(&(120, 40)));
        assert_eq!(sizes.get("t2"), None);
    }
}
