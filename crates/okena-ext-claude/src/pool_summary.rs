//! Status-bar summary of one host's account pool (see [`crate::pool_model`]):
//! "Today", the soft limit that keeps quota for the rest of the week, and
//! "Nonstop", the hard limit when every account is blocked. The popover
//! explains both and opens each account's detail.

use crate::pool_model::{self, Pace, PlanSize, RUNWAY_HORIZON_MS, Runway, Today};
use crate::pool_usage::{AccountKey, PoolHistory};
use crate::settings::PoolDisplay;
use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::{h_flex, v_flex};
use okena_core::ai_proxy::{AiAccount, AiUsage};
use okena_extensions::ThemeColors;
use okena_transport::LOCAL_DAEMON_CONNECTION_ID;
use okena_ui::tokens::{ui_text_ms, ui_text_sm, ui_text_xs};
use okena_usage::history::History;
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

pub(crate) struct AccountShare {
    key: AccountKey,
    label: String,
    tier: Option<&'static str>,
    five_hour: Option<AiUsage>,
    seven_day: Option<AiUsage>,
    weekly_reset_ms: Option<i64>,
    /// Weekly percent of this account used since midnight.
    used_today: f64,
    blocked: Option<String>,
}

pub(crate) struct HostSummary {
    /// `None` for the local daemon.
    pub host_name: Option<String>,
    runway: Runway,
    today: Today,
    pace: Pace,
    working: WorkingDays,
    mean_weight: f64,
    accounts: Vec<AccountShare>,
}

impl HostSummary {
    /// Weighted weekly points as work at the pool's pace.
    fn work_ms(&self, points: f64) -> i64 {
        let per_hour = self.pace.session_points_per_hour * self.pace.weekly_per_session_point;
        if per_hour > 0.0 {
            (points / per_hour * HOUR_MS as f64) as i64
        } else {
            0
        }
    }

    /// Weighted weekly points as percent of one average account's week.
    fn account_weeks(&self, points: f64) -> f64 {
        points / self.mean_weight
    }
}

pub(crate) fn summarize(
    host: &HostAiAccounts,
    history: &PoolHistory,
    working: WorkingDays,
) -> HostSummary {
    let now = jiff::Timestamp::now();
    let now_ms = now.as_millisecond();
    let tz = jiff::tz::TimeZone::system();
    let midnight = now
        .to_zoned(tz.clone())
        .date()
        .to_zoned(tz.clone())
        .map_or(now_ms, |midnight| midnight.timestamp().as_millisecond());
    let empty = History::default();
    let accounts: Vec<(&AiAccount, &History, AccountKey)> = host
        .accounts
        .iter()
        .filter(|account| pool_model::in_service(account))
        .map(|account| {
            let key = AccountKey::new(host, account);
            let history = history
                .entries
                .get(&key)
                .map_or(&empty, |entry| &entry.history);
            (account, history, key)
        })
        .collect();
    let pairs: Vec<(&AiAccount, &History)> = accounts
        .iter()
        .map(|(account, history, _)| (*account, *history))
        .collect();
    let pace = pool_model::pace(&pairs);
    let pool: Vec<&AiAccount> = pairs.iter().map(|(account, _)| *account).collect();
    let shares: Vec<AccountShare> = accounts
        .iter()
        .map(|(account, history, key)| AccountShare {
            key: key.clone(),
            label: account.label.clone(),
            tier: tier_label(account),
            five_hour: account.five_hour.clone(),
            seven_day: account.seven_day.clone(),
            weekly_reset_ms: account
                .seven_day
                .as_ref()
                .and_then(|usage| usage.resets_at_ms),
            used_today: pool_model::weekly_used_between(
                account,
                history,
                midnight as f64 / 1000.0,
                now_ms as f64 / 1000.0,
                now,
            ),
            blocked: blocked_reason(account, now_ms),
        })
        .collect();
    let mean_weight = if pool.is_empty() {
        1.0
    } else {
        pool.iter()
            .map(|account| pool_model::plan_weight(account))
            .sum::<f64>()
            / pool.len() as f64
    };
    HostSummary {
        host_name: (host.connection_id != LOCAL_DAEMON_CONNECTION_ID)
            .then(|| host.host_name.clone()),
        runway: pool_model::runway(&pool, pace, now_ms),
        today: pool_model::today(&pairs, working, now, &tz),
        pace,
        working,
        mean_weight,
        accounts: shares,
    }
}

