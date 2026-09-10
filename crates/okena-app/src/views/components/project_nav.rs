//! Getting from a panel to a project: focusing it, or opening its diff.
//!
//! Every panel that lists checkouts — an agent session's, a project's, a task's
//! detail — offers the same two ways out of it, and each had grown its own copy.
//! Free functions over the handles a host already holds, because the hosts are
//! different types.

use crate::workspace::focus::FocusManager;
use crate::workspace::state::{WindowId, Workspace};
use gpui::*;
use okena_workspace::request_broker::RequestBroker;
use okena_workspace::requests::{OverlayRequest, ProjectOverlay, ProjectOverlayKind};

/// Focus a project in the terminal workspace, leaving any harness view.
///
/// Leaving the harness view is the point: focusing something while a view
/// still covered the main area would look like nothing happened.
pub fn focus_project(
    workspace: &Entity<Workspace>,
    focus_manager: &Entity<FocusManager>,
    window_id: WindowId,
    project_id: &str,
    cx: &mut App,
) {
    let workspace = workspace.clone();
    let project_id = project_id.to_string();
    focus_manager.update(cx, |fm, cx| {
        workspace.update(cx, |ws, cx| {
            ws.set_focused_project_individual(fm, Some(project_id), cx);
        });
        cx.notify();
    });
    okena_workspace::harness_state::set_active_harness(window_id, None, cx);
}

/// Show what changed in a checkout.
pub fn open_diff(request_broker: &Entity<RequestBroker>, project_id: &str, cx: &mut App) {
    request_broker.update(cx, |broker, cx| {
        broker.push_overlay_request(
            OverlayRequest::Project(ProjectOverlay {
                project_id: project_id.to_string(),
                kind: ProjectOverlayKind::DiffViewer {
                    file: None,
                    mode: None,
                    commit_message: None,
                    commits: None,
                    commit_index: None,
                },
            }),
            cx,
        );
    });
}
