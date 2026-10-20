//! Terminal status board: the pure grouping both the sidebar and the board
//! overlay draw from. GPUI-free.
//!
//! A status is a user-set column id on a terminal
//! (`ProjectData::terminal_statuses`). Grouping resolves ids against the
//! ordered columns from settings: columns first, in their order, then an
//! unassigned bucket for terminals with no status or a status whose column
//! no longer exists. Input order is kept inside every group.

use okena_core::types::BoardColumn;
use std::collections::HashMap;

/// One group of terminals: a column, or `None` for the unassigned bucket.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusGroup {
    pub column: Option<BoardColumn>,
    pub terminal_ids: Vec<String>,
}

impl StatusGroup {
    /// The label a header shows for this group.
    pub fn label(&self) -> &str {
        self.column
            .as_ref()
            .map(|c| c.name.as_str())
            .unwrap_or("No status")
    }
}

/// Whether any of `terminal_ids` carries a status that resolves to one of
/// `columns`. When false, callers render the flat list exactly as before
/// statuses existed.
pub fn has_status_groups(
    terminal_ids: &[String],
    statuses: &HashMap<String, String>,
    columns: &[BoardColumn],
) -> bool {
    terminal_ids.iter().any(|tid| {
        statuses
            .get(tid)
            .is_some_and(|status| columns.iter().any(|c| &c.id == status))
    })
}

/// Group `terminal_ids` by status. Empty columns are skipped; the unassigned
/// bucket comes last and only when non-empty. With no resolvable status at
/// all the result is a single unassigned group holding every terminal.
pub fn group_terminals_by_status(
    terminal_ids: &[String],
    statuses: &HashMap<String, String>,
    columns: &[BoardColumn],
) -> Vec<StatusGroup> {
    let mut groups: Vec<StatusGroup> = columns
        .iter()
        .map(|column| StatusGroup {
            column: Some(column.clone()),
            terminal_ids: Vec::new(),
        })
        .collect();
    let mut unassigned = Vec::new();
    for tid in terminal_ids {
        let slot = statuses
            .get(tid)
            .and_then(|status| groups.iter_mut().find(|g| g.column.as_ref().is_some_and(|c| &c.id == status)));
        match slot {
            Some(group) => group.terminal_ids.push(tid.clone()),
            None => unassigned.push(tid.clone()),
        }
    }
    groups.retain(|g| !g.terminal_ids.is_empty());
    if !unassigned.is_empty() || groups.is_empty() {
        groups.push(StatusGroup {
            column: None,
            terminal_ids: unassigned,
        });
    }
    groups
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

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn no_statuses_is_one_unassigned_group_in_input_order() {
        let groups = group_terminals_by_status(&ids(&["t2", "t1"]), &HashMap::new(), &columns());
        assert_eq!(
            groups,
            vec![StatusGroup {
                column: None,
                terminal_ids: ids(&["t2", "t1"])
            }]
        );
        assert!(!has_status_groups(&ids(&["t2", "t1"]), &HashMap::new(), &columns()));
        assert_eq!(groups[0].label(), "No status");
    }

    #[test]
    fn groups_follow_column_order_skip_empty_and_put_unassigned_last() {
        let statuses: HashMap<String, String> = [
            ("t1", "done"),
            ("t3", "todo"),
            ("t4", "done"),
            ("t5", "gone"), // column removed → unassigned
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        let groups =
            group_terminals_by_status(&ids(&["t1", "t2", "t3", "t4", "t5"]), &statuses, &columns());
        let shape: Vec<(Option<&str>, Vec<&str>)> = groups
            .iter()
            .map(|g| {
                (
                    g.column.as_ref().map(|c| c.id.as_str()),
                    g.terminal_ids.iter().map(String::as_str).collect(),
                )
            })
            .collect();
        assert_eq!(
            shape,
            vec![
                (Some("todo"), vec!["t3"]),
                (Some("done"), vec!["t1", "t4"]),
                (None, vec!["t2", "t5"]),
            ]
        );
        assert!(has_status_groups(&ids(&["t1"]), &statuses, &columns()));
        // A status with no column does not count as grouped.
        assert!(!has_status_groups(&ids(&["t5"]), &statuses, &columns()));
    }

    #[test]
    fn empty_input_yields_one_empty_unassigned_group() {
        let groups = group_terminals_by_status(&[], &HashMap::new(), &columns());
        assert_eq!(groups.len(), 1);
        assert!(groups[0].column.is_none());
        assert!(groups[0].terminal_ids.is_empty());
    }
}
