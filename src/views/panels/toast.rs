// Re-export Toast, ToastLevel, and ToastManager from workspace (shared data types)
pub use crate::workspace::toast::{Toast, ToastLevel, ToastManager};

use crate::theme::theme;
use crate::ui::tokens::{RADIUS_STD, SPACE_MD, SPACE_SM, SPACE_XS, TEXT_MS, ICON_SM};
use gpui::*;
use std::time::Duration;

/// Tick interval for the overlay's animation/prune loop
const TICK_INTERVAL: Duration = Duration::from_millis(50);

/// Duration of fade-in animation
const FADE_IN_DURATION: Duration = Duration::from_millis(150);

/// Toast width
const TOAST_WIDTH: f32 = 320.0;

/// Accent stripe width
const ACCENT_WIDTH: f32 = 3.0;

trait ToastLevelExt {
    fn icon_char(self) -> &'static str;
    fn accent_color(self, t: &crate::theme::ThemeColors) -> u32;
}

impl ToastLevelExt for ToastLevel {
    fn icon_char(self) -> &'static str {
        match self {
            ToastLevel::Success => "✓",
            ToastLevel::Error => "✗",
            ToastLevel::Warning => "⚠",
            ToastLevel::Info => "ℹ",
        }
    }

    fn accent_color(self, t: &crate::theme::ThemeColors) -> u32 {
        match self {
            ToastLevel::Success => t.success,
            ToastLevel::Error => t.error,
            ToastLevel::Warning => t.warning,
            ToastLevel::Info => t.term_blue,
        }
    }
}

/// Opacity based on fade-in (0.0 → 1.0 over FADE_IN_DURATION)
fn toast_opacity(toast: &Toast) -> f32 {
    let elapsed = toast.created.elapsed();
    if elapsed >= FADE_IN_DURATION {
        1.0
    } else {
        elapsed.as_secs_f32() / FADE_IN_DURATION.as_secs_f32()
    }
}

// ─── ToastOverlay (GPUI entity) ─────────────────────────────────────────────

pub struct ToastOverlay {
    toasts: Vec<Toast>,
}

impl ToastOverlay {
    pub fn new(cx: &mut Context<Self>) -> Self {
        // Start async tick loop for animations and expiry
        cx.spawn(async move |this: WeakEntity<ToastOverlay>, cx| {
            loop {
                smol::Timer::after(TICK_INTERVAL).await;

                let result = this.update(cx, |this, cx| {
                    // Drain pending toasts from HookMonitor into ToastManager
                    if let Some(monitor) = cx.try_global::<crate::workspace::hook_monitor::HookMonitor>() {
                        let hook_toasts = monitor.drain_pending_toasts();
                        ToastManager::post_batch(hook_toasts, cx);
                    }

                    if let Some(tm) = cx.try_global::<ToastManager>() {
                        let snapshot = tm.drain_snapshot();
                        if snapshot != this.toasts {
                            this.toasts = snapshot;
                            cx.notify();
                        }
                    }
                    // Also re-render during fade-in animations
                    if this.toasts.iter().any(|t| toast_opacity(t) < 1.0) {
                        cx.notify();
                    }
                });

                if result.is_err() {
                    break;
                }
            }
        })
        .detach();

        Self { toasts: Vec::new() }
    }
}


impl Render for ToastOverlay {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.toasts.is_empty() {
            return div().into_any_element();
        }

        let t = theme(cx);

