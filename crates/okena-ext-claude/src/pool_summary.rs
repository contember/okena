//! Status-bar summary of one host's account pool, as the host's daemon
//! publishes it in its usage ([`ApiPool`]): "Today", the soft limit that keeps
//! quota for the rest of the week, and "Nonstop", the hard limit when every
//! account is blocked. The popover explains both and opens each account's
//! detail.

use crate::pool_usage::AccountKey;
use crate::settings::PoolDisplay;
use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::tooltip::Tooltip;
use gpui_component::{h_flex, v_flex};
use okena_core::api::{
    ApiPool, ApiPoolAccount, ApiPoolAmount, ApiPoolNonstop, ApiUsage, ApiUsageProvider,
    ApiUsageWindow, ApiUsageWindowKind,
};
use okena_extensions::ThemeColors;
use okena_transport::LOCAL_DAEMON_CONNECTION_ID;
use okena_ui::tokens::{ui_text_ms, ui_text_sm, ui_text_xs};
use okena_usage::{
    Severity, WorkingDays, format_reset_time_epoch, severity_color, usage_body_container,
    usage_divider, usage_kv_row, usage_trigger_item,
};
use okena_workspace::remote_sync::HostAiAccounts;
use std::rc::Rc;

const MINUTE_MS: i64 = 60 * 1000;
const HOUR_MS: i64 = 60 * MINUTE_MS;

pub(crate) type OpenAccount = Rc<dyn Fn(&AccountKey, &mut Window, &mut App)>;
pub(crate) type GoBack = Rc<dyn Fn(&mut Window, &mut App)>;

struct AccountShare<'a> {
    key: AccountKey,
    account: &'a ApiPoolAccount,
    five_hour: Option<&'a ApiUsageWindow>,
    seven_day: Option<&'a ApiUsageWindow>,
}

pub(crate) struct HostSummary<'a> {
    /// `None` for the local daemon.
    pub host_name: Option<String>,
    pool: &'a ApiPool,
    /// The host's working days are every day, so it has no days off to show.
    works_every_day: bool,
    accounts: Vec<AccountShare<'a>>,
}

/// The pool of `host` from its usage snapshot, or `None` when the host does
/// not publish one.
pub(crate) fn summarize<'a>(host: &HostAiAccounts, usage: &'a ApiUsage) -> Option<HostSummary<'a>> {
    let pool = usage.provider(ApiUsageProvider::Claude)?.pool.as_ref()?;
    let window = |account: &'a ApiPoolAccount, kind: ApiUsageWindowKind| {
        account.windows.iter().find(|window| window.kind == kind)
    };
    let accounts = pool
        .accounts
        .iter()
        .map(|account| AccountShare {
            key: AccountKey {
                connection_id: host.connection_id.clone(),
                account_id: account.id.clone(),
            },
            account,
            five_hour: window(account, ApiUsageWindowKind::FiveHour),
            seven_day: window(account, ApiUsageWindowKind::Weekly),
        })
        .collect();
    Some(HostSummary {
        host_name: (host.connection_id != LOCAL_DAEMON_CONNECTION_ID)
            .then(|| host.host_name.clone()),
        pool,
        works_every_day: WorkingDays::from_indices(&usage.working_days).is_all(),
        accounts,
    })
}

fn seconds_to_ms(seconds: f64) -> i64 {
    (seconds * 1000.0) as i64
}

fn work_ms(amount: ApiPoolAmount) -> i64 {
    seconds_to_ms(amount.work_seconds)
}

/// How the hard limit reads.
struct NonstopView {
    /// Status-bar value: "3h10m", "7d+", "out, 2h".
    trigger: String,
    /// Popover value: "3h10m", "a week or more", "blocked".
    value: String,
    note: String,
    severity: Severity,
}

