//! External presentation requests (`okena project show|hide`, `okena term
//! fullscreen`) pushed by the daemon after the action succeeded there. The
//! desktop owns per-window presentation, so it applies each request to its own
//! window here; the daemon's copy only serves its synthetic main window.

use crate::workspace::state::WindowId;
use gpui::*;

use super::Okena;

impl Okena {
    /// The window an external request lands in: the requested one, else the
    /// active desktop window, else main.
    fn presentation_target_window(&self, requested: Option<WindowId>, cx: &App) -> WindowId {
        requested
            .or_else(|| self.active_window_id(cx))
            .unwrap_or(WindowId::Main)
    }

    /// The main or extra window that has OS focus. `None` when another
    /// window (a detached terminal or overlay) or no Okena window is active.
    fn active_window_id(&self, cx: &App) -> Option<WindowId> {
        let active = cx.active_window()?;
        if active == self.main_window_handle {
            return Some(WindowId::Main);
        }
        self.extra_window_handles
            .iter()
            .find(|(_, handle)| **handle == active)
            .map(|(id, _)| *id)
    }

    /// Show or hide `project_id` in the target window. Idempotent; a project
    /// not synced yet (`project add --hidden`) is applied once it arrives.
    pub(super) fn apply_project_visibility_request(
        &mut self,
        project_id: &str,
        show: bool,
        requested_window: Option<WindowId>,
        cx: &mut Context<Self>,
    ) {
        let target = self.presentation_target_window(requested_window, cx);
        let Some((view, _)) = self.window_view_and_handle(target) else {
            log::warn!("Ignoring project visibility request for unknown window {target:?}");
            return;
        };
        let workspace = self.workspace.clone();
        let focus_manager = view.read(cx).focus_manager();
        focus_manager.update(cx, |fm, cx| {
            workspace.update(cx, |ws, cx| {
                ws.request_project_overview_visibility(fm, target, project_id, show, cx);
            });
            cx.notify();
        });
    }

    /// Enter fullscreen on `terminal_id`, or exit fullscreen when it is `None`,
    /// in the target window.
    pub(super) fn apply_fullscreen_request(
        &mut self,
        project_id: &str,
        terminal_id: Option<&str>,
        requested_window: Option<WindowId>,
        cx: &mut Context<Self>,
    ) {
        let target = self.presentation_target_window(requested_window, cx);
        let Some((view, _)) = self.window_view_and_handle(target) else {
            log::warn!("Ignoring fullscreen request for unknown window {target:?}");
            return;
        };
        if let Some(terminal_id) = terminal_id {
            let terminal_known = self
                .workspace
                .read(cx)
                .project(project_id)
                .and_then(|project| project.layout.as_ref())
                .is_some_and(|layout| layout.find_terminal_path(terminal_id).is_some());
            if !terminal_known {
                log::warn!("Ignoring fullscreen request for unknown terminal {terminal_id}");
                return;
            }
        }
        let workspace = self.workspace.clone();
        let focus_manager = view.read(cx).focus_manager();
        focus_manager.update(cx, |fm, cx| {
            workspace.update(cx, |ws, cx| match terminal_id {
                Some(terminal_id) => ws.set_fullscreen_terminal(
                    fm,
                    project_id.to_string(),
                    terminal_id.to_string(),
                    cx,
                ),
                None => ws.exit_fullscreen(fm, cx),
            });
            cx.notify();
        });
    }
}
