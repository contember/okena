//! The keyboard cursor walks a family's worktrees in the order the rows are
//! drawn: data order while the status board is off, grouped by status while
//! it is on.

use super::{Sidebar, SidebarCursorItem};
use gpui::{App, AppContext as _, Entity, TestAppContext};
use okena_core::types::BoardColumn;
use okena_ui::board::{GlobalStatusBoard, StatusBoard};
use okena_workspace::focus::FocusManager;
use okena_workspace::request_broker::RequestBroker;
use okena_workspace::state::{WindowId, Workspace, WorkspaceData};
use std::sync::LazyLock;

static COLUMNS: LazyLock<Vec<BoardColumn>> = LazyLock::new(|| {
    vec![
        BoardColumn::new("todo", "To do"),
        BoardColumn::new("done", "Done"),
    ]
});

fn board_on(_: &App) -> StatusBoard<'_> {
    StatusBoard {
        enabled: true,
        columns: &COLUMNS,
    }
}

fn board_off(_: &App) -> StatusBoard<'_> {
    StatusBoard {
        enabled: false,
        columns: &COLUMNS,
    }
}

/// A parent with worktrees `a` (done), `b` (none) and `c` (todo), in that
/// data order.
fn family() -> WorkspaceData {
    let worktree = |id: &str, status: Option<&str>| {
        serde_json::json!({
            "id": id,
            "name": id,
            "path": format!("/tmp/{id}"),
            "status": status,
            "worktree_info": { "parent_project_id": "parent" },
        })
    };
    serde_json::from_value(serde_json::json!({
        "version": 1,
        "projects": [
            {
                "id": "parent",
                "name": "parent",
                "path": "/tmp/parent",
                "worktree_ids": ["a", "b", "c"],
            },
            worktree("a", Some("done")),
            worktree("b", None),
            worktree("c", Some("todo")),
        ],
        "project_order": ["parent"],
    }))
    .unwrap()
}

fn new_sidebar(cx: &mut TestAppContext) -> Entity<Sidebar> {
    cx.update(|cx| {
        let workspace = cx.new(|_| Workspace::new(family()));
        let focus_manager = cx.new(|_| FocusManager::new());
        let request_broker = cx.new(|_| RequestBroker::new());
        cx.new(|cx| {
            Sidebar::new(
                WindowId::Main,
                workspace,
                focus_manager,
                request_broker,
                Default::default(),
                cx,
            )
        })
    })
}

fn worktree_cursor_order(sidebar: &Entity<Sidebar>, cx: &mut TestAppContext) -> Vec<String> {
    sidebar.update(cx, |sidebar, cx| {
        sidebar
            .build_cursor_items(cx)
            .into_iter()
            .filter_map(|item| match item {
                SidebarCursorItem::WorktreeProject { project_id } => Some(project_id),
                _ => None,
            })
            .collect()
    })
}

#[gpui::test]
fn board_off_cursor_walks_worktrees_in_data_order(cx: &mut TestAppContext) {
    cx.update(|cx| cx.set_global(GlobalStatusBoard(board_off)));
    let sidebar = new_sidebar(cx);
    assert_eq!(
        worktree_cursor_order(&sidebar, cx),
        vec!["parent", "a", "b", "c"]
    );
}

#[gpui::test]
fn board_on_cursor_walks_worktrees_grouped_by_status(cx: &mut TestAppContext) {
    cx.update(|cx| cx.set_global(GlobalStatusBoard(board_on)));
    let sidebar = new_sidebar(cx);
    // To do (c), Done (a), then No status (b); the main checkout stays first.
    assert_eq!(
        worktree_cursor_order(&sidebar, cx),
        vec!["parent", "c", "a", "b"]
    );
}
