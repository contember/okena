//! Status groups within a parent project's family in the sidebar.
//!
//! When any member of a family (the main checkout or one of its worktrees)
//! carries a status that resolves to a board column, the family's rows are
//! split under small non-cursor captions, one per non-empty group in the
//! board order ("No status" where the settings put it, last by default), the
//! main checkout included, the way the lanes group the same family in the
//! main area. Members drag from group to group (see [`plan_family_drop`]). A family without statuses renders exactly
//! as before: main checkout first, worktrees in data order. The grouping
//! itself is `okena_workspace::status_board`; the keyboard cursor walks the
//! same grouped order (`cursor.rs`), and the captions are folded into the
//! cursor's scroll-index map with [`fold_extra_rows`].

use gpui::{
    AnyElement, App, Context, Div, Entity, FontWeight, InteractiveElement, IntoElement,
    ParentElement, Stateful, Styled, div, px, rgb,
};
use okena_core::api::ActionRequest;
use okena_core::theme::FolderColor;
use okena_ui::board::StatusBoard;
use okena_ui::theme::theme;
use okena_ui::tokens::ui_text_xs;
use okena_workspace::state::Workspace;
use okena_workspace::status_board::{GroupedRow, caption_label, grouped_rows};
use std::rc::Rc;

use super::Sidebar;
use crate::drag::WorktreeDrag;

/// Which member of a family a row draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FamilyMember {
    /// The parent project's own checkout.
    Main,
    /// A worktree, by its index into the family's worktree children.
    Worktree(usize),
}

/// The caption above a status group: its label, colour and column (`None`
/// for "No status"), which a member dropped on it moves to.
pub(crate) struct GroupCaption {
    pub label: String,
    pub color: FolderColor,
    pub column: Option<String>,
}

/// One family row to draw: the member and, on the first row of a group, the
/// caption to draw above it.
pub(crate) struct FamilyRow {
    pub member: FamilyMember,
    pub caption: Option<GroupCaption>,
}

/// The order to draw a family in: the main checkout (status `main_status`)
/// and its worktrees (`worktree_statuses`). Grouped by status, the main
/// checkout among them, when any status resolves to a column; main first
/// and worktrees as given otherwise. The render and the keyboard cursor
/// both walk this order.
pub(crate) fn family_rows(
    main_status: Option<&str>,
    worktree_statuses: &[Option<&str>],
    cx: &App,
) -> Vec<FamilyRow> {
    group_family_rows(
        main_status,
        worktree_statuses,
        okena_ui::board::status_board(cx),
    )
}

fn group_family_rows(
    main_status: Option<&str>,
    worktree_statuses: &[Option<&str>],
    board: StatusBoard<'_>,
) -> Vec<FamilyRow> {
    // Member 0 is the main checkout, member `i + 1` worktree `i`.
    let member = |index: usize| match index {
        0 => FamilyMember::Main,
        i => FamilyMember::Worktree(i - 1),
    };
    let statuses: Vec<Option<&str>> = std::iter::once(main_status)
        .chain(worktree_statuses.iter().copied())
        .collect();
    let grouped = if board.enabled {
        grouped_rows(&statuses, board.columns, board.no_status_at)
    } else {
        None
    };
    match grouped {
        Some(rows) => rows
            .into_iter()
            .map(|GroupedRow { index, caption }| FamilyRow {
                member: member(index),
                caption: caption.map(|column| GroupCaption {
                    label: caption_label(&column).to_string(),
                    color: board.group_color(column.as_ref()),
                    column: column.map(|c| c.id),
                }),
            })
            .collect(),
        None => (0..statuses.len())
            .map(|index| FamilyRow {
                member: member(index),
                caption: None,
            })
            .collect(),
    }
}

/// The group a member with `status` is drawn in: its status when that names
/// a column of an enabled board, `None` otherwise ("No status", or the flat
/// list).
pub(crate) fn status_group<'a>(status: Option<&'a str>, board: StatusBoard<'_>) -> Option<&'a str> {
    status.filter(|id| board.enabled && board.columns.iter().any(|c| c.id == *id))
}