fn tier_label(account: &AiAccount) -> Option<&'static str> {
    match PlanSize::of(account) {
        PlanSize::Max20x => Some("Max 20x"),
        PlanSize::Max5x => Some("Max 5x"),
        PlanSize::Base => None,
    }
}

fn blocked_reason(account: &AiAccount, now_ms: i64) -> Option<String> {
    if let Some(period) = account.exhausted_period(now_ms) {
        return Some(format!("{period} limit"));
    }
    account
        .cooldown_until_ms
        .filter(|until| *until > now_ms)
        .map(|_| "429 cooldown".into())
}

fn runway_severity(runway: Runway) -> Severity {
    match runway {
        Runway::Blocked { .. } => Severity::Critical,
        Runway::Lasts { ms } if ms < HOUR_MS => Severity::Critical,
        Runway::Lasts { ms } if ms < 3 * HOUR_MS => Severity::Warning,
        _ => Severity::Normal,
    }
}

fn today_severity(summary: &HostSummary) -> Severity {
    let left = summary.work_ms(summary.today.left());
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

fn runway_value(runway: Runway, now_ms: i64) -> String {
    match runway {
        Runway::Lasts { ms } => short_duration(ms),
        Runway::BeyondHorizon => format!("{}+", short_duration(RUNWAY_HORIZON_MS)),
        Runway::Blocked {
            until_ms: Some(until),
        } => format!("out, {}", short_duration(until - now_ms)),
        Runway::Blocked { until_ms: None } => "out".into(),
    }
}

/// "3h left" or "45m over".
fn today_value(summary: &HostSummary) -> String {
    let left = summary.work_ms(summary.today.left());
    if left >= 0 {
        format!("{} left", short_duration(left))
    } else {
        format!("{} over", short_duration(-left))
    }
}

fn moment(ms: i64) -> String {
    format_reset_time_epoch(ms as f64 / 1000.0, true)
}

pub(crate) fn render_trigger(
    summary: &HostSummary,
    display: PoolDisplay,
    t: &ThemeColors,
    cx: &App,
) -> Vec<AnyElement> {
    let now_ms = jiff::Timestamp::now().as_millisecond();
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
        let today = summary.today;
        let fraction = if today.safe_per_day > 0.0 {
            (today.used / today.safe_per_day).clamp(0.0, 1.0) as f32
        } else {
            1.0
        };
        items.push(usage_trigger_item(
            t,
            cx,
            "Today".into(),
            today_value(summary),
            Some(fraction),
            severity_color(t, today_severity(summary)),
        ));
    }
    if display.runway {
        items.push(usage_trigger_item(
            t,
            cx,
            "Nonstop".into(),
            runway_value(summary.runway, now_ms),
            None,
            severity_color(t, runway_severity(summary.runway)),
        ));
    }
    items
}

