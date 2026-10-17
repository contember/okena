//! Desktop mirror of the daemon's agent usage.
//!
//! The daemon polls Claude / Codex usage and pushes [`ApiUsage`] over its
//! WebSocket (`usage_changed`). The app layer (`okena-app`) owns the
//! connection, so it installs a [`GlobalDaemonUsage`] entity, feeds it every
//! snapshot from the local daemon connection, and plugs in a
//! [`UsageRefreshFn`] that calls `POST /v1/usage/refresh`. The usage widgets
//! in the extension crates only read this entity — they never fetch.

use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::*;
use okena_core::api::{ApiProviderUsage, ApiUsage, ApiUsageProvider, ApiUsageWindow};

use crate::history::HistoryPoint;
use crate::{
    SegmentUnit, TriggerItem, UsageRow, WorkingDays, effective_time_pct, format_window_label,
    linear_time_pct, now_secs, segment_unit_for_window,
};

/// Asks the daemon to refetch one provider now. Installed by the app layer.
pub type UsageRefreshFn = Rc<dyn Fn(ApiUsageProvider, &mut App)>;

/// Client-side spacing between refresh requests for one provider. The daemon
/// throttles too (60 s); this only saves the round trip on hover jitter.
const REFRESH_REQUEST_THROTTLE: Duration = Duration::from_secs(60);

/// The latest usage snapshot from the desktop's daemon.
#[derive(Default)]
pub struct DaemonUsage {
    usage: Option<ApiUsage>,
    source_label: Option<SharedString>,
    refresh: Option<UsageRefreshFn>,
    last_refresh_request: HashMap<ApiUsageProvider, Instant>,
}

impl DaemonUsage {
    pub fn usage(&self) -> Option<&ApiUsage> {
        self.usage.as_ref()
    }

    pub fn provider(&self, provider: ApiUsageProvider) -> Option<&ApiProviderUsage> {
        self.usage.as_ref()?.provider(provider)
    }

    /// Name of the machine the numbers come from, set only when the daemon is
    /// not on this machine.
    pub fn source_label(&self) -> Option<&SharedString> {
        self.source_label.as_ref()
    }

    /// Replace the snapshot (`None` when the daemon connection is gone).
    pub fn set_usage(&mut self, usage: Option<ApiUsage>, cx: &mut Context<Self>) {
        if self.usage != usage {
            self.usage = usage;
            cx.notify();
        }
    }

    pub fn set_source_label(&mut self, label: Option<SharedString>, cx: &mut Context<Self>) {
        if self.source_label != label {
            self.source_label = label;
            cx.notify();
        }
    }

    pub fn set_refresh(&mut self, refresh: UsageRefreshFn) {
        self.refresh = Some(refresh);
    }
}

/// Global handle to the shared [`DaemonUsage`] entity.
pub struct GlobalDaemonUsage(pub Entity<DaemonUsage>);

impl Global for GlobalDaemonUsage {}

impl GlobalDaemonUsage {
    /// The shared entity, created empty on first use.
    pub fn entity(cx: &mut App) -> Entity<DaemonUsage> {
        if let Some(global) = cx.try_global::<GlobalDaemonUsage>() {
            return global.0.clone();
        }
        let entity = cx.new(|_| DaemonUsage::default());
        cx.set_global(GlobalDaemonUsage(entity.clone()));
        entity
    }

    /// Ask the daemon to refetch `provider` (popover opened). Throttled per
    /// provider; a no-op until the app layer installed a refresh function.
    pub fn request_refresh(provider: ApiUsageProvider, cx: &mut App) {
        let entity = Self::entity(cx);
        let refresh = entity.update(cx, |store, _| {
            let now = Instant::now();
            let recent = store
                .last_refresh_request
                .get(&provider)
                .is_some_and(|at| now.duration_since(*at) < REFRESH_REQUEST_THROTTLE);
            if recent {
                return None;
            }
            let refresh = store.refresh.clone()?;
            store.last_refresh_request.insert(provider, now);
            Some(refresh)
        });
        if let Some(refresh) = refresh {
            refresh(provider, cx);
        }
    }
}

/// Clone of one provider's entry from the shared snapshot.
pub fn provider_usage(provider: ApiUsageProvider, cx: &mut App) -> Option<ApiProviderUsage> {
    GlobalDaemonUsage::entity(cx)
        .read(cx)
        .provider(provider)
        .cloned()
}

/// Build a popover row for a wire window.
pub fn window_row(
    window: &ApiUsageWindow,
    label: impl Into<SharedString>,
    marker_id: impl Into<SharedString>,
) -> UsageRow {
    let now = now_secs().unwrap_or(0.0);
    UsageRow {
        label: label.into(),
        period: format_window_label(window.window_seconds).into(),
        pct: window.used_percent,
        time_pct: window
            .reset_at
            .and_then(|reset| linear_time_pct(reset, window.window_seconds, now)),
        reset_epoch: window.reset_at,
        period_secs: window.window_seconds,
        unit: segment_unit_for_window(window.window_seconds),
        marker_id: marker_id.into(),
    }
}

/// The window's history samples as chart points.
pub fn window_points(window: &ApiUsageWindow) -> Vec<HistoryPoint> {
    window
        .history
        .iter()
        .map(|point| HistoryPoint {
            recorded_at: point.recorded_at,
            used_percent: point.used_percent,
        })
        .collect()
}

/// A status-bar trigger item whose color matches the popover row.
pub fn window_trigger_item(
    window: &ApiUsageWindow,
    label: impl Into<SharedString>,
    working: WorkingDays,
) -> TriggerItem {
    let now = now_secs().unwrap_or(0.0);
    let unit: Option<SegmentUnit> = segment_unit_for_window(window.window_seconds);
    let linear = window
        .reset_at
        .and_then(|reset| linear_time_pct(reset, window.window_seconds, now));
    let effective = effective_time_pct(
        window.reset_at,
        window.window_seconds,
        unit,
        working,
        linear,
    );
    (label.into(), window.used_percent, effective)
}