/// A drop target in a family, owned by the row or caption that draws it.
pub(crate) enum FamilyDropSpot {
    /// A member's row: its project, status and, for a worktree, its data
    /// index.
    Row {
        project_id: String,
        status: Option<String>,
        worktree_index: Option<usize>,
    },
    /// A group caption: its column (`None` = "No status").
    Caption { column: Option<String> },
}

/// What dropping `drag` on `spot` in family `family_id` does; nothing for a
/// member of another family.
pub(crate) fn family_drop_plan(
    workspace: &Entity<Workspace>,
    drag: &WorktreeDrag,
    family_id: &str,
    spot: &FamilyDropSpot,
    cx: &App,
) -> FamilyDrop {
    if drag.parent_id != family_id {
        return FamilyDrop::default();
    }
    let (onto_itself, target) = match spot {
        FamilyDropSpot::Row {
            project_id,
            status,
            worktree_index,
        } => (
            *project_id == drag.worktree_id,
            FamilyDropTarget::Row {
                status: status.as_deref(),
                worktree_index: *worktree_index,
            },
        ),
        FamilyDropSpot::Caption { column } => (
            false,
            FamilyDropTarget::Caption {
                column: column.as_deref(),
            },
        ),
    };
    let dragged_status = workspace
        .read(cx)
        .project(&drag.worktree_id)
        .and_then(|p| p.status.as_deref());
    plan_family_drop(
        dragged_status,
        drag.main_checkout,
        onto_itself,
        target,
        okena_ui::board::status_board(cx),
    )
}

/// Where a family member was dropped.
#[derive(Clone, Copy, Debug)]
pub(crate) enum FamilyDropTarget<'a> {
    /// A member's row: its status and, for a worktree, its data index.
    Row {
        status: Option<&'a str>,
        worktree_index: Option<usize>,
    },
    /// A group caption: its column (`None` = "No status").
    Caption { column: Option<&'a str> },
}

/// What a drop within one family does.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct FamilyDrop {
    /// Set the dragged member's status (`Some(None)` clears it).
    pub status: Option<Option<String>>,
    /// Move the dragged worktree to this index of the worktree order.
    pub reorder_to: Option<usize>,
}

/// Plan a drop of a family member (status `dragged_status`) on `target` in
/// the same family. Dropped into another status group, the member takes
/// that group's status ("No status" clears it); a worktree dropped on a
/// worktree row also moves to that row's place in the worktree order, so it
/// lands next to it. The main checkout has no place in that order.
pub(crate) fn plan_family_drop(
    dragged_status: Option<&str>,
    main_checkout: bool,
    onto_itself: bool,
    target: FamilyDropTarget<'_>,
    board: StatusBoard<'_>,
) -> FamilyDrop {
    if onto_itself {
        return FamilyDrop::default();
    }
    let (target_group, worktree_index) = match target {
        FamilyDropTarget::Row {
            status,
            worktree_index,
        } => (status_group(status, board), worktree_index),
        FamilyDropTarget::Caption { column } => (status_group(column, board), None),
    };
    let status = (board.enabled && status_group(dragged_status, board) != target_group)
        .then(|| target_group.map(str::to_string));
    let reorder_to = worktree_index.filter(|_| !main_checkout);
    FamilyDrop { status, reorder_to }
}

/// Shift each cursor item's scroll-child index by the number of non-cursor
/// rows (`extra_rows`, each recorded as the `flat_idx` it was inserted at)
/// that precede it. A row inserted at position `h` sits before cursor item
/// `h` and every later one.
pub(crate) fn fold_extra_rows(scroll_indices: &mut [usize], extra_rows: &[usize]) {
    if extra_rows.is_empty() {
        return;
    }
    for (i, target) in scroll_indices.iter_mut().enumerate() {
        *target += extra_rows.iter().filter(|&&h| h <= i).count();
    }
}

/// Width of the family guide line. Whole pixels with a whole-pixel centre
/// keep it crisp on 1x and 2x displays.
const TREE_GUIDE_WIDTH: f32 = 2.0;

