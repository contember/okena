//! Toast notification data type.
//!
//! Pure data — the GPUI-backed `ToastManager` lives in `okena-workspace`.

use okena_core::api::ApiToast;
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
    /// Optional secondary line rendered smaller + muted under `message`
    /// (e.g. the project / cwd context for a soft-close toast).
    pub detail: Option<String>,
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
            detail: None,
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

    /// Override the auto-generated id with a caller-chosen one. Used by the
    /// optimistic soft-close path, which reserves a stable toast id up front
    /// (before it knows whether a toast will actually be shown) so the same id
    /// can later dismiss the toast on undo / kill-now / grace expiry.
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = id.into();
        self
    }

    /// Attach clickable actions (rendered as buttons, with a countdown).
    pub fn with_actions(mut self, actions: Vec<ToastAction>) -> Self {
        self.actions = actions;
        self
    }

    /// Attach a secondary line rendered smaller + muted under the message.
    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
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

    /// Project onto the serde-serializable wire type for forwarding over the
    /// remote protocol. Drops the local-only `created` (the receiver stamps its
    /// own) and the `actions` (informational toasts only — see [`ApiToast`]);
    /// the `ttl` travels as milliseconds.
    pub fn to_api(&self) -> ApiToast {
        ApiToast {
            id: self.id.clone(),
            level: self.level.as_wire_str().to_string(),
            message: self.message.clone(),
            detail: self.detail.clone(),
            // Saturate rather than wrap on the (practically impossible) overflow
            // of a multi-billion-year TTL; avoids a lossy `as` cast.
            ttl_ms: u64::try_from(self.ttl.as_millis()).unwrap_or(u64::MAX),
        }
    }

    /// Reconstruct a local toast from a wire `ApiToast`. Stamps a fresh
    /// `created` (the wire type carries no timestamp), rebuilds the `ttl` from
    /// `ttl_ms`, and leaves `actions` empty (never carried over the wire). An
    /// unrecognized `level` string falls back to [`ToastLevel::Info`].
    pub fn from_api(api: &ApiToast) -> Self {
        Self {
            id: api.id.clone(),
            level: ToastLevel::from_wire_str(&api.level),
            message: api.message.clone(),
            detail: api.detail.clone(),
            created: Instant::now(),
            ttl: Duration::from_millis(api.ttl_ms),
            actions: Vec::new(),
        }
    }
}

impl ToastLevel {
    /// Stable lowercase wire token for this level.
    fn as_wire_str(self) -> &'static str {
        match self {
            ToastLevel::Success => "success",
            ToastLevel::Error => "error",
            ToastLevel::Warning => "warning",
            ToastLevel::Info => "info",
        }
    }

    /// Parse a wire token back into a level, defaulting unknown tokens to `Info`
    /// so a future server adding a level never breaks an older client.
    fn from_wire_str(s: &str) -> Self {
        match s {
            "success" => ToastLevel::Success,
            "error" => ToastLevel::Error,
            "warning" => ToastLevel::Warning,
            _ => ToastLevel::Info,
        }
    }
}

impl PartialEq for Toast {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_api_maps_all_fields() {
        let toast = Toast::error("boom")
            .with_id("fixed-id")
            .with_detail("context line")
            .with_ttl(Duration::from_millis(7500));
        let api = toast.to_api();
        assert_eq!(api.id, "fixed-id");
        assert_eq!(api.level, "error");
        assert_eq!(api.message, "boom");
        assert_eq!(api.detail.as_deref(), Some("context line"));
        assert_eq!(api.ttl_ms, 7500);
    }

    #[test]
    fn all_levels_round_trip_through_wire() {
        for level in [
            ToastLevel::Success,
            ToastLevel::Error,
            ToastLevel::Warning,
            ToastLevel::Info,
        ] {
            assert_eq!(ToastLevel::from_wire_str(level.as_wire_str()), level);
        }
    }

    #[test]
    fn unknown_wire_level_falls_back_to_info() {
        assert_eq!(ToastLevel::from_wire_str("nope"), ToastLevel::Info);
    }

    /// `to_api` → `from_api` preserves the serializable fields (id / level /
    /// message / detail / ttl). `created` is freshly stamped and `actions` are
    /// dropped by design, so they are intentionally not part of the round-trip.
    #[test]
    fn api_round_trip_preserves_serializable_fields() {
        let original = Toast::warning("careful")
            .with_id("rt-1")
            .with_detail("more")
            .with_ttl(Duration::from_millis(3210));
        let restored = Toast::from_api(&original.to_api());
        assert_eq!(restored.id, original.id);
        assert_eq!(restored.level, original.level);
        assert_eq!(restored.message, original.message);
        assert_eq!(restored.detail, original.detail);
        assert_eq!(restored.ttl, original.ttl);
        assert!(restored.actions.is_empty());
    }

    #[test]
    fn from_api_with_no_detail() {
        let api = ApiToast {
            id: "x".into(),
            level: "info".into(),
            message: "hi".into(),
            detail: None,
            ttl_ms: 1000,
        };
        let toast = Toast::from_api(&api);
        assert_eq!(toast.level, ToastLevel::Info);
        assert!(toast.detail.is_none());
        assert_eq!(toast.ttl, Duration::from_millis(1000));
    }
}