/// `None` for a state added by a newer daemon, which this client cannot word.
fn nonstop_view(nonstop: &ApiPoolNonstop, now_ms: i64) -> Option<NonstopView> {
    let lasting_note = "Working nonstop from now, every account hits its 5h or weekly limit \
                        after this long. Accounts come back as their windows reset.";
    let view = match *nonstop {
        ApiPoolNonstop::Lasts { work_seconds } => {
            let ms = seconds_to_ms(work_seconds);
            let severity = if ms < HOUR_MS {
                Severity::Critical
            } else if ms < 3 * HOUR_MS {
                Severity::Warning
            } else {
                Severity::Normal
            };
            NonstopView {
                trigger: short_duration(ms),
                value: short_duration(ms),
                note: lasting_note.into(),
                severity,
            }
        }
        ApiPoolNonstop::BeyondHorizon { horizon_seconds } => NonstopView {
            trigger: format!("{}+", short_duration(seconds_to_ms(horizon_seconds))),
            value: "a week or more".into(),
            note: lasting_note.into(),
            severity: Severity::Normal,
        },
        ApiPoolNonstop::Blocked { until: Some(until) } => NonstopView {
            trigger: format!("out, {}", short_duration(seconds_to_ms(until) - now_ms)),
            value: "blocked".into(),
            note: format!(
                "Every account has hit its 5h or weekly limit; the first is back {}.",
                moment(until)
            ),
            severity: Severity::Critical,
        },
        ApiPoolNonstop::Blocked { until: None } => NonstopView {
            trigger: "out".into(),
            value: "blocked".into(),
            note: "Every account has hit its 5h or weekly limit.".into(),
            severity: Severity::Critical,
        },
        ApiPoolNonstop::Unknown => return None,
    };
    Some(view)
}

fn today_severity(summary: &HostSummary) -> Severity {
    let left = work_ms(summary.pool.today.left);
    if left < 0 {
        Severity::Critical
    } else if left < HOUR_MS {
        Severity::Warning
    } else {
        Severity::Normal
    }
}

/// "45m", "3h10m", "26h", "3d".
fn short_duration(ms: i64) -> String {
    let minutes = (ms / MINUTE_MS).max(1);
    if minutes < 60 {
        format!("{minutes}m")
    } else if minutes < 10 * 60 {
        format!("{}h{:02}m", minutes / 60, minutes % 60)
    } else if minutes < 48 * 60 {
        format!("{}h", minutes / 60)
    } else {
        format!("{}d", minutes / (24 * 60))
    }
}

/// "3h left" or "45m over".
fn today_value(summary: &HostSummary) -> String {
    let left = work_ms(summary.pool.today.left);
    if left >= 0 {
        format!("{} left", short_duration(left))
    } else {
        format!("{} over", short_duration(-left))
    }
}

/// Share of today's safe use already used, for the trigger's bar.
fn today_fraction(summary: &HostSummary) -> f32 {
    let today = &summary.pool.today;
    if today.safe_per_day.account_week_percent > 0.0 {
        (today.used.account_week_percent / today.safe_per_day.account_week_percent).clamp(0.0, 1.0)
            as f32
    } else {
        1.0
    }
}

fn moment(epoch: f64) -> String {
    format_reset_time_epoch(epoch, true)
}

/// The Nonstop value of the status bar, when shown and in a known state.
fn trigger_nonstop(
    summary: &HostSummary,
    display: PoolDisplay,
    now_ms: i64,
) -> Option<NonstopView> {
    display
        .runway
        .then(|| nonstop_view(&summary.pool.nonstop, now_ms))
        .flatten()
}

/// The status-bar items, or `None` when no value is shown, so no empty
/// trigger appears.
pub(crate) fn render_trigger(
    summary: &HostSummary,
    display: PoolDisplay,
    t: &ThemeColors,
    cx: &App,
) -> Option<Vec<AnyElement>> {
    let nonstop = trigger_nonstop(summary, display, jiff::Timestamp::now().as_millisecond());
    if !display.today && nonstop.is_none() {
        return None;
    }
    let mut items = Vec::new();
    if let Some(host_name) = &summary.host_name {
        items.push(
            div()
                .text_size(ui_text_sm(cx))
                .text_color(rgb(t.text_muted))
                .child(host_name.clone())
                .into_any_element(),
        );
    }
    if display.today {
        items.push(usage_trigger_item(
            t,
            cx,
            "Today".into(),
            today_value(summary),
            Some(today_fraction(summary)),
            severity_color(t, today_severity(summary)),
        ));
    }
    if let Some(nonstop) = nonstop {
        items.push(usage_trigger_item(
            t,
            cx,
            "Nonstop".into(),
            nonstop.trigger,
            None,
            severity_color(t, nonstop.severity),
        ));
    }
    Some(items)
}