/// Draw a thin tree guide in the parent's colour down the left of every
/// row of an expanded family (main checkout, captions, worktrees and their
/// expanded children), centred under the expand arrow of the family's
/// header row, whose left padding is `header_padding`. Each row is wrapped
/// so consecutive rows join into one continuous line; the row itself is
/// untouched.
pub(crate) fn add_tree_guide(rows: &mut [AnyElement], header_padding: f32, color: u32) {
    let centre = header_padding + crate::item_widgets::EXPAND_ARROW_WIDTH / 2.0;
    let mut line = rgb(color);
    line.a = 0.55;
    for row in rows.iter_mut() {
        let inner = std::mem::replace(row, div().into_any_element());
        *row = div()
            .relative()
            .w_full()
            .child(inner)
            .child(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(px(centre - TREE_GUIDE_WIDTH / 2.0))
                    .w(px(TREE_GUIDE_WIDTH))
                    .bg(line),
            )
            .into_any_element();
    }
}

impl Sidebar {
    /// Small caption between family rows: `TO DO`, `REVIEW`, … in the
    /// column's colour (muted when it has none). A member of the family
    /// `parent_id` dropped on it moves to the caption's status.
    pub(crate) fn render_status_group_header(
        &self,
        caption: &GroupCaption,
        parent_id: &str,
        left_padding: f32,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let t = theme(cx);
        let text_color = match caption.color {
            FolderColor::Default => t.text_muted,
            color => t.get_folder_color(color),
        };
        let header = div()
            .id(gpui::ElementId::Name(
                format!(
                    "status-group-{parent_id}-{}",
                    caption.column.as_deref().unwrap_or("none")
                )
                .into(),
            ))
            .h(px(18.0))
            .pl(px(left_padding))
            .pr(px(8.0))
            .flex()
            .items_center()
            .gap(px(5.0))
            .child(
                div()
                    .size(px(6.0))
                    .rounded_full()
                    .bg(rgb(text_color))
                    .flex_shrink_0(),
            )
            .child(
                div()
                    .text_size(ui_text_xs(cx))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(text_color))
                    .child(caption.label.to_uppercase()),
            );
        self.family_drop_target(
            header,
            parent_id.to_string(),
            FamilyDropSpot::Caption {
                column: caption.column.clone(),
            },
            cx,
        )
    }

    /// Make `element` (a row or caption of family `family_id`) a drop target
    /// for the family's members. It only accepts, and only lights up for, a
    /// drop that changes something: a reorder draws an insert line above
    /// it, a status-only move tints it.
    pub(crate) fn family_drop_target(
        &self,
        element: Stateful<Div>,
        family_id: String,
        spot: FamilyDropSpot,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let t = theme(cx);
        let spot = Rc::new(spot);
        let family_id: Rc<str> = family_id.into();
        element
            .drag_over::<WorktreeDrag>({
                let workspace = self.workspace.clone();
                let spot = spot.clone();
                let family_id = family_id.clone();
                move |style, drag, _window, cx| {
                    let plan = family_drop_plan(&workspace, drag, &family_id, &spot, cx);
                    if plan.reorder_to.is_some() {
                        style.border_t_2().border_color(rgb(t.border_active))
                    } else {
                        style.bg(rgb(t.bg_hover))
                    }
                }
            })
            .can_drop({
                let workspace = self.workspace.clone();
                let spot = spot.clone();
                let family_id = family_id.clone();
                move |dragged, _window, cx| match dragged.downcast_ref::<WorktreeDrag>() {
                    Some(drag) => {
                        family_drop_plan(&workspace, drag, &family_id, &spot, cx)
                            != FamilyDrop::default()
                    }
                    None => true,
                }
            })
            .on_drop(cx.listener(move |this, drag: &WorktreeDrag, _window, cx| {
                let plan = family_drop_plan(&this.workspace, drag, &family_id, &spot, cx);
                this.apply_family_drop(drag, plan, cx);
            }))
    }

    /// Send the actions a planned drop of `drag` makes.
    fn apply_family_drop(&self, drag: &WorktreeDrag, plan: FamilyDrop, cx: &mut Context<Self>) {
        if let Some(status) = plan.status {
            self.dispatch_action_for_project(
                &drag.worktree_id,
                ActionRequest::SetProjectStatus {
                    project_id: drag.worktree_id.clone(),
                    status,
                },
                cx,
            );
        }
        if let Some(new_index) = plan.reorder_to {
            self.dispatch_action_for_project(
                &drag.parent_id,
                ActionRequest::ReorderWorktree {
                    parent_id: drag.parent_id.clone(),
                    worktree_id: drag.worktree_id.clone(),
                    new_index,
                },
                cx,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        FamilyDrop, FamilyDropTarget, FamilyMember, FamilyRow, StatusBoard, fold_extra_rows,
        group_family_rows, plan_family_drop, status_group,
    };
    use okena_core::theme::FolderColor;
    use okena_core::types::BoardColumn;

    fn columns() -> Vec<BoardColumn> {
        vec![
            BoardColumn::new("todo", "To do"),
            BoardColumn::new("done", "Done"),
        ]
    }

    fn shape(rows: &[FamilyRow]) -> Vec<(FamilyMember, Option<&str>)> {
        rows.iter()
            .map(|r| (r.member, r.caption.as_ref().map(|c| c.label.as_str())))
            .collect()
    }

    use FamilyMember::{Main, Worktree};

    #[test]
    fn board_off_keeps_main_first_and_worktrees_in_data_order() {
        let columns = columns();
        let board = StatusBoard {
            enabled: false,
            columns: &columns,
            no_status_color: FolderColor::Default,
            no_status_at: usize::MAX,
        };
        let rows = group_family_rows(Some("todo"), &[Some("done"), None, Some("todo")], board);
        assert_eq!(
            shape(&rows),
            vec![
                (Main, None),
                (Worktree(0), None),
                (Worktree(1), None),
                (Worktree(2), None)
            ]
        );
    }

    #[test]
    fn board_on_groups_the_main_checkout_with_its_worktrees() {
        let columns = columns();
        let board = StatusBoard {
            enabled: true,
            columns: &columns,
            no_status_color: FolderColor::Default,
            no_status_at: usize::MAX,
        };
        let rows = group_family_rows(Some("done"), &[Some("done"), None, Some("todo")], board);
        assert_eq!(
            shape(&rows),
            vec![
                (Worktree(2), Some("To do")),
                (Main, Some("Done")),
                (Worktree(0), None),
                (Worktree(1), Some("No status")),
            ]
        );
    }

    #[test]
    fn a_status_on_the_main_checkout_alone_groups_the_family() {
        let columns = columns();
        let board = StatusBoard {
            enabled: true,
            columns: &columns,
            no_status_color: FolderColor::Default,
            no_status_at: usize::MAX,
        };
        let rows = group_family_rows(Some("todo"), &[None, None], board);
        assert_eq!(
            shape(&rows),
            vec![
                (Main, Some("To do")),
                (Worktree(0), Some("No status")),
                (Worktree(1), None),
            ]
        );
    }

    #[test]
    fn a_family_without_statuses_stays_flat() {
        let columns = columns();
        let board = StatusBoard {
            enabled: true,
            columns: &columns,
            no_status_color: FolderColor::Default,
            no_status_at: usize::MAX,
        };
        let rows = group_family_rows(None, &[None, Some("gone")], board);
        assert_eq!(
            shape(&rows),
            vec![(Main, None), (Worktree(0), None), (Worktree(1), None)]
        );
    }

    #[test]
    fn no_status_caption_takes_the_no_status_color() {
        let columns = columns();
        let board = StatusBoard {
            enabled: true,
            columns: &columns,
            no_status_color: FolderColor::Blue,
            no_status_at: usize::MAX,
        };
        let rows = group_family_rows(Some("todo"), &[None], board);
        let captions: Vec<FolderColor> = rows
            .iter()
            .filter_map(|r| r.caption.as_ref().map(|c| c.color))
            .collect();
        let todo = columns.iter().find(|c| c.id == "todo").unwrap().color;
        assert_eq!(captions, vec![todo, FolderColor::Blue]);
    }

    #[test]
    fn status_group_merges_unknown_into_no_status_and_ignores_a_disabled_board() {
        let columns = columns();
        let on = StatusBoard {
            enabled: true,
            columns: &columns,
            no_status_color: FolderColor::Default,
            no_status_at: usize::MAX,
        };
        assert_eq!(status_group(Some("todo"), on), Some("todo"));
        assert_eq!(status_group(Some("gone"), on), None);
        assert_eq!(status_group(None, on), None);
        let off = StatusBoard {
            enabled: false,
            columns: &columns,
            no_status_color: FolderColor::Default,
            no_status_at: usize::MAX,
        };
        assert_eq!(status_group(Some("todo"), off), None);
    }

    fn on() -> StatusBoard<'static> {
        static COLUMNS: std::sync::LazyLock<Vec<BoardColumn>> = std::sync::LazyLock::new(columns);
        StatusBoard {
            enabled: true,
            columns: &COLUMNS,
            no_status_color: FolderColor::Default,
            no_status_at: usize::MAX,
        }
    }

    fn row(status: Option<&str>, worktree_index: Option<usize>) -> FamilyDropTarget<'_> {
        FamilyDropTarget::Row {
            status,
            worktree_index,
        }
    }

    #[test]
    fn dropping_into_another_group_takes_its_status_and_place() {
        // A worktree onto a worktree in "Done": status and order.
        assert_eq!(
            plan_family_drop(Some("todo"), false, false, row(Some("done"), Some(3)), on()),
            FamilyDrop {
                status: Some(Some("done".into())),
                reorder_to: Some(3),
            }
        );
        // Onto a row in "No status" (an unknown id counts as none): cleared.
        assert_eq!(
            plan_family_drop(Some("todo"), false, false, row(Some("gone"), Some(0)), on()),
            FamilyDrop {
                status: Some(None),
                reorder_to: Some(0),
            }
        );
        // Onto the main checkout's row: its status, no place to take.
        assert_eq!(
            plan_family_drop(None, false, false, row(Some("todo"), None), on()),
            FamilyDrop {
                status: Some(Some("todo".into())),
                reorder_to: None,
            }
        );
    }

    #[test]
    fn dropping_on_a_caption_only_sets_the_status() {
        let caption = |column| FamilyDropTarget::Caption { column };
        assert_eq!(
            plan_family_drop(None, false, false, caption(Some("done")), on()),
            FamilyDrop {
                status: Some(Some("done".into())),
                reorder_to: None,
            }
        );
        assert_eq!(
            plan_family_drop(Some("done"), true, false, caption(None), on()),
            FamilyDrop {
                status: Some(None),
                reorder_to: None,
            }
        );
        // Its own group's caption: nothing.
        assert_eq!(
            plan_family_drop(Some("done"), false, false, caption(Some("done")), on()),
            FamilyDrop::default()
        );
    }

    #[test]
    fn dropping_within_a_group_only_reorders() {
        assert_eq!(
            plan_family_drop(Some("done"), false, false, row(Some("done"), Some(1)), on()),
            FamilyDrop {
                status: None,
                reorder_to: Some(1),
            }
        );
        // The main checkout never takes a place in the worktree order.
        assert_eq!(
            plan_family_drop(Some("done"), true, false, row(Some("done"), Some(1)), on()),
            FamilyDrop::default()
        );
        // Onto itself: nothing.
        assert_eq!(
            plan_family_drop(Some("todo"), false, true, row(Some("done"), Some(1)), on()),
            FamilyDrop::default()
        );
    }

    #[test]
    fn a_disabled_board_keeps_drops_to_reordering() {
        let columns = columns();
        let off = StatusBoard {
            enabled: false,
            columns: &columns,
            no_status_color: FolderColor::Default,
            no_status_at: usize::MAX,
        };
        assert_eq!(
            plan_family_drop(Some("todo"), false, false, row(Some("done"), Some(2)), off),
            FamilyDrop {
                status: None,
                reorder_to: Some(2),
            }
        );
    }

    #[test]
    fn fold_extra_rows_counts_headers_at_or_before_each_item() {
        // Cursor items 0..5 map 1:1 to scroll children starting at 1; headers
        // were inserted before items 1 and 3.
        let mut indices: Vec<usize> = (1..=5).collect();
        fold_extra_rows(&mut indices, &[1, 3]);
        assert_eq!(indices, vec![1, 3, 4, 6, 7]);
    }

    #[test]
    fn fold_extra_rows_is_identity_without_headers() {
        let mut indices = vec![2, 3, 4];
        fold_extra_rows(&mut indices, &[]);
        assert_eq!(indices, vec![2, 3, 4]);
    }
}