        div()
            .absolute()
            .bottom(px(32.0)) // above status bar
            .right(px(12.0))
            .w(px(TOAST_WIDTH))
            .flex()
            .flex_col()
            .gap(SPACE_XS)
            .children(self.toasts.iter().map(|toast| {
                let accent_color = toast.level.accent_color(&t);
                let icon_char = toast.level.icon_char();
                let opacity = toast_opacity(toast);
                let toast_id = toast.id.clone();

                div()
                    .id(SharedString::from(format!("toast-{}", toast.id)))
                    .opacity(opacity)
                    .bg(rgb(t.bg_secondary))
                    .border_1()
                    .border_color(rgb(t.border))
                    .rounded(RADIUS_STD)
                    .shadow_xl()
                    .flex()
                    .overflow_hidden()
                    // Accent stripe
                    .child(
                        div()
                            .w(px(ACCENT_WIDTH))
                            .h_full()
                            .bg(rgb(accent_color))
                            .flex_shrink_0(),
                    )
                    // Content
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_start()
                            .flex_1()
                            .overflow_x_hidden()
                            .gap(SPACE_SM)
                            .px(SPACE_MD)
                            .py(SPACE_SM)
                            // Icon
                            .child(
                                div()
                                    .text_color(rgb(accent_color))
                                    .text_size(TEXT_MS)
                                    .flex_shrink_0()
                                    .mt(px(1.0))
                                    .child(icon_char),
                            )
                            // Message
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .overflow_x_hidden()
                                    .whitespace_normal()
                                    .text_size(TEXT_MS)
                                    .text_color(rgb(t.text_primary))
                                    .child(toast.message.clone()),
                            )
                            // Close button
                            .child(
                                div()
                                    .id(SharedString::from(format!("toast-close-{}", toast.id)))
                                    .cursor_pointer()
                                    .flex_shrink_0()
                                    .rounded(RADIUS_STD)
                                    .p(px(2.0))
                                    .hover(|s| s.bg(rgb(t.bg_hover)))
                                    .child(
                                        svg()
                                            .path("icons/close.svg")
                                            .size(ICON_SM)
                                            .text_color(rgb(t.text_muted)),
                                    )
                                    .on_click(move |_, _window, cx| {
                                        ToastManager::dismiss(&toast_id, cx);
                                    }),
                            ),
                    )
            }))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{Toast, ToastLevel, ToastManager};
    use std::thread;
    use std::time::Duration;

    #[test]
    fn test_toast_expiry() {
        let toast = Toast::error("fail").with_ttl(Duration::from_millis(50));
        assert!(!toast.is_expired());
        thread::sleep(Duration::from_millis(60));
        assert!(toast.is_expired());
    }

    #[test]
    fn test_drain_snapshot_prunes_expired() {
        let tm = ToastManager::new();
        {
            let mut q = tm.0.lock();
            q.push(Toast::success("a"));
            q.push(Toast::error("b").with_ttl(Duration::from_millis(1)));
            q.push(Toast::warning("c"));
        }
        // Wait for the short-TTL toast to expire
        thread::sleep(Duration::from_millis(10));
        let snapshot = tm.drain_snapshot();
        assert_eq!(snapshot.len(), 2);
        assert_eq!(snapshot[0].message, "a");
        assert_eq!(snapshot[1].message, "c");
    }

    #[test]
    fn test_queue_cap() {
        let tm = ToastManager::new();
        {
            let mut q = tm.0.lock();
            for i in 0..7 {
                q.push(Toast::info(format!("msg-{}", i)));
            }
            while q.len() > 5 {
                q.remove(0);
            }
        }
        let q = tm.0.lock();
        assert_eq!(q.len(), 5);
        // Oldest (0, 1) should be dropped, first remaining is msg-2
        assert_eq!(q[0].message, "msg-2");
    }

    #[test]
    fn test_dismiss_by_id() {
        let tm = ToastManager::new();
        let ids: Vec<String>;
        {
            let mut q = tm.0.lock();
            q.push(Toast::success("a"));
            q.push(Toast::error("b"));
            q.push(Toast::warning("c"));
            ids = q.iter().map(|t| t.id.clone()).collect();
        }
        // Dismiss the middle toast
        tm.0.lock().retain(|t| t.id != ids[1]);
        let q = tm.0.lock();
        assert_eq!(q.len(), 2);
        assert_eq!(q[0].id, ids[0]);
        assert_eq!(q[1].id, ids[2]);
    }

    #[test]
    fn test_with_ttl_builder() {
        let toast = Toast::error("x").with_ttl(Duration::from_secs(30));
        assert_eq!(toast.ttl, Duration::from_secs(30));
        assert_eq!(toast.level, ToastLevel::Error);
    }
}
