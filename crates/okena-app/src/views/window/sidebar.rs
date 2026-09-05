use crate::settings::settings_entity;
use gpui::*;

use super::WindowView;

impl WindowView {
    /// Toggle sidebar visibility.
    pub(super) fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        self.sidebar_ctrl.toggle();
        let open = self.sidebar_ctrl.is_open();
        let window_id = self.window_id;
        // Persist per-window so each viewport remembers its own sidebar
        // state. Also keep the global setting in sync so a fresh-spawned
        // window (which uses the global as its first-launch default before
        // its own WindowState entry exists) opens with the user's most
        // recent preference.
        self.workspace
            .update(cx, |ws, cx| ws.set_sidebar_open(window_id, open, cx));
        settings_entity(cx).update(cx, |s, cx| s.set_sidebar_open(open, cx));
        self.sync_status_bar_sidebar_state(cx);
        cx.notify();
    }

    /// Sync sidebar open state to the status bar and title bar for icon highlighting
    fn sync_status_bar_sidebar_state(&self, cx: &mut Context<Self>) {
        let open = self.sidebar_ctrl.is_open();
        self.status_bar.update(cx, |sb, cx| {
            sb.set_sidebar_open(open, cx);
        });
        self.title_bar.update(cx, |tb, cx| {
            tb.set_sidebar_open(open, cx);
        });
    }

    /// Toggle auto-hide mode
    pub(super) fn toggle_sidebar_auto_hide(&mut self, cx: &mut Context<Self>) {
        self.sidebar_ctrl.toggle_auto_hide();
        let open = self.sidebar_ctrl.is_open();
        let window_id = self.window_id;
        let auto_hide = self.sidebar_ctrl.is_auto_hide();
        self.workspace
            .update(cx, |ws, cx| ws.set_sidebar_open(window_id, open, cx));
        settings_entity(cx).update(cx, |s, cx| {
            s.set_sidebar_auto_hide(auto_hide, cx);
            s.set_sidebar_open(open, cx);
        });
        self.sync_status_bar_sidebar_state(cx);
        cx.notify();
    }

    /// Show sidebar temporarily in auto-hide mode
    pub(super) fn show_sidebar_on_hover(&mut self, cx: &mut Context<Self>) {
        if self.sidebar_ctrl.show_on_hover() {
            cx.notify();
        }
    }

    /// Hide sidebar when mouse leaves in auto-hide mode
    pub(super) fn hide_sidebar_on_leave(&mut self, cx: &mut Context<Self>) {
        if self.sidebar_ctrl.hide_on_leave() {
            cx.notify();
        }
    }
}
