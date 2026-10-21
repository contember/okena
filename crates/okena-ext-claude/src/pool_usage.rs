//! Usage of the AI proxy's account pool. While a host routes new shells through
//! the proxy, these accounts serve `claude`, not the `claude` login itself.
//! Every account comes from the host's published pool (`ApiPool::accounts`);
//! an account's history is read from its host when its detail opens.

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::{h_flex, v_flex};
use okena_core::api::{ApiPoolAccount, ApiPoolAccountHistory, ApiUsageWindow, ApiUsageWindowKind};
use okena_extensions::ThemeColors;
use okena_transport::LOCAL_DAEMON_CONNECTION_ID;
use okena_ui::tokens::{ui_text_ms, ui_text_sm, ui_text_xs};
use okena_usage::{
    GlobalDaemonUsage, HostUsage, SegmentUnit, Segments, TriggerItem, WorkingDays,
    format_reset_time_epoch, format_window_label, headline_color, now_secs, render_usage_bar,
    render_usage_row, reset_aligned_segments, usage_body_container, usage_trigger_items,
    window_points, window_row, window_trigger_item,
};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

/// How long a loaded history serves before opening the detail reads it again;
/// the host reloads it at most once a minute too.
const HISTORY_REUSE: Duration = Duration::from_secs(60);

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct AccountKey {
    pub connection_id: String,
    pub account_id: String,
}

impl AccountKey {
    pub fn new(connection_id: &str, account: &ApiPoolAccount) -> Self {
        Self {
            connection_id: connection_id.to_string(),
            account_id: account.id.clone(),
        }
    }

    pub fn element_id(&self) -> String {
        format!(
            "claude-pool:{}:{}{}",
            self.connection_id.len(),
            self.connection_id,
            self.account_id
        )
    }
}

struct GlobalPoolHistory(WeakEntity<PoolHistory>);
impl Global for GlobalPoolHistory {}

#[derive(Default)]
pub(crate) struct AccountHistory {
    /// The account's windows with `history`, as its host served them.
    pub windows: Vec<ApiUsageWindow>,
    pub error: Option<String>,
    fetched_at: Option<Instant>,
    request: Option<Task<()>>,
}

/// Usage history of the pool accounts whose detail was opened, read once per
/// opening (reused for a minute) and shared across windows.
#[derive(Default)]
pub(crate) struct PoolHistory {
    pub entries: BTreeMap<AccountKey, AccountHistory>,
}

impl PoolHistory {
    pub fn shared(cx: &mut App) -> Entity<Self> {
        if let Some(existing) = cx
            .try_global::<GlobalPoolHistory>()
            .and_then(|g| g.0.upgrade())
        {
            return existing;
        }
        let entity = cx.new(|_| Self::default());
        cx.set_global(GlobalPoolHistory(entity.downgrade()));
        entity
    }

    pub fn request(&mut self, key: AccountKey, cx: &mut Context<Self>) {
        let entry = self.entries.entry(key.clone()).or_default();
        if entry.request.is_some()
            || entry
                .fetched_at
                .is_some_and(|at| at.elapsed() < HISTORY_REUSE)
        {
            return;
        }
        let response =
            GlobalDaemonUsage::pool_account_history(&key.connection_id, &key.account_id, cx);
        let request_key = key.clone();
        let task = cx.spawn(async move |this: WeakEntity<Self>, cx| {
            let result = response
                .await
                .and_then(|history| checked_history(history, &key.account_id));
            let _ = this.update(cx, |this, cx| {
                let entry = this.entries.entry(key).or_default();
                entry.request = None;
                entry.fetched_at = Some(Instant::now());
                match result {
                    Ok(windows) => {
                        entry.error = None;
                        entry.windows = windows;
                    }
                    Err(error) => {
                        entry.error = Some(error);
                    }
                }
                cx.notify();
            });
        });
        self.entries.entry(request_key).or_default().request = Some(task);
    }
}

