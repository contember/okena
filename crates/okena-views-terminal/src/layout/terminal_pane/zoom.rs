//! Focus state and actions for running terminals.

use super::TerminalPane;
use crate::ActionDispatch;
use crate::layout::zoom_header::{ZoomHeader, navigate_zoom};
use gpui::*;

impl<D: ActionDispatch + Send + Sync> TerminalPane<D> {
    pub(super) fn is_zoomed(&self, cx: &Context<Self>) -> bool {
        let fm = self.focus_manager.read(cx);
        self.terminal_id
            .as_ref()
            .is_some_and(|id| fm.is_terminal_fullscreened(&self.project_id, id))
    }

    pub(super) fn navigate_zoom(&self, forward: bool, cx: &mut Context<Self>) {
        if self.is_zoomed(cx)
            && let Some(id) = &self.terminal_id
        {
            navigate_zoom(
                &self.workspace,
                self.action_dispatcher.as_ref(),
                &self.project_id,
                id,
                forward,
                cx,
            );
        }
    }

    pub(super) fn render_zoom_header(&self, cx: &Context<Self>) -> impl IntoElement {
        let project = self.workspace.read(cx).project(&self.project_id);
        let title = self
            .terminal_id
            .as_ref()
            .zip(project)
            .map(|(id, p)| {
                p.terminal_display_name(id, self.terminal.as_ref().and_then(|t| t.title()))
            })
            .unwrap_or_else(|| "Terminal".to_owned());
        let is_hook = self
            .terminal_id
            .as_ref()
            .zip(project)
            .is_some_and(|(id, p)| p.hook_terminals.contains_key(id));
        ZoomHeader {
            workspace: &self.workspace,
            request_broker: &self.request_broker,
            focus_manager: &self.focus_manager,
            dispatcher: &self.action_dispatcher,
            project_id: &self.project_id,
            layout_path: &self.layout_path,
            pane_id: self.terminal_id.as_deref(),
            focus: &self.focus_handle,
            shell: Some(self.shell_type.clone()),
            can_export_buffer: self.backend.supports_buffer_capture(),
        }
        .render(title, is_hook, cx)
    }
}
