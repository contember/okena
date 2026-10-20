//! Status board: the pure grouping both the sidebar and the board overlay
//! draw from. GPUI-free.
//!
//! A status is a user-set column id on a project (`ProjectData::status`),
//! typically on the worktrees of one parent project. Grouping resolves ids
//! against the ordered columns from settings: columns first, in their order,
//! then an unassigned bucket for projects with no status or a status whose
//! column no longer exists. Input order is kept inside every group.

use okena_core::types::BoardColumn;
use okena_state::ProjectData;

/// One row of a grouped list: the item's original index and, on the first
/// row of each group, the caption to draw above it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GroupedRow {
    pub index: usize,
    /// `Some(column)` opens a column group, `Some(None)`… is not a thing:
    /// `caption: Some(None)` opens the "No status" group.
    pub caption: Option<Option<BoardColumn>>,
}

/// Whether any of `statuses` resolves to one of `columns`. When false,
/// callers render the flat list exactly as before statuses existed.
pub fn has_status_groups(statuses: &[Option<&str>], columns: &[BoardColumn]) -> bool {
    statuses
        .iter()
        .flatten()
        .any(|status| columns.iter().any(|c| &c.id == status))
}

/// Order the items whose statuses are `statuses` by column: columns in
/// settings order (empty ones skipped), then the unassigned bucket. Returns
/// `None` when nothing resolves to a column, so the caller keeps its flat
/// list. Each group's first row carries the caption.
pub fn grouped_rows(statuses: &[Option<&str>], columns: &[BoardColumn]) -> Option<Vec<GroupedRow>> {
    if !has_status_groups(statuses, columns) {
        return None;
    }
    let mut rows = Vec::with_capacity(statuses.len());
    for column in columns {
        let mut first = true;
        for (index, status) in statuses.iter().enumerate() {
            if *status == Some(column.id.as_str()) {
                rows.push(GroupedRow {
                    index,
                    caption: first.then(|| Some(column.clone())),
                });
                first = false;
            }
        }
    }
    let mut first = true;
    for (index, status) in statuses.iter().enumerate() {
        let assigned = status.is_some_and(|s| columns.iter().any(|c| c.id == s));
        if !assigned {
            rows.push(GroupedRow {
                index,
                caption: first.then_some(None),
            });
            first = false;
        }
    }
    Some(rows)
}

/// The caption text for a group.
pub fn caption_label(column: &Option<BoardColumn>) -> &str {
    column
        .as_ref()
        .map(|c| c.name.as_str())
        .unwrap_or("No status")
}

/// A card on the board: one project (a parent or a worktree).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoardCardRef {
    pub project_id: String,
}

/// One board column with its cards. Every column is present even when
/// empty, and so is the trailing unassigned column, since a board is also a
/// drop target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoardColumnModel {
    pub column: Option<BoardColumn>,
    pub cards: Vec<BoardCardRef>,
}

impl BoardColumnModel {
    pub fn label(&self) -> &str {
        caption_label(&self.column)
    }
}

/// Lay `projects` out on the board: columns in settings order plus the
/// unassigned column last, cards in the given project order.
pub fn board_model(projects: &[&ProjectData], columns: &[BoardColumn]) -> Vec<BoardColumnModel> {
    let mut model: Vec<BoardColumnModel> = columns
        .iter()
        .map(|column| BoardColumnModel {
            column: Some(column.clone()),
            cards: Vec::new(),
        })
        .collect();
    model.push(BoardColumnModel {
        column: None,
        cards: Vec::new(),
    });
    let unassigned = model.len() - 1;
    for project in projects {
        let slot = project
            .status
            .as_deref()
            .and_then(|status| columns.iter().position(|c| c.id == status))
            .unwrap_or(unassigned);
        model[slot].cards.push(BoardCardRef {
            project_id: project.id.clone(),
        });
    }
    model
}