fn checked_history(
    history: ApiPoolAccountHistory,
    account_id: &str,
) -> Result<Vec<ApiUsageWindow>, String> {
    if history.account_id != account_id {
        return Err("Usage history belongs to another account".into());
    }
    Ok(history.windows)
}

/// Status-bar label for a pool window.
fn trigger_label(window: &ApiUsageWindow) -> Option<&'static str> {
    match window.kind {
        ApiUsageWindowKind::FiveHour => Some("5h"),
        ApiUsageWindowKind::Weekly => Some("7d"),
        _ => None,
    }
}

/// The account is blocked by a quota window at 100%. Its red bar already says
/// so, so the status bar adds no label for it.
fn blocked_by_full_window(account: &ApiPoolAccount) -> bool {
    account.blocked_until.is_some_and(|until| {
        account
            .windows
            .iter()
            .any(|window| window.used_percent >= 100.0 && window.reset_at == Some(until))
    })
}

pub(crate) fn render_trigger(
    account: &ApiPoolAccount,
    working: WorkingDays,
    t: &ThemeColors,
    cx: &App,
) -> AnyElement {
    let status = account
        .blocked
        .clone()
        .filter(|_| !blocked_by_full_window(account));
    let items = trigger_items(account, working);
    h_flex()
        .gap(px(6.0))
        .child(
            div()
                .text_size(ui_text_sm(cx))
                .text_color(rgb(t.text_muted))
                .child(account.label.clone()),
        )
        .when_some(status, |el, status| {
            el.child(
                div()
                    .text_size(ui_text_sm(cx))
                    .text_color(rgb(t.warning))
                    .child(status),
            )
        })
        .children(usage_trigger_items(t, cx, &items))
        .when(items.is_empty(), |el| {
            el.child(
                div()
                    .text_size(ui_text_sm(cx))
                    .text_color(rgb(t.text_muted))
                    .child("Usage unknown"),
            )
        })
        .into_any_element()
}

fn trigger_items(account: &ApiPoolAccount, working: WorkingDays) -> Vec<TriggerItem> {
    account
        .windows
        .iter()
        .filter_map(|window| Some(window_trigger_item(window, trigger_label(window)?, working)))
        .collect()
}

/// One labeled bar per known window, with the grid and time marker of the
/// account detail, for the pool popover's account list.
pub(crate) fn window_bars(
    five_hour: Option<&ApiUsageWindow>,
    seven_day: Option<&ApiUsageWindow>,
    working: WorkingDays,
    marker: &str,
    t: &ThemeColors,
    cx: &App,
) -> Vec<AnyElement> {
    [
        ("5h", five_hour, SegmentUnit::Hour),
        ("7d", seven_day, SegmentUnit::Day),
    ]
    .into_iter()
    .filter_map(|(period, window, unit)| {
        let window = window?;
        let seconds = window.window_seconds;
        let segments = window.reset_at.map_or_else(Segments::default, |reset| {
            reset_aligned_segments(reset, seconds, unit, working)
        });
        let time = segments
            .time_pct
            .or_else(|| okena_usage::linear_time_pct(window.reset_at?, seconds, now_secs()?));
        Some(
            h_flex()
                .gap(px(8.0))
                .items_center()
                .child(
                    div()
                        .w(px(18.0))
                        .text_size(ui_text_xs(cx))
                        .text_color(rgb(t.text_muted))
                        .child(period),
                )
                .child(div().flex_1().child(render_usage_bar(
                    t,
                    window.used_percent,
                    time,
                    format!("{marker}-{period}-bar"),
                    &segments,
                )))
                .child(
                    div()
                        .w(px(34.0))
                        .flex()
                        .justify_end()
                        .text_size(ui_text_xs(cx))
                        .text_color(rgb(headline_color(t, window.used_percent, time)))
                        .child(format!("{:.0}%", window.used_percent)),
                )
                .into_any_element(),
        )
    })
    .collect()
}

