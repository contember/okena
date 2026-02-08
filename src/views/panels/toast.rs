use crate::theme::theme;
use crate::ui::tokens::{RADIUS_STD, SPACE_MD, SPACE_SM, SPACE_XS, TEXT_MS, ICON_SM};
use gpui::*;
use gpui_component::h_flex;
use parking_lot::Mutex;
use std::sync::Arc;
use std::time::{Duration, Instant};
use uuid::Uuid;

/// Default time-to-live for toast notifications
const DEFAULT_TTL: Duration = Duration::from_secs(5);

/// Maximum number of visible toasts
const MAX_VISIBLE_TOASTS: usize = 5;

/// Tick interval for the overlay's animation/prune loop
const TICK_INTERVAL: Duration = Duration::from_millis(50);

/// Duration of fade-in animation
const FADE_IN_DURATION: Duration = Duration::from_millis(150);

/// Toast width
const TOAST_WIDTH: f32 = 320.0;

/// Accent stripe width
const ACCENT_WIDTH: f32 = 3.0;

// ─── ToastLevel ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastLevel {
    Success,
    Error,
    Warning,
    Info,
}

impl ToastLevel {
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

// ─── Toast ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Toast {
    pub id: String,
    pub level: ToastLevel,
    pub message: String,
    pub created: Instant,
    pub ttl: Duration,
}

impl Toast {
    fn new(level: ToastLevel, message: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            level,
            message: message.into(),
            created: Instant::now(),
            ttl: DEFAULT_TTL,
        }
    }

    pub fn success(message: impl Into<String>) -> Self {
        Self::new(ToastLevel::Success, message)
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self::new(ToastLevel::Error, message)
    }

    pub fn warning(message: impl Into<String>) -> Self {
        Self::new(ToastLevel::Warning, message)
    }

    pub fn info(message: impl Into<String>) -> Self {
        Self::new(ToastLevel::Info, message)
    }

    pub fn with_ttl(mut self, ttl: Duration) -> Self {
        self.ttl = ttl;
        self
    }

    pub fn is_expired(&self) -> bool {
        self.created.elapsed() >= self.ttl
    }

    /// Opacity based on fade-in (0.0 → 1.0 over FADE_IN_DURATION)
    fn opacity(&self) -> f32 {
        let elapsed = self.created.elapsed();
        if elapsed >= FADE_IN_DURATION {
            1.0
        } else {
            elapsed.as_secs_f32() / FADE_IN_DURATION.as_secs_f32()
        }
    }
}

// ─── ToastManager (Global) ─────────────────────────────────────────────────

#[derive(Clone)]
pub struct ToastManager(pub Arc<Mutex<Vec<Toast>>>);

impl Global for ToastManager {}

impl ToastManager {
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(Vec::new())))
    }

    /// Post a toast, capping the queue at MAX_VISIBLE_TOASTS (oldest dropped).
    pub fn post(toast: Toast, cx: &App) {
        if let Some(tm) = cx.try_global::<ToastManager>() {
            let mut queue = tm.0.lock();
            queue.push(toast);
            // Drop oldest if over cap
            while queue.len() > MAX_VISIBLE_TOASTS {
                queue.remove(0);
            }
        }
    }

    pub fn success(message: impl Into<String>, cx: &App) {
        Self::post(Toast::success(message), cx);
    }

    pub fn error(message: impl Into<String>, cx: &App) {
        Self::post(Toast::error(message), cx);
    }

    pub fn warning(message: impl Into<String>, cx: &App) {
        Self::post(Toast::warning(message), cx);
    }

    #[allow(dead_code)]
    pub fn info(message: impl Into<String>, cx: &App) {
        Self::post(Toast::info(message), cx);
    }

    /// Remove a toast by ID.
    pub fn dismiss(id: &str, cx: &App) {
        if let Some(tm) = cx.try_global::<ToastManager>() {
            tm.0.lock().retain(|t| t.id != id);
        }
    }

    /// Return non-expired toasts and prune expired ones from the queue.
    pub fn drain_snapshot(&self) -> Vec<Toast> {
        let mut queue = self.0.lock();
        queue.retain(|t| !t.is_expired());
        queue.clone()
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
                    if let Some(tm) = cx.try_global::<ToastManager>() {
                        let snapshot = tm.drain_snapshot();
                        if snapshot != this.toasts {
                            this.toasts = snapshot;
                            cx.notify();
                        }
                    }
                    // Also re-render during fade-in animations
                    if this.toasts.iter().any(|t| t.opacity() < 1.0) {
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

impl PartialEq for Toast {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
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
                let opacity = toast.opacity();
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
                        h_flex()
                            .flex_1()
                            .items_center()
                            .gap(SPACE_SM)
                            .px(SPACE_MD)
                            .py(SPACE_SM)
                            // Icon
                            .child(
                                div()
                                    .text_color(rgb(accent_color))
                                    .text_size(TEXT_MS)
                                    .flex_shrink_0()
                                    .child(icon_char),
                            )
                            // Message
                            .child(
                                div()
                                    .flex_1()
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
    use super::{Toast, ToastLevel, ToastManager, MAX_VISIBLE_TOASTS};
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
            // Simulate the cap logic from post()
            while q.len() > MAX_VISIBLE_TOASTS {
                q.remove(0);
            }
        }
        let q = tm.0.lock();
        assert_eq!(q.len(), MAX_VISIBLE_TOASTS);
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
