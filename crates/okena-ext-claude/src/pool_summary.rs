//! Status-bar summary of one host's account pool: runway and daily budget
//! (see [`crate::pool_model`]), with a popover that shows the model's inputs and
//! each account's share.

use crate::pool_model::{self, DailyBudget, Pace, PlanSize, RUNWAY_HORIZON_MS, Runway};
use crate::pool_usage::{AccountKey, PoolHistory};
use crate::settings::PoolDisplay;
use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::{h_flex, v_flex};
use okena_core::ai_proxy::AiAccount;
use okena_extensions::ThemeColors;
use okena_transport::LOCAL_DAEMON_CONNECTION_ID;
use okena_ui::tokens::{ui_text_ms, ui_text_sm, ui_text_xs};
use okena_usage::history::History;
use okena_usage::{
    Severity, WorkingDays, format_reset_time_epoch, severity_color, usage_body_container,
    usage_divider, usage_kv_row, usage_trigger_item,
};
use okena_workspace::remote_sync::HostAiAccounts;

const MINUTE_MS: i64 = 60 * 1000;
const HOUR_MS: i64 = 60 * MINUTE_MS;

pub(crate) struct AccountShare {
    label: String,
    tier: Option<&'static str>,
    five_hour: Option<f64>,
    seven_day: Option<f64>,
    weight: f64,
    budget: DailyBudget,
    blocked: Option<String>,
}

pub(crate) struct HostSummary {
    /// `None` for the local daemon.
    pub host_name: Option<String>,
    runway: Runway,
    today: DailyBudget,
    pace: Pace,
    mean_weight: f64,
    accounts: Vec<AccountShare>,
}

pub(crate) fn summarize(
    host: &HostAiAccounts,
    history: &PoolHistory,
    working: WorkingDays,
) -> HostSummary {
    let now = jiff::Timestamp::now();
    let now_ms = now.as_millisecond();
    let tz = jiff::tz::TimeZone::system();
    let empty = History::default();
    let accounts: Vec<(&AiAccount, &History)> = host
        .accounts
        .iter()
        .filter(|account| pool_model::in_service(account))
        .map(|account| {
            let key = AccountKey::new(host, account);
            let history = history
                .entries
                .get(&key)
                .map_or(&empty, |entry| &entry.history);
            (account, history)
        })
        .collect();
    let pace = pool_model::pace(&accounts);
    let pool: Vec<&AiAccount> = accounts.iter().map(|(account, _)| *account).collect();
    let mut today = DailyBudget::default();
    let shares = accounts
        .iter()
        .map(|(account, history)| {
            let budget = pool_model::daily_budget(account, history, working, now, &tz);
            today += budget;
            AccountShare {
                label: account.label.clone(),
                tier: tier_label(account),
                five_hour: account.five_hour.as_ref().map(|usage| usage.utilization),
                seven_day: account.seven_day.as_ref().map(|usage| usage.utilization),
                weight: pool_model::plan_weight(account),
                budget,
                blocked: blocked_reason(account, now_ms),
            }
        })
        .collect::<Vec<_>>();
    let mean_weight = if shares.is_empty() {
        1.0
    } else {
        shares.iter().map(|share| share.weight).sum::<f64>() / shares.len() as f64
    };
    HostSummary {
        host_name: (host.connection_id != LOCAL_DAEMON_CONNECTION_ID)
            .then(|| host.host_name.clone()),
        runway: pool_model::runway(&pool, pace, now_ms),
        today,
        pace,
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

fn budget_severity(percent: f64) -> Severity {
    if percent > 100.0 {
        Severity::Critical
    } else if percent > 85.0 {
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
    if display.runway {
        items.push(usage_trigger_item(
            t,
            cx,
            "Runway".into(),
            runway_value(summary.runway, now_ms),
            None,
            severity_color(t, runway_severity(summary.runway)),
        ));
    }
    if display.today {
        let item = match summary.today.percent() {
            Some(percent) => usage_trigger_item(
                t,
                cx,
                "Today".into(),
                format!("{percent:.0}%"),
                Some((percent / 100.0).clamp(0.0, 1.0) as f32),
                severity_color(t, budget_severity(percent)),
            ),
            None => usage_trigger_item(
                t,
                cx,
                "Today".into(),
                "no quota".into(),
                None,
                severity_color(t, Severity::Critical),
            ),
        };
        items.push(item);
    }
    items
}

pub(crate) fn render_body(summary: &HostSummary, t: &ThemeColors, cx: &App) -> Div {
    let runway_text = match summary.runway {
        Runway::Lasts { ms } => format!("{} of nonstop work", short_duration(ms)),
        Runway::BeyondHorizon => "a week or more".into(),
        Runway::Blocked {
            until_ms: Some(until),
        } => format!(
            "all blocked, first back {}",
            format_reset_time_epoch(until as f64 / 1000.0, true)
        ),
        Runway::Blocked { until_ms: None } => "all blocked".into(),
    };
    let today_text = match summary.today.percent() {
        Some(percent) => format!("{percent:.0}% of today's budget"),
        None => "no weekly quota left".into(),
    };
    let session_hours = 100.0 * summary.mean_weight / summary.pace.session_points_per_hour;
    let pace_note = format!(
        "Your pace fills a 5h window in {}; a full 5h window costs {:.0}% of a week. {}",
        short_duration((session_hours * HOUR_MS as f64) as i64),
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
            .child(usage_kv_row(
                t,
                cx,
                "Runway",
                runway_text,
                severity_color(t, runway_severity(summary.runway)),
            ))
            .child(usage_kv_row(
                t,
                cx,
                "Today",
                today_text,
                severity_color(
                    t,
                    summary
                        .today
                        .percent()
                        .map_or(Severity::Critical, budget_severity),
                ),
            ))
            .child(
                div()
                    .text_size(ui_text_xs(cx))
                    .text_color(rgb(t.text_muted))
                    .child(pace_note),
            )
            .child(usage_divider(t))
            .children(
                summary
                    .accounts
                    .iter()
                    .map(|share| render_share(share, t, cx)),
            ),
    )
}

fn render_share(share: &AccountShare, t: &ThemeColors, cx: &App) -> AnyElement {
    let percent = |value: Option<f64>| value.map_or("–".into(), |value| format!("{value:.0}%"));
    // The budget is weighted by plan; the row reads in the account's own percent.
    let today = format!(
        "today {:.0} / {:.0}%",
        share.budget.used / share.weight,
        share.budget.allowance / share.weight
    );
    let cell = |text: String| {
        div()
            .text_size(ui_text_xs(cx))
            .text_color(rgb(t.text_secondary))
            .child(text)
    };
    v_flex()
        .gap(px(1.0))
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
        .child(
            h_flex()
                .gap(px(10.0))
                .child(cell(format!("5h {}", percent(share.five_hour))))
                .child(cell(format!("7d {}", percent(share.seven_day))))
                .child(cell(today)),
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