/// One pool account of `host`: its windows from the published pool, with the
/// chart from `detail`, the history the host served (if loaded).
pub(crate) fn render_body(
    host: &HostUsage,
    account: &ApiPoolAccount,
    detail: &[ApiUsageWindow],
    chart_state: &Entity<okena_usage::HistoryChartState>,
    t: &ThemeColors,
    cx: &App,
) -> Div {
    let working = WorkingDays::from_indices(&host.usage.working_days);
    let marker = AccountKey::new(&host.connection_id, account).element_id();
    let mut title = account.label.clone();
    if host.connection_id != LOCAL_DAEMON_CONNECTION_ID {
        title = format!("{} / {title}", host.host_name);
    }
    let status = account
        .blocked
        .as_ref()
        .map(|blocked| match account.blocked_until {
            Some(until) => format!("{blocked} until {}", format_reset_time_epoch(until, true)),
            None => blocked.clone(),
        });
    let rows = account.windows.iter().map(|window| {
        let period = format_window_label(window.window_seconds);
        let row = window_row(window, window.label.clone(), format!("{marker}-{period}"));
        let points = detail
            .iter()
            .find(|loaded| loaded.id == window.id)
            .map(window_points)
            .unwrap_or_default();
        render_usage_row(t, cx, &row, working, points, chart_state).into_any_element()
    });

    usage_body_container().child(
        v_flex()
            .gap(px(7.0))
            .child(
                h_flex()
                    .items_baseline()
                    .justify_between()
                    .gap(px(8.0))
                    .child(
                        h_flex()
                            .gap(px(6.0))
                            .items_baseline()
                            .child(
                                div()
                                    .text_size(ui_text_ms(cx))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(t.text_primary))
                                    .child(title),
                            )
                            .when_some(account.tier.clone(), |el, tier| {
                                el.child(
                                    div()
                                        .text_size(ui_text_xs(cx))
                                        .text_color(rgb(t.text_muted))
                                        .child(tier),
                                )
                            }),
                    )
                    .when_some(status, |el, status| {
                        el.child(
                            div()
                                .text_size(ui_text_xs(cx))
                                .text_color(rgb(t.text_muted))
                                .child(status),
                        )
                    }),
            )
            .children(rows),
    )
}

#[cfg(test)]
mod tests {
    use super::{AccountKey, PoolHistory, blocked_by_full_window, trigger_items};
    use okena_core::api::{
        ApiPoolAccount, ApiPoolAccountHistory, ApiUsagePoint, ApiUsageWindow, ApiUsageWindowKind,
    };
    use okena_usage::{GlobalDaemonUsage, WorkingDays};
    use std::rc::Rc;

    fn window(id: &str, kind: ApiUsageWindowKind, used_percent: f64) -> ApiUsageWindow {
        ApiUsageWindow {
            id: id.into(),
            label: id.into(),
            kind,
            used_percent,
            window_seconds: 18000.0,
            reset_at: Some(2.0),
            active: true,
            history: Vec::new(),
        }
    }

    fn account() -> ApiPoolAccount {
        ApiPoolAccount {
            id: "a".into(),
            label: "Work".into(),
            tier: None,
            blocked: None,
            blocked_until: None,
            used_today_percent: 0.0,
            windows: Vec::new(),
        }
    }