/// `working` is this desktop's working days, for the account bars' grid.
pub(crate) fn render_body(
    summary: &HostSummary,
    working: WorkingDays,
    open_account: OpenAccount,
    t: &ThemeColors,
    cx: &App,
) -> Div {
    let now_ms = jiff::Timestamp::now().as_millisecond();
    let today = &summary.pool.today;
    let pace = summary.pool.pace;
    let work = |amount: ApiPoolAmount| {
        format!(
            "{} ({:.0}%)",
            short_duration(work_ms(amount)),
            amount.account_week_percent
        )
    };
    let muted = |text: String| {
        div()
            .text_size(ui_text_xs(cx))
            .text_color(rgb(t.text_muted))
            .child(text)
    };
    let section = |title: &'static str| {
        div()
            .text_size(ui_text_xs(cx))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(rgb(t.text_muted))
            .child(title)
    };

    let beyond_safe = match today.runs_short_at {
        Some(at) => {
            let refill = today
                .runs_short_before
                .as_ref()
                .map_or(String::new(), |label| format!(", before {label} resets"));
            format!(
                "Work up to this and every working day until the resets still has quota. \
                 More is possible, but the pool then runs short {}{refill}.",
                moment(at)
            )
        }
        None => "Work up to this and every working day until the resets still has quota. \
                 More takes from what the pool refills for later weeks."
            .into(),
    };
    let recent_text = match today.runs_out_at_recent_pace {
        Some(wall) => format!("runs out {}", moment(wall)),
        None => "lasts the week".into(),
    };
    let pace_note = format!(
        "Times are work at your pace: a 5h window fills in {} and costs {:.0}% of a week. \
         Percentages are of one account's week. {}",
        short_duration(seconds_to_ms(pace.session_fill_seconds)),
        pace.session_week_percent,
        if pace.from_history {
            "Measured over the last 7 days."
        } else {
            "Defaults until there is more history."
        }
    );
    let nonstop = nonstop_view(&summary.pool.nonstop, now_ms);

    usage_body_container().child(
        v_flex()
            .gap(px(7.0))
            .child(section("THIS WEEK · SOFT LIMIT"))
            .child(info_kv_row(
                t,
                cx,
                "Today",
                vec![beyond_safe, pace_note],
                today_value(summary),
                severity_color(t, today_severity(summary)),
            ))
            .child(
                v_flex()
                    .gap(px(3.0))
                    .child(usage_kv_row(
                        t,
                        cx,
                        "Used today",
                        work(today.used),
                        t.text_secondary,
                    ))
                    .child(usage_kv_row(
                        t,
                        cx,
                        "Safe per working day",
                        work(today.safe_per_day),
                        t.text_secondary,
                    ))
                    .child(usage_kv_row(
                        t,
                        cx,
                        "At the last 24h's pace",
                        recent_text,
                        if today.runs_out_at_recent_pace.is_some() {
                            t.warning
                        } else {
                            t.text_secondary
                        },
                    ))
                    .when(
                        !summary.works_every_day && today.off_day.account_week_percent > 0.0,
                        |el| {
                            el.child(muted(format!(
                                "Days off use {} each, as last week.",
                                work(today.off_day)
                            )))
                        },
                    ),
            )
            .child(usage_divider(t))
            .when_some(nonstop, |el, nonstop| {
                el.child(section("RIGHT NOW · HARD LIMIT"))
                    .child(info_kv_row(
                        t,
                        cx,
                        "Nonstop",
                        vec![nonstop.note],
                        nonstop.value,
                        severity_color(t, nonstop.severity),
                    ))
                    .child(usage_divider(t))
            })
            .children(
                summary
                    .accounts
                    .iter()
                    .map(|share| render_share(share, working, open_account.clone(), t, cx)),
            ),
    )
}

