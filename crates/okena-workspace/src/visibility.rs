//! Pure logic for computing the visible set of projects in the sidebar.
//!
//! Folder filter, focus override, and worktree grouping all interact non-trivially.
//! Keeping this in its own module lets us unit-test the tricky cases without
//! constructing a GPUI entity.

use std::collections::{HashMap, HashSet};

use crate::state::{ProjectData, WindowState, WorkspaceData};

/// Compute the ordered list of visible projects given current workspace state.
///
/// Rules:
/// - When a project is focused, only that project (and optionally its worktree
///   children) is shown.
/// - When `focus_individual` is true, a focused parent project does NOT expand
///   its worktree children.
/// - When the window has a folder filter, top-level projects are hidden and
///   only projects inside the filtered folder are shown. Focus override still
///   wins.
/// - Worktree children are grouped directly after their parent project.
pub fn compute_visible_projects<'a>(
    data: &'a WorkspaceData,
    focused: Option<&String>,
    focus_individual: bool,
    window: &WindowState,
) -> Vec<&'a ProjectData> {
    let folder_filter = window.folder_filter.as_ref();
    // Pre-compute worktree children whose parent lives in a folder.
    // These must only be added during folder expansion (not from project_order),
    // because their position in project_order may not reflect the folder ordering.
    let folder_owned_worktrees: HashSet<&str> = {
        let folder_project_ids: HashSet<&str> = data
            .folders
            .iter()
            .flat_map(|f| f.project_ids.iter().map(|id| id.as_str()))
            .collect();
        data.projects
            .iter()
            .filter(|p| {
                p.worktree_info
                    .as_ref()
                    .map_or(false, |wi| {
                        folder_project_ids.contains(wi.parent_project_id.as_str())
                    })
            })
            .map(|p| p.id.as_str())
            .collect()
    };

    let mut result = Vec::new();
    // Track worktree children already added via their parent's folder
    let mut added_via_folder: HashSet<&str> = HashSet::new();
    for id in &data.project_order {
        if let Some(folder) = data.folders.iter().find(|f| f.id == *id) {
            // When folder filter is active, skip folders that don't match
            if let Some(filter_id) = folder_filter {
                if &folder.id != filter_id {
                    // Still allow the focused project (or its worktree) through
                    if focused.is_some() {
                        for pid in &folder.project_ids {
                            if let Some(p) = data.projects.iter().find(|p| &p.id == pid) {
                                push_project_with_worktrees(
                                    data,
                                    p,
                                    focused,
                                    focus_individual,
                                    window,
                                    &mut result,
                                );
                            }
                        }
                    }
                    continue;
                }
            }
            // Folder: include its projects and their worktree children.
            // Worktree children live in project_order (not folder.project_ids),
            // so we expand them here to keep them positioned within their folder's section.
            for pid in &folder.project_ids {
                // Skip if already added as a worktree child of a previous folder project
                if added_via_folder.contains(pid.as_str()) {
                    continue;
                }
                if let Some(p) = data.projects.iter().find(|p| p.id == *pid) {
                    push_project_with_worktrees(
                        data,
                        p,
                        focused,
                        focus_individual,
                        window,
                        &mut result,
                    );
                    if folder_filter.is_some() {
                        for wt_id in &p.worktree_ids {
                            added_via_folder.insert(wt_id.as_str());
                        }
                    }
                }
            }
        } else if let Some(p) = data.projects.iter().find(|p| p.id == *id) {
            // Skip worktree children that belong to folder projects —
            // they'll be added during their parent's folder expansion instead
            if folder_owned_worktrees.contains(p.id.as_str())
                || added_via_folder.contains(p.id.as_str())
            {
                continue;
            }
            // Top-level project: hide when folder filter is active
            if folder_filter.is_some() {
                // Still allow the focused project through
                if focused.is_some() {
                    push_project_with_worktrees(
                        data,
                        p,
                        focused,
                        focus_individual,
                        window,
                        &mut result,
                    );
                }
                continue;
            }
            push_project_with_worktrees(data, p, focused, focus_individual, window, &mut result);
        }
    }

    // Group worktree children right after their parent project.
    // Only moves worktrees whose parent IS in the result; orphan worktrees
    // (parent not visible or in a folder) stay at their original position.
    let worktree_children: HashMap<&str, Vec<&ProjectData>> = {
        let result_ids: HashSet<&str> = result.iter().map(|p| p.id.as_str()).collect();
        let mut map: HashMap<&str, Vec<&ProjectData>> = HashMap::new();
        for p in &result {
            if let Some(ref wi) = p.worktree_info {
                if result_ids.contains(wi.parent_project_id.as_str()) {
                    map.entry(wi.parent_project_id.as_str()).or_default().push(p);
                }
            }
        }
        map
    };
    if !worktree_children.is_empty() {
        let grouped_child_ids: HashSet<&str> = worktree_children
            .values()
            .flat_map(|children| children.iter().map(|p| p.id.as_str()))
            .collect();
        let mut grouped = Vec::with_capacity(result.len());
        for p in &result {
            if grouped_child_ids.contains(p.id.as_str()) {
                continue;
            }
            grouped.push(*p);
            if let Some(children) = worktree_children.get(p.id.as_str()) {
                grouped.extend(children);
            }
        }
        return grouped;
    }

    result
}

