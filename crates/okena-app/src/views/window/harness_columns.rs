//! Harness views in the main content area.
//!
//! Exactly one view shows at a time and it takes the whole main area, replacing
//! the projects grid. Selecting the active entry again is a no-op rather than a
//! toggle — the view must not vanish under a user who clicks it twice. The way
//! back to the terminal workspace is the view's own close button.
//!
//! Pane entities are kept alive after being switched away from, so returning to
//! a view restores what it had loaded instead of refetching.

use crate::views::harness::{HarnessPane, HarnessPaneEvent};
use gpui::*;
use okena_core::harness::HarnessSection;

use super::WindowView;

impl WindowView {
    /// Show `section` full-width. A no-op if it is already showing.
    pub(crate) fn show_harness_view(&mut self, section: HarnessSection, cx: &mut Context<Self>) {
        if okena_workspace::harness_state::active_harness(self.window_id, cx) == Some(section) {
            return;
        }
        if !self.harness_panes.iter().any(|(s, _)| *s == section) {
            let client = match self.local_daemon_action_client(cx) {
                Ok(client) => client,
                Err(error) => {
                    crate::views::panels::toast::ToastManager::error(error, cx);
                    return;
                }
            };
            let ctx = crate::views::harness::PaneContext {
                client,
                workspace: self.workspace.clone(),
                focus_manager: self.focus_manager.clone(),
                window_id: self.window_id,
                terminals: self.terminals.clone(),
                active_drag: self.active_drag.clone(),
            };
            let pane = cx.new(|cx| HarnessPane::new(section, ctx, cx));
            cx.subscribe(
                &pane,
                move |this, _pane, event: &HarnessPaneEvent, cx| match event {
                    HarnessPaneEvent::Close(section) => this.close_harness_view(*section, cx),
                },
            )
            .detach();
            self.harness_panes.push((section, pane));
        }
        okena_workspace::harness_state::set_active_harness(self.window_id, Some(section), cx);
        cx.notify();
    }

    /// Return to the terminal workspace, dropping the view's state.
    pub(crate) fn close_harness_view(&mut self, section: HarnessSection, cx: &mut Context<Self>) {
        self.harness_panes.retain(|(s, _)| *s != section);
        if okena_workspace::harness_state::active_harness(self.window_id, cx) == Some(section) {
            okena_workspace::harness_state::set_active_harness(self.window_id, None, cx);
        }
        cx.notify();
    }

    /// The pane filling the main area, if a harness view is showing.
    pub(crate) fn active_harness_pane(&self, cx: &App) -> Option<Entity<HarnessPane>> {
        let section = okena_workspace::harness_state::active_harness(self.window_id, cx)?;
        self.harness_panes
            .iter()
            .find(|(s, _)| *s == section)
            .map(|(_, pane)| pane.clone())
    }
}
