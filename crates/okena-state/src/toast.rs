//! Toast notification data type.
//!
//! Pure data — the GPUI-backed `ToastManager` lives in `okena-workspace`.

use std::time::{Duration, Instant};

/// Default time-to-live for toast notifications
const DEFAULT_TTL: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastLevel {
    #[allow(dead_code)]
    Success,
    Error,
    Warning,
    Info,
}

/// Visual emphasis for a clickable toast action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastActionStyle {
    /// Neutral / secondary action.
    Default,
    /// Emphasized primary action (e.g. "Undo").
    Primary,
    /// Destructive action (e.g. "Close now").
    Danger,
}

/// A clickable button rendered inside a toast.
///
/// `id` is opaque to this crate — the view layer that posted the toast is
/// responsible for interpreting it (e.g. `"soft_close_undo:<project>:<terminal>"`).
/// This keeps `okena-state` free of any GPUI / action-routing knowledge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToastAction {
    pub id: String,
    pub label: String,
    pub style: ToastActionStyle,
}

impl ToastAction {
    pub fn new(id: impl Into<String>, label: impl Into<String>, style: ToastActionStyle) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            style,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Toast {
    pub id: String,
    pub level: ToastLevel,
    pub message: String,
    pub created: Instant,
    pub ttl: Duration,
    /// Optional clickable actions. When non-empty the overlay also renders a
    /// countdown indicator driven by `created` + `ttl`.
    pub actions: Vec<ToastAction>,
}

impl Toast {
    fn new(level: ToastLevel, message: impl Into<String>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            level,
            message: message.into(),
            created: Instant::now(),
            ttl: DEFAULT_TTL,
            actions: Vec::new(),
        }
    }

    #[allow(dead_code)]
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

    /// Attach clickable actions (rendered as buttons, with a countdown).
    pub fn with_actions(mut self, actions: Vec<ToastAction>) -> Self {
        self.actions = actions;
        self
    }

    /// Remaining lifetime as a 0.0..=1.0 fraction of the TTL
    /// (1.0 = just created, 0.0 = expired). Drives the countdown indicator.
    pub fn remaining_fraction(&self) -> f32 {
        let ttl = self.ttl.as_secs_f32();
        if ttl <= 0.0 {
            return 0.0;
        }
        (1.0 - self.created.elapsed().as_secs_f32() / ttl).clamp(0.0, 1.0)
    }

    /// True when the toast has lived past its TTL.
    pub fn is_expired(&self) -> bool {
        self.created.elapsed() >= self.ttl
    }
}

impl PartialEq for Toast {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}