pub(crate) fn render_body(
    summary: &HostSummary,
    open_account: OpenAccount,
    t: &ThemeColors,
    cx: &App,
) -> Div {
    let today = summary.today;
    let now_ms = jiff::Timestamp::now().as_millisecond();
    let work = |points: f64| {
        format!(
            "{} ({:.0}%)",
            short_duration(summary.work_ms(points)),
            summary.account_weeks(points)
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

    let beyond_safe = match today.bottleneck_ms {
        Some(at) => {
            let refill = summary
                .accounts
                .iter()
                .find(|share| {
                    share
                        .weekly_reset_ms
                        .is_some_and(|reset| (reset - at).abs() < 30 * MINUTE_MS)
                })
                .map_or(String::new(), |share| {
                    format!(", before {} resets", share.label)
                });
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
    let recent_text = match today.wall_at_recent_pace_ms {
        Some(wall) => format!("runs out {}", moment(wall)),
        None => "lasts the week".into(),
    };
    let mut refills: Vec<(i64, &AccountShare)> = summary
        .accounts
        .iter()
        .filter(|share| {
            share
                .seven_day
                .as_ref()
                .is_some_and(|usage| usage.utilization > 0.0)
        })
        .filter_map(|share| Some((share.weekly_reset_ms.filter(|at| *at > now_ms)?, share)))
        .collect();
    refills.sort_by_key(|(at, _)| *at);

    let nonstop_text = match summary.runway {
        Runway::Lasts { ms } => short_duration(ms),
        Runway::BeyondHorizon => "a week or more".into(),
        Runway::Blocked { .. } => "blocked".into(),
    };
    let nonstop_note = match summary.runway {
        Runway::Lasts { .. } | Runway::BeyondHorizon => {
            "Working nonstop from now, every account hits its 5h or weekly limit \
             after this long. Accounts come back as their windows reset."
                .to_string()
        }
        Runway::Blocked {
            until_ms: Some(until),
        } => format!(
            "Every account has hit its 5h or weekly limit; the first is back {}.",
            moment(until)
        ),
        Runway::Blocked { until_ms: None } => {
            "Every account has hit its 5h or weekly limit.".to_string()
        }
    };
    let pace_note = format!(
        "Times are work at your pace: a 5h window fills in {} and costs {:.0}% of a week. \
         Percentages are of one account's week. {}",
        short_duration(
            (100.0 * summary.mean_weight / summary.pace.session_points_per_hour * HOUR_MS as f64)
                as i64
        ),
        summary.pace.weekly_per_session_point * 100.0,
        if summary.pace.from_history {
            "Measured over the last 7 days."
        } else {
            "Defaults until there is more history."
        }
    );

    usage_body_container().child(
        v_flex()
            .gap(px(7.0))
            .child(section("THIS WEEK · SOFT LIMIT"))
            .child(usage_kv_row(
                t,
                cx,
                "Today",
                today_value(summary),
                severity_color(t, today_severity(summary)),
            ))
            .child(muted(beyond_safe))
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
                        if today.wall_at_recent_pace_ms.is_some() {
                            t.warning
                        } else {
                            t.text_secondary
                        },
                    ))
                    .when(!summary.working.is_all() && today.off_day > 0.0, |el| {
                        el.child(muted(format!(
                            "Days off use {} each, as last week.",
                            work(today.off_day)
                        )))
                    }),
            )
            .when(!refills.is_empty(), |el| {
                el.child(
                    v_flex()
                        .gap(px(2.0))
                        .child(
                            div()
                                .text_size(ui_text_xs(cx))
                                .text_color(rgb(t.text_secondary))
                                .child("Next refills"),
                        )
                        .children(refills.into_iter().take(3).map(|(at, share)| {
                            muted(format!(
                                "{}  {}  +{:.0}%",
                                moment(at),
                                share.label,
                                share
                                    .seven_day
                                    .as_ref()
                                    .map_or(0.0, |usage| usage.utilization)
                            ))
                        })),
                )
            })
            .child(usage_divider(t))
            .child(section("RIGHT NOW · HARD LIMIT"))
            .child(usage_kv_row(
                t,
                cx,
                "Nonstop",
                nonstop_text,
                severity_color(t, runway_severity(summary.runway)),
            ))
            .child(muted(nonstop_note))
            .child(muted(pace_note))
            .child(usage_divider(t))
            .children(
                summary
                    .accounts
                    .iter()
                    .map(|share| render_share(share, summary.working, open_account.clone(), t, cx)),
            ),
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
                                .child(share.label.clone()),
                        )
                        .when_some(share.tier, |row, tier| {
                            row.child(
                                div()
                                    .text_size(ui_text_xs(cx))
                                    .text_color(rgb(t.text_muted))
                                    .child(tier),
                            )
                        })
                        .when_some(share.blocked.clone(), |row, blocked| {
                            row.child(
                                div()
                                    .text_size(ui_text_xs(cx))
                                    .text_color(rgb(t.warning))
                                    .child(blocked),
                            )
                        }),
                )
                .children(crate::pool_usage::window_bars(
                    share.five_hour.as_ref(),
                    share.seven_day.as_ref(),
                    working,
                    &share.key.element_id(),
                    t,
                    cx,
                ))
                .child(
                    h_flex()
                        .gap(px(10.0))
                        .child(cell(format!("today +{:.0}%", share.used_today)))
                        .when_some(share.weekly_reset_ms, |row, at| {
                            row.child(cell(format!("resets {}", moment(at))))
                        }),
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
    use super::short_duration;

    #[test]
    fn short_duration_keeps_the_status_bar_narrow() {
        assert_eq!(short_duration(0), "1m");
        assert_eq!(short_duration(45 * 60 * 1000), "45m");
        assert_eq!(short_duration((3 * 60 + 10) * 60 * 1000), "3h10m");
        assert_eq!(short_duration(26 * 60 * 60 * 1000), "26h");
        assert_eq!(short_duration(7 * 24 * 60 * 60 * 1000), "7d");
    }
}