/// The project a board opened from `project_id` covers: a worktree opens its
/// parent's board. Returns the root id and the ids of the root plus its
/// worktrees, in the parent's `worktree_ids` order.
pub fn board_family<'a>(
    projects: &'a [ProjectData],
    project_id: &str,
) -> Option<(String, Vec<&'a ProjectData>)> {
    let opened = projects.iter().find(|p| p.id == project_id)?;
    let root_id = opened
        .worktree_info
        .as_ref()
        .map(|wt| wt.parent_project_id.clone())
        .filter(|parent| projects.iter().any(|p| &p.id == parent))
        .unwrap_or_else(|| opened.id.clone());
    let root = projects.iter().find(|p| p.id == root_id)?;
    let mut family = vec![root];
    for wt_id in &root.worktree_ids {
        if let Some(wt) = projects.iter().find(|p| &p.id == wt_id) {
            family.push(wt);
        }
    }
    Some((root_id, family))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn columns() -> Vec<BoardColumn> {
        vec![
            BoardColumn::new("todo", "To do"),
            BoardColumn::new("review", "Review"),
            BoardColumn::new("done", "Done"),
        ]
    }

    fn project(id: &str, status: Option<&str>) -> ProjectData {
        ProjectData {
            id: id.to_string(),
            name: id.to_string(),
            path: format!("/{id}"),
            layout: None,
            terminal_names: HashMap::new(),
            status: status.map(str::to_string),
            hidden_terminals: HashMap::new(),
            worktree_info: None,
            worktree_ids: Vec::new(),
            folder_color: Default::default(),
            hooks: Default::default(),
            connection_id: None,
            service_terminals: HashMap::new(),
            agent_sessions: HashMap::new(),
            default_shell: None,
            hook_terminals: HashMap::new(),
            pinned: false,
            last_activity_at: None,
            is_creating: false,
            is_closing: false,
            creating_progress: None,
        }
    }

    fn worktree(id: &str, parent: &str, status: Option<&str>) -> ProjectData {
        let mut p = project(id, status);
        p.worktree_info = Some(okena_state::WorktreeMetadata {
            parent_project_id: parent.to_string(),
            color_override: None,
            main_repo_path: String::new(),
            worktree_path: String::new(),
            branch_name: String::new(),
        });
        p
    }

    #[test]
    fn no_resolvable_status_keeps_the_flat_list() {
        assert!(grouped_rows(&[None, None], &columns()).is_none());
        // A status whose column is gone does not count.
        assert!(grouped_rows(&[Some("gone"), None], &columns()).is_none());
    }

    #[test]
    fn rows_follow_column_order_then_unassigned_with_one_caption_per_group() {
        let statuses = [Some("done"), None, Some("todo"), Some("done"), Some("gone")];
        let rows = grouped_rows(&statuses, &columns()).unwrap();
        let shape: Vec<(usize, Option<Option<&str>>)> = rows
            .iter()
            .map(|r| {
                (
                    r.index,
                    r.caption
                        .as_ref()
                        .map(|c| c.as_ref().map(|c| c.id.as_str())),
                )
            })
            .collect();
        assert_eq!(
            shape,
            vec![
                (2, Some(Some("todo"))),
                (0, Some(Some("done"))),
                (3, None),
                (1, Some(None)),
                (4, None),
            ]
        );
        assert_eq!(
            caption_label(&rows[3].caption.clone().unwrap()),
            "No status"
        );
    }

    #[test]
    fn board_model_keeps_every_column_and_places_cards_by_status() {
        let a = project("a", Some("review"));
        let b = project("b", Some("nope"));
        let c = project("c", None);
        let model = board_model(&[&a, &b, &c], &columns());
        let shape: Vec<(Option<&str>, Vec<&str>)> = model
            .iter()
            .map(|col| {
                (
                    col.column.as_ref().map(|c| c.id.as_str()),
                    col.cards.iter().map(|r| r.project_id.as_str()).collect(),
                )
            })
            .collect();
        assert_eq!(
            shape,
            vec![
                (Some("todo"), vec![]),
                (Some("review"), vec!["a"]),
                (Some("done"), vec![]),
                (None, vec!["b", "c"]),
            ]
        );
        assert_eq!(model[3].label(), "No status");
    }

    #[test]
    fn board_family_opens_the_parent_from_a_worktree_in_worktree_order() {
        let mut root = project("root", None);
        root.worktree_ids = vec!["wt2".to_string(), "wt1".to_string()];
        let wt1 = worktree("wt1", "root", Some("done"));
        let wt2 = worktree("wt2", "root", None);
        let other = project("other", None);
        let projects = vec![wt1, other, root, wt2];

        let (root_id, family) = board_family(&projects, "wt1").unwrap();
        assert_eq!(root_id, "root");
        let ids: Vec<&str> = family.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, vec!["root", "wt2", "wt1"]);

        let (root_id, _) = board_family(&projects, "other").unwrap();
        assert_eq!(root_id, "other");
        assert!(board_family(&projects, "missing").is_none());
    }
}