/// A label/value row whose explanation sits in a tooltip behind an (i).
fn info_kv_row(
    t: &ThemeColors,
    cx: &App,
    label: &'static str,
    paragraphs: Vec<String>,
    value: String,
    value_color: u32,
) -> impl IntoElement {
    let paragraphs: Rc<[String]> = paragraphs.into();
    h_flex()
        .items_center()
        .justify_between()
        .child(
            h_flex()
                .gap(px(5.0))
                .items_center()
                .child(
                    div()
                        .text_size(ui_text_ms(cx))
                        .text_color(rgb(t.text_secondary))
                        .child(label),
                )
                .child(
                    div()
                        .id(SharedString::from(format!("claude-pool-info-{label}")))
                        .child(
                            svg()
                                .path("icons/info.svg")
                                .size(px(12.0))
                                .text_color(rgb(t.text_muted)),
                        )
                        .tooltip(move |window, cx| {
                            let paragraphs = paragraphs.clone();
                            Tooltip::element(move |_, _| {
                                v_flex()
                                    .w(px(280.0))
                                    .py(px(4.0))
                                    .gap(px(6.0))
                                    .children(paragraphs.iter().cloned())
                            })
                            .build(window, cx)
                        }),
                ),
        )
        .child(
            div()
                .text_size(ui_text_ms(cx))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(value_color))
                .child(value),
        )
}

fn render_share(
    share: &AccountShare,
    working: WorkingDays,
    open: OpenAccount,
    t: &ThemeColors,
    cx: &App,
) -> AnyElement {
    let cell = |text: String| {
        div()
            .text_size(ui_text_xs(cx))
            .text_color(rgb(t.text_secondary))
            .child(text)
    };
    let key = share.key.clone();
    h_flex()
        .id(SharedString::from(format!(
            "{}-share",
            share.key.element_id()
        )))
        .cursor_pointer()
        .rounded(px(4.0))
        .mx(px(-6.0))
        .px(px(6.0))
        .py(px(3.0))
        .hover(|style| style.bg(rgb(t.bg_hover)))
        .on_click(move |_, window, cx| open(&key, window, cx))
        .child(
            v_flex()
                .flex_1()
                .gap(px(3.0))
                .child(
                    h_flex()
                        .gap(px(6.0))
                        .items_baseline()
                        .child(
                            div()
                                .text_size(ui_text_ms(cx))
                                .text_color(rgb(t.text_primary))
                                .child(share.account.label.clone()),
                        )
                        .when_some(share.account.tier.clone(), |row, tier| {
                            row.child(
                                div()
                                    .text_size(ui_text_xs(cx))
                                    .text_color(rgb(t.text_muted))
                                    .child(tier),
                            )
                        })
                        .when_some(share.account.blocked.clone(), |row, blocked| {
                            row.child(
                                div()
                                    .text_size(ui_text_xs(cx))
                                    .text_color(rgb(t.warning))
                                    .child(blocked),
                            )
                        }),
                )
                .children(crate::pool_usage::window_bars(
                    share.five_hour,
                    share.seven_day,
                    working,
                    &share.key.element_id(),
                    t,
                    cx,
                ))
                .child(
                    h_flex()
                        .gap(px(10.0))
                        .child(cell(format!(
                            "today +{:.0}%",
                            share.account.used_today_percent
                        )))
                        .when_some(
                            share.seven_day.and_then(|window| window.reset_at),
                            |row, at| row.child(cell(format!("resets {}", moment(at)))),
                        ),
                ),
        )
        .child(
            div()
                .text_size(ui_text_sm(cx))
                .text_color(rgb(t.text_muted))
                .child("›"),
        )
        .into_any_element()
}