    #[gpui::test]
    fn history_requests_are_shared_and_isolated_by_host(cx: &mut gpui::TestAppContext) {
        use std::cell::Cell;

        let calls = Rc::new(Cell::new(0));
        let counter = calls.clone();
        let (first, second) = cx.update(|cx| {
            GlobalDaemonUsage::entity(cx).update(cx, |store, _| {
                store.set_pool_history(Rc::new(move |connection, account_id, _| {
                    counter.set(counter.get() + 1);
                    let percent = if connection == "local" { 23.0 } else { 71.0 };
                    let mut weekly = window("five_hour", ApiUsageWindowKind::FiveHour, percent);
                    weekly.history = vec![ApiUsagePoint {
                        recorded_at: 1.0,
                        used_percent: percent,
                    }];
                    gpui::Task::ready(Ok(ApiPoolAccountHistory {
                        account_id: account_id.into(),
                        windows: vec![weekly],
                    }))
                }));
            });
            (PoolHistory::shared(cx), PoolHistory::shared(cx))
        });
        assert_eq!(first, second);
        let local = AccountKey {
            connection_id: "local".into(),
            account_id: "a".into(),
        };
        let remote = AccountKey {
            connection_id: "remote".into(),
            account_id: "a".into(),
        };
        cx.update(|cx| {
            first.update(cx, |history, cx| history.request(local.clone(), cx));
            second.update(cx, |history, cx| history.request(local.clone(), cx));
            second.update(cx, |history, cx| history.request(remote.clone(), cx));
        });
        cx.run_until_parked();
        cx.update(|cx| {
            second.update(cx, |history, cx| history.request(local.clone(), cx));
            let history = first.read(cx);
            for (key, percent) in [(local, 23.0), (remote, 71.0)] {
                let entry = &history.entries[&key];
                assert!(entry.error.is_none());
                assert_eq!(entry.windows[0].history[0].used_percent, percent);
            }
        });
        assert_eq!(calls.get(), 2);
    }

    #[gpui::test]
    fn a_history_of_another_account_is_an_error(cx: &mut gpui::TestAppContext) {
        let history = cx.update(|cx| {
            GlobalDaemonUsage::entity(cx).update(cx, |store, _| {
                store.set_pool_history(Rc::new(|_, _, _| {
                    gpui::Task::ready(Ok(ApiPoolAccountHistory {
                        account_id: "b".into(),
                        windows: Vec::new(),
                    }))
                }));
            });
            PoolHistory::shared(cx)
        });
        let key = AccountKey {
            connection_id: "local".into(),
            account_id: "a".into(),
        };
        cx.update(|cx| history.update(cx, |history, cx| history.request(key.clone(), cx)));
        cx.run_until_parked();
        cx.update(|cx| {
            let entry = &history.read(cx).entries[&key];
            assert!(entry.error.is_some());
            assert!(entry.windows.is_empty());
        });
    }

    #[test]
    fn account_keys_do_not_mix_hosts_or_ambiguous_ids() {
        let first = AccountKey {
            connection_id: "ab".into(),
            account_id: "c".into(),
        };
        let second = AccountKey {
            connection_id: "a".into(),
            account_id: "bc".into(),
        };
        assert_ne!(first, second);
        assert_ne!(first.element_id(), second.element_id());
        let remote = AccountKey {
            connection_id: "remote".into(),
            account_id: first.account_id.clone(),
        };
        assert_ne!(first, remote);
    }

    #[test]
    fn a_full_window_needs_no_status_label_but_other_blocks_do() {
        let limited = ApiPoolAccount {
            blocked: Some("7d limit".into()),
            blocked_until: Some(2.0),
            windows: vec![window("seven_day", ApiUsageWindowKind::Weekly, 100.0)],
            ..account()
        };
        assert!(blocked_by_full_window(&limited));
        let disabled = ApiPoolAccount {
            blocked: Some("Disabled".into()),
            blocked_until: None,
            ..limited.clone()
        };
        assert!(!blocked_by_full_window(&disabled));
        let cooling = ApiPoolAccount {
            blocked: Some("429 cooldown".into()),
            blocked_until: Some(9.0),
            ..limited
        };
        assert!(!blocked_by_full_window(&cooling));
    }

    #[test]
    fn zero_session_usage_does_not_hide_an_exhausted_weekly_quota() {
        let account = ApiPoolAccount {
            windows: vec![
                window("five_hour", ApiUsageWindowKind::FiveHour, 0.0),
                window("seven_day", ApiUsageWindowKind::Weekly, 100.0),
            ],
            ..account()
        };
        let items = trigger_items(&account, WorkingDays::all());
        assert_eq!(
            items
                .iter()
                .map(|(label, percent, _)| (label.as_ref(), *percent))
                .collect::<Vec<_>>(),
            [("5h", 0.0), ("7d", 100.0)]
        );
    }
}
