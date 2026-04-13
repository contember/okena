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
            id: uuid::Uuid::new_v4().to_string(),
            level,
            message: message.into(),
            created: Instant::now(),
            ttl: DEFAULT_TTL,
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

    #[allow(dead_code)]
    pub fn with_ttl(mut self, ttl: Duration) -> Self {
        self.ttl = ttl;
        self
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