/// The way back from an account's detail to the pool.
pub(crate) fn render_back(on_back: GoBack, t: &ThemeColors, cx: &App) -> AnyElement {
    div()
        .px(px(12.0))
        .pt(px(8.0))
        .child(
            div()
                .id("claude-pool-back")
                .cursor_pointer()
                .text_size(ui_text_xs(cx))
                .text_color(rgb(t.text_muted))
                .hover(|style| style.text_color(rgb(t.text_primary)))
                .on_click(move |_, window, cx| on_back(window, cx))
                .child("‹ All accounts"),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::{
        HOUR_MS, MINUTE_MS, nonstop_view, short_duration, summarize, today_fraction,
        today_severity, today_value, trigger_nonstop,
    };
    use crate::settings::PoolDisplay;
    use okena_core::api::{
        ApiPool, ApiPoolAccount, ApiPoolAmount, ApiPoolNonstop, ApiPoolPace, ApiPoolToday,
        ApiProviderUsage, ApiUsage, ApiUsageProvider, ApiUsageStatus, ApiUsageWindow,
        ApiUsageWindowKind,
    };
    use okena_transport::LOCAL_DAEMON_CONNECTION_ID;
    use okena_usage::Severity;
    use okena_workspace::remote_sync::HostAiAccounts;

    fn amount(hours: f64, percent: f64) -> ApiPoolAmount {
        ApiPoolAmount {
            work_seconds: hours * 3600.0,
            account_week_percent: percent,
        }
    }

    fn window(id: &str, kind: ApiUsageWindowKind, reset_at: f64) -> ApiUsageWindow {
        ApiUsageWindow {
            id: id.into(),
            label: id.into(),
            kind,
            used_percent: 40.0,
            window_seconds: 18000.0,
            reset_at: Some(reset_at),
            active: true,
            history: Vec::new(),
        }
    }

    fn pool(left_hours: f64) -> ApiPool {
        ApiPool {
            today: ApiPoolToday {
                left: amount(left_hours, left_hours),
                used: amount(1.0, 5.0),
                safe_per_day: amount(4.0, 20.0),
                off_day: amount(0.0, 0.0),
                runs_short_at: None,
                runs_short_before: None,
                runs_out_at_recent_pace: None,
            },
            nonstop: ApiPoolNonstop::Lasts {
                work_seconds: 5.0 * 3600.0,
            },
            pace: ApiPoolPace {
                session_fill_seconds: 3.0 * 3600.0,
                session_week_percent: 12.0,
                from_history: true,
            },
            accounts: vec![ApiPoolAccount {
                id: "a".into(),
                label: "Work".into(),
                tier: Some("Max 5x".into()),
                blocked: None,
                used_today_percent: 3.0,
                windows: vec![
                    window("seven_day", ApiUsageWindowKind::Weekly, 2000.0),
                    window("five_hour", ApiUsageWindowKind::FiveHour, 1000.0),
                ],
            }],
        }
    }

    fn usage(pool: Option<ApiPool>) -> ApiUsage {
        ApiUsage {
            providers: vec![ApiProviderUsage {
                pool,
                ..ApiProviderUsage::empty(ApiUsageProvider::Claude, ApiUsageStatus::Ok)
            }],
            working_days: vec![0, 1, 2, 3, 4],
            generated_at: 0.0,
        }
    }

    fn host(connection_id: &str) -> HostAiAccounts {
        HostAiAccounts {
            connection_id: connection_id.into(),
            host_name: "studio".into(),
            accounts: Vec::new(),
        }
    }

    #[test]
    fn short_duration_keeps_the_status_bar_narrow() {
        assert_eq!(short_duration(0), "1m");
        assert_eq!(short_duration(45 * 60 * 1000), "45m");
        assert_eq!(short_duration((3 * 60 + 10) * 60 * 1000), "3h10m");
        assert_eq!(short_duration(26 * 60 * 60 * 1000), "26h");
        assert_eq!(short_duration(7 * 24 * 60 * 60 * 1000), "7d");
    }

    #[test]
    fn a_host_without_a_published_pool_has_no_summary() {
        let usage = usage(None);
        assert!(summarize(&host(LOCAL_DAEMON_CONNECTION_ID), &usage).is_none());
    }

    #[test]
    fn summary_keys_accounts_by_host_and_finds_their_windows() {
        let usage = usage(Some(pool(2.0)));
        let local = summarize(&host(LOCAL_DAEMON_CONNECTION_ID), &usage).unwrap();
        assert_eq!(local.host_name, None);
        assert!(!local.works_every_day);
        let remote = summarize(&host("remote"), &usage).unwrap();
        assert_eq!(remote.host_name.as_deref(), Some("studio"));
        let share = &remote.accounts[0];
        assert_eq!(share.key.connection_id, "remote");
        assert_eq!(share.key.account_id, "a");
        assert_eq!(
            share.five_hour.map(|window| window.id.as_str()),
            Some("five_hour")
        );
        assert_eq!(
            share.seven_day.map(|window| window.id.as_str()),
            Some("seven_day")
        );
    }

    #[test]
    fn today_reads_left_or_over_with_its_severity() {
        let host = host(LOCAL_DAEMON_CONNECTION_ID);
        let cases = [
            (3.0, "3h00m left", Severity::Normal),
            (0.5, "30m left", Severity::Warning),
            (-0.75, "45m over", Severity::Critical),
        ];
        for (left_hours, text, severity) in cases {
            let usage = usage(Some(pool(left_hours)));
            let summary = summarize(&host, &usage).unwrap();
            assert_eq!(today_value(&summary), text);
            assert_eq!(today_severity(&summary), severity);
            assert_eq!(today_fraction(&summary), 0.25);
        }
    }

    #[test]
    fn nonstop_reads_like_the_status_bar_and_skips_unknown_states() {
        let now_ms = 1_000_000;
        let lasts = |minutes: i64| ApiPoolNonstop::Lasts {
            work_seconds: (minutes * MINUTE_MS) as f64 / 1000.0,
        };
        let view = nonstop_view(&lasts(50), now_ms).unwrap();
        assert_eq!(
            (view.trigger.as_str(), view.severity),
            ("50m", Severity::Critical)
        );
        assert_eq!(
            nonstop_view(&lasts(120), now_ms).unwrap().severity,
            Severity::Warning
        );
        assert_eq!(
            nonstop_view(&lasts(180), now_ms).unwrap().severity,
            Severity::Normal
        );

        let beyond = ApiPoolNonstop::BeyondHorizon {
            horizon_seconds: 7.0 * 86400.0,
        };
        let view = nonstop_view(&beyond, now_ms).unwrap();
        assert_eq!(
            (view.trigger.as_str(), view.value.as_str()),
            ("7d+", "a week or more")
        );

        let until = (now_ms + 2 * HOUR_MS) as f64 / 1000.0;
        let view = nonstop_view(&ApiPoolNonstop::Blocked { until: Some(until) }, now_ms).unwrap();
        assert_eq!(
            (view.trigger.as_str(), view.value.as_str(), view.severity),
            ("out, 2h00m", "blocked", Severity::Critical)
        );
        assert!(nonstop_view(&ApiPoolNonstop::Unknown, now_ms).is_none());
    }

    #[test]
    fn nonstop_edge_states_read_as_at_head() {
        let view = nonstop_view(&ApiPoolNonstop::Blocked { until: None }, 0).unwrap();
        assert_eq!(
            (view.trigger.as_str(), view.value.as_str(), view.severity),
            ("out", "blocked", Severity::Critical)
        );
        let beyond = ApiPoolNonstop::BeyondHorizon {
            horizon_seconds: 7.0 * 86400.0,
        };
        assert_eq!(nonstop_view(&beyond, 0).unwrap().severity, Severity::Normal);
    }

    #[test]
    fn today_fraction_is_full_without_a_safe_amount_and_capped_at_one() {
        let host = host(LOCAL_DAEMON_CONNECTION_ID);
        let mut nothing_safe = pool(0.0);
        nothing_safe.today.safe_per_day = amount(0.0, 0.0);
        let usage_without_safe = usage(Some(nothing_safe));
        let summary = summarize(&host, &usage_without_safe).unwrap();
        assert_eq!(today_fraction(&summary), 1.0);

        let mut over = pool(-2.0);
        over.today.used = amount(6.0, 30.0);
        let usage_over = usage(Some(over));
        let summary = summarize(&host, &usage_over).unwrap();
        assert_eq!(today_fraction(&summary), 1.0);
    }

    #[test]
    fn an_unknown_nonstop_state_leaves_the_trigger_without_it() {
        let mut unknown = pool(2.0);
        unknown.nonstop = ApiPoolNonstop::Unknown;
        let unknown_usage = usage(Some(unknown));
        let summary = summarize(&host(LOCAL_DAEMON_CONNECTION_ID), &unknown_usage).unwrap();
        let nonstop_only = PoolDisplay {
            accounts: false,
            runway: true,
            today: false,
        };
        assert!(trigger_nonstop(&summary, nonstop_only, 0).is_none());

        let known_usage = usage(Some(pool(2.0)));
        let summary = summarize(&host(LOCAL_DAEMON_CONNECTION_ID), &known_usage).unwrap();
        assert!(trigger_nonstop(&summary, nonstop_only, 0).is_some());
        let today_only = PoolDisplay {
            runway: false,
            today: true,
            ..nonstop_only
        };
        assert!(trigger_nonstop(&summary, today_only, 0).is_none());
    }
}