/// Push a project and its worktree children into the result list, respecting
/// focus filtering: when a project is focused, only show that project (not
/// sibling worktrees). When a worktree is focused, only show that worktree.
/// When `individual` is true, even a focused parent shows only itself.
fn push_project_with_worktrees<'a>(
    data: &'a WorkspaceData,
    p: &'a ProjectData,
    focused: Option<&String>,
    individual: bool,
    window: &WindowState,
    result: &mut Vec<&'a ProjectData>,
) {
    match focused {
        None => {
            if !window.hidden_project_ids.contains(&p.id) {
                result.push(p);
            }
            for wt_id in &p.worktree_ids {
                if let Some(wt) = data.projects.iter().find(|pp| &pp.id == wt_id)
                    && !window.hidden_project_ids.contains(&wt.id)
                {
                    result.push(wt);
                }
            }
        }
        Some(fid) => {
            if &p.id == fid {
                result.push(p);
                if !individual {
                    for wt_id in &p.worktree_ids {
                        if let Some(wt) = data.projects.iter().find(|pp| &pp.id == wt_id) {
                            result.push(wt);
                        }
                    }
                }
            } else if p.worktree_ids.contains(fid)
                && let Some(wt) = data.projects.iter().find(|pp| &pp.id == fid)
            {
                result.push(wt);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::HooksConfig;
    use crate::state::{FolderData, LayoutNode, WorktreeMetadata};
    use okena_core::theme::FolderColor;
    use okena_terminal::shell_config::ShellType;
    use std::collections::HashMap;

    fn make_project(id: &str) -> ProjectData {
        ProjectData {
            id: id.to_string(),
            name: format!("Project {}", id),
            path: "/tmp/test".to_string(),
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
            worktree_ids: Vec::new(),
            folder_color: FolderColor::default(),
            hooks: HooksConfig::default(),
            is_remote: false,
            connection_id: None,
            service_terminals: HashMap::new(),
            default_shell: None,
            hook_terminals: HashMap::new(),
        }
    }

    fn make_wt(id: &str, parent: &str) -> ProjectData {
        let mut p = make_project(id);
        p.worktree_info = Some(WorktreeMetadata {
            parent_project_id: parent.to_string(),
            color_override: None,
            main_repo_path: "/tmp/repo".to_string(),
            worktree_path: format!("/tmp/{}", id),
            branch_name: format!("branch-{}", id),
        });
        p
    }

    fn make_data(projects: Vec<ProjectData>, order: Vec<&str>, hidden: &[&str]) -> WorkspaceData {
        // Per-window viewport model: hidden state is set explicitly via
        // `main_window.hidden_project_ids`. Tests that don't exercise hidden
        // behavior pass an empty `hidden` slice.
        let main_window = WindowState {
            hidden_project_ids: hidden.iter().map(|s| s.to_string()).collect(),
            ..WindowState::default()
        };
        WorkspaceData {
            version: 1,
            projects,
            project_order: order.into_iter().map(String::from).collect(),
            service_panel_heights: HashMap::new(),
            hook_panel_heights: HashMap::new(),
            folders: Vec::new(),
            main_window,
            extra_windows: Vec::new(),
        }
    }

    #[test]
    fn filters_hidden_projects() {
        let data = make_data(
            vec![
                make_project("p1"),
                make_project("p2"),
                make_project("p3"),
            ],
            vec!["p1", "p2", "p3"],
            &["p2"],
        );
        let visible = compute_visible_projects(&data, None, false, &data.main_window);
        assert_eq!(visible.len(), 2);
        assert_eq!(visible[0].id, "p1");
        assert_eq!(visible[1].id, "p3");
    }

    #[test]
    fn focused_project_shown_even_when_hidden() {
        let data = make_data(
            vec![
                make_project("p1"),
                make_project("p2"),
                make_project("p3"),
            ],
            vec!["p1", "p2", "p3"],
            &["p3"],
        );
        let focused = "p3".to_string();
        let visible =
            compute_visible_projects(&data, Some(&focused), false, &data.main_window);
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].id, "p3");
    }

    #[test]
    fn folder_expands_children() {
        let mut data = make_data(
            vec![make_project("p1"), make_project("p2")],
            vec!["f1"],
            &[],
        );
        data.folders.push(FolderData {
            id: "f1".to_string(),
            name: "Folder".to_string(),
            project_ids: vec!["p1".to_string(), "p2".to_string()],
            folder_color: FolderColor::default(),
        });
        let visible = compute_visible_projects(&data, None, false, &WindowState::default());
        assert_eq!(visible.len(), 2);
    }

    #[test]
    fn folder_filter_hides_top_level() {
        let mut data = make_data(
            vec![
                make_project("p1"),
                make_project("p2"),
                make_project("p3"),
            ],
            vec!["f1", "p3"],
            &[],
        );
        data.folders.push(FolderData {
            id: "f1".to_string(),
            name: "Folder".to_string(),
            project_ids: vec!["p1".to_string(), "p2".to_string()],
            folder_color: FolderColor::default(),
        });
        let window = WindowState {
            folder_filter: Some("f1".to_string()),
            ..WindowState::default()
        };
        let visible = compute_visible_projects(&data, None, false, &window);
        assert_eq!(visible.len(), 2);
        assert!(visible.iter().all(|p| p.id != "p3"));
    }

    #[test]
    fn worktree_children_grouped_after_parent() {
        let mut parent = make_project("parent");
        parent.worktree_ids = vec!["wt1".to_string()];
        let wt1 = make_wt("wt1", "parent");
        let data = make_data(vec![parent, wt1], vec!["parent"], &[]);
        let visible = compute_visible_projects(&data, None, false, &WindowState::default());
        assert_eq!(visible.len(), 2);
        assert_eq!(visible[0].id, "parent");
        assert_eq!(visible[1].id, "wt1");
    }

    #[test]
    fn focus_worktree_shows_only_worktree() {
        let mut parent = make_project("parent");
        parent.worktree_ids = vec!["wt1".to_string()];
        let wt1 = make_wt("wt1", "parent");
        let data = make_data(vec![parent, wt1], vec!["parent"], &[]);
        let focused = "wt1".to_string();
        let visible =
            compute_visible_projects(&data, Some(&focused), false, &WindowState::default());
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].id, "wt1");
    }

    #[test]
    fn focus_parent_individual_hides_worktrees() {
        let mut parent = make_project("parent");
        parent.worktree_ids = vec!["wt1".to_string()];
        let wt1 = make_wt("wt1", "parent");
        let data = make_data(vec![parent, wt1], vec!["parent"], &[]);
        let focused = "parent".to_string();
        let visible =
            compute_visible_projects(&data, Some(&focused), true, &WindowState::default());
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].id, "parent");
    }

    #[test]
    fn orphan_worktree_shown_when_parent_hidden() {
        // Hidden parent without worktree_ids — the child still has worktree_info
        // pointing at it and lives in project_order as an independent entry.
        let parent = make_project("p1");
        let w1 = make_wt("w1", "p1");
        let data = make_data(vec![parent, w1], vec!["p1", "w1"], &["p1"]);
        let visible = compute_visible_projects(&data, None, false, &data.main_window);
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].id, "w1");
    }

    #[test]
    fn hidden_project_ids_hides_projects() {
        // hidden_project_ids on the window state hides projects -- the per-
        // window hidden set is the sole visibility mechanism after the
        // legacy ProjectData.show_in_overview field was removed.
        let data = make_data(
            vec![
                make_project("p1"),
                make_project("p2"),
                make_project("p3"),
            ],
            vec!["p1", "p2", "p3"],
            &[],
        );
        let mut window = WindowState::default();
        window.hidden_project_ids.insert("p2".to_string());
        let visible = compute_visible_projects(&data, None, false, &window);
        let ids: Vec<&str> = visible.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, vec!["p1", "p3"]);
    }

    #[test]
    fn hidden_worktree_id_hides_worktree_under_visible_parent() {
        // Worktree visibility also routes through hidden_project_ids: when the
        // parent is visible but a worktree's id is in the hidden set, the
        // worktree is dropped from the result.
        let mut parent = make_project("parent");
        parent.worktree_ids = vec!["wt1".to_string()];
        let wt1 = make_wt("wt1", "parent");
        let data = make_data(vec![parent, wt1], vec!["parent"], &[]);
        let mut window = WindowState::default();
        window.hidden_project_ids.insert("wt1".to_string());
        let visible = compute_visible_projects(&data, None, false, &window);
        let ids: Vec<&str> = visible.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, vec!["parent"]);
    }

    #[test]
    fn folder_filter_sourced_from_window_state() {
        // Same fixture as folder_filter_hides_top_level but routed via
        // WindowState.folder_filter instead of the prior loose argument —
        // verifies the new signature reads filter from the window.
        let mut data = make_data(
            vec![
                make_project("p1"),
                make_project("p2"),
                make_project("p3"),
            ],
            vec!["f1", "p3"],
            &[],
        );
        data.folders.push(FolderData {
            id: "f1".to_string(),
            name: "Folder".to_string(),
            project_ids: vec!["p1".to_string(), "p2".to_string()],
            folder_color: FolderColor::default(),
        });
        let mut window = WindowState::default();
        window.folder_filter = Some("f1".to_string());
        let visible = compute_visible_projects(&data, None, false, &window);
        let ids: Vec<&str> = visible.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, vec!["p1", "p2"]);
    }
}
