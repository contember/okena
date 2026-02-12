use crate::views::sidebar_controller::{AnimationTarget, SidebarController, FRAME_TIME_MS};
use gpui::*;

use super::RootView;

impl RootView {
    /// Toggle sidebar visibility with animation
    pub(super) fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        let target = self.sidebar_ctrl.toggle(&mut self.app_settings);
        self.sync_status_bar_sidebar_state(cx);
        self.animate_sidebar_to(target, cx);
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
        let target = self.sidebar_ctrl.toggle_auto_hide(&mut self.app_settings);
        self.animate_sidebar_to(target, cx);
        cx.notify();
    }

    /// Show sidebar temporarily in auto-hide mode
    pub(super) fn show_sidebar_on_hover(&mut self, cx: &mut Context<Self>) {
        let target = self.sidebar_ctrl.show_on_hover();
        self.animate_sidebar_to(target, cx);
    }

    /// Hide sidebar when mouse leaves in auto-hide mode
    pub(super) fn hide_sidebar_on_leave(&mut self, cx: &mut Context<Self>) {
        let target = self.sidebar_ctrl.hide_on_leave();
        self.animate_sidebar_to(target, cx);
    }

    /// Animate sidebar to target if needed
    pub(super) fn animate_sidebar_to(&mut self, target: AnimationTarget, cx: &mut Context<Self>) {
        if let Some(target_value) = target.value() {
            self.animate_sidebar(target_value, cx);
        }
    }

    /// Animate sidebar to target value (0.0 = collapsed, 1.0 = expanded)
    pub(super) fn animate_sidebar(&mut self, target: f32, cx: &mut Context<Self>) {
        let current = self.sidebar_ctrl.animation();

        // Skip animation if already at target
        if (current - target).abs() < 0.01 {
            self.sidebar_ctrl.set_animation(target);
            cx.notify();
            return;
        }

        let steps = SidebarController::animation_steps();
        let step_duration = std::time::Duration::from_millis(FRAME_TIME_MS);

        cx.spawn(async move |this: WeakEntity<RootView>, cx| {
            for i in 1..=steps {
                smol::Timer::after(step_duration).await;

                let progress = SidebarController::ease_progress(current, target, i, steps);

                let result = this.update(cx, |this, cx| {
                    this.sidebar_ctrl.set_animation(progress);
                    cx.notify();
                });
                if result.is_err() {
                    break;
                }
            }

            // Ensure we reach the target exactly
            let _ = this.update(cx, |this, cx| {
                this.sidebar_ctrl.set_animation(target);
                cx.notify();
            });
        }).detach();
    }
}
