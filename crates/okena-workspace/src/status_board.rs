//! Status board: the pure grouping the sidebar captions and the main-area
//! lanes draw from. GPUI-free.
//!
//! A status is a user-set column id on a project (`ProjectData::status`),
//! typically on the worktrees of one parent project. Grouping resolves ids
//! against the ordered columns from settings: columns first, in their order,
//! then an unassigned bucket for projects with no status or a status whose
//! column no longer exists. Input order is kept inside every group.

use okena_core::types::BoardColumn;

/// One row of a grouped list: the item's original index and, on the first
/// row of each group, the caption to draw above it (`Some(None)` opens the
/// "No status" group).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GroupedRow {
    pub index: usize,
    pub caption: Option<Option<BoardColumn>>,
}

/// One lane of the main area: a column (or `None` for "No status") and the
/// original indices of the items in it. Only non-empty lanes exist.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lane {
    pub column: Option<BoardColumn>,
    pub indices: Vec<usize>,
}

impl Lane {
    pub fn label(&self) -> &str {
        caption_label(&self.column)
    }
}

/// Whether any of `statuses` resolves to one of `columns`. When false,
/// callers render the flat list exactly as before statuses existed.
pub fn has_status_groups(statuses: &[Option<&str>], columns: &[BoardColumn]) -> bool {
    statuses
        .iter()
        .flatten()
        .any(|status| columns.iter().any(|c| &c.id == status))
}

/// Group the items whose statuses are `statuses` by column: columns in
/// settings order (empty ones skipped), then the unassigned bucket. Returns
/// `None` when nothing resolves to a column, so the caller keeps its flat
/// list.
pub fn lanes(statuses: &[Option<&str>], columns: &[BoardColumn]) -> Option<Vec<Lane>> {
    if !has_status_groups(statuses, columns) {
        return None;
    }
    let mut lanes: Vec<Lane> = columns
        .iter()
        .map(|column| Lane {
            column: Some(column.clone()),
            indices: Vec::new(),
        })
        .collect();
    let mut unassigned = Vec::new();
    for (index, status) in statuses.iter().enumerate() {
        match status.and_then(|s| columns.iter().position(|c| c.id == s)) {
            Some(slot) => lanes[slot].indices.push(index),
            None => unassigned.push(index),
        }
    }
    lanes.retain(|lane| !lane.indices.is_empty());
    if !unassigned.is_empty() {
        lanes.push(Lane {
            column: None,
            indices: unassigned,
        });
    }
    Some(lanes)
}

/// [`lanes`] flattened into rows, each group's first row carrying the
/// caption. `None` when nothing resolves to a column.
pub fn grouped_rows(statuses: &[Option<&str>], columns: &[BoardColumn]) -> Option<Vec<GroupedRow>> {
    let lanes = lanes(statuses, columns)?;
    let mut rows = Vec::with_capacity(statuses.len());
    for lane in lanes {
        for (i, index) in lane.indices.into_iter().enumerate() {
            rows.push(GroupedRow {
                index,
                caption: (i == 0).then(|| lane.column.clone()),
            });
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

#[cfg(test)]
mod tests {
    use super::*;

    fn columns() -> Vec<BoardColumn> {
        vec![
            BoardColumn::new("todo", "To do"),
            BoardColumn::new("review", "Review"),
            BoardColumn::new("done", "Done"),
        ]
    }

    #[test]
    fn no_resolvable_status_keeps_the_flat_list() {
        assert!(lanes(&[None, None], &columns()).is_none());
        assert!(grouped_rows(&[None, None], &columns()).is_none());
        // A status whose column is gone does not count.
        assert!(lanes(&[Some("gone"), None], &columns()).is_none());
    }

    #[test]
    fn lanes_follow_column_order_skip_empty_and_put_unassigned_last() {
        let statuses = [Some("done"), None, Some("todo"), Some("done"), Some("gone")];
        let lanes = lanes(&statuses, &columns()).unwrap();
        let shape: Vec<(Option<&str>, Vec<usize>)> = lanes
            .iter()
            .map(|l| (l.column.as_ref().map(|c| c.id.as_str()), l.indices.clone()))
            .collect();
        assert_eq!(
            shape,
            vec![
                (Some("todo"), vec![2]),
                (Some("done"), vec![0, 3]),
                (None, vec![1, 4]),
            ]
        );
        assert_eq!(lanes[2].label(), "No status");
    }

    #[test]
    fn rows_carry_one_caption_per_group() {
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
}
