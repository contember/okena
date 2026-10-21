//! The AI proxy's account pool as the wire's [`okena_core::api::ApiPool`]:
//! [`crate::pool`]'s model applied to the accounts and their usage history.

use crate::history::{History, window_history};
use crate::pace::WorkingDays;
use crate::pool::{self, PlanSize, RUNWAY_HORIZON_MS, Runway};
use jiff::Timestamp;
use jiff::tz::TimeZone;
use okena_core::ai_proxy::{AiAccount, AiUsage};
use okena_core::api::{
    ApiPool, ApiPoolAccount, ApiPoolAccountHistory, ApiPoolAmount, ApiPoolNonstop, ApiPoolPace,
    ApiPoolToday, ApiUsageWindow, ApiUsageWindowKind,
};
use std::collections::HashMap;

const HOUR_SECS: f64 = 3600.0;
const FIVE_HOUR_SECS: f64 = 5.0 * HOUR_SECS;
const SEVEN_DAY_SECS: f64 = 7.0 * 24.0 * HOUR_SECS;
/// A weekly reset this close to the moment the pool runs short names the
/// account that causes it.
const RUNS_SHORT_RESET_TOLERANCE_MS: i64 = 30 * 60 * 1000;

/// Summarize the pool at `now`. `histories` holds each account's usage history
/// by account id; a missing one counts as empty. Every account is listed, but
/// only those in service count. `None` when the pool has no accounts at all.
pub fn summarize(
    accounts: &[AiAccount],
    histories: &HashMap<String, History>,
    working: WorkingDays,
    now: Timestamp,
    tz: &TimeZone,
) -> Option<ApiPool> {
    if accounts.is_empty() {
        return None;
    }
    let now_ms = now.as_millisecond();
    let now_secs = now_ms as f64 / 1000.0;
    let midnight_secs =
        now.to_zoned(tz.clone())
            .date()
            .to_zoned(tz.clone())
            .map_or(now_ms, |midnight| midnight.timestamp().as_millisecond()) as f64
            / 1000.0;
    let empty = History::default();
    let pairs: Vec<(&AiAccount, &History)> = accounts
        .iter()
        .filter(|account| pool::in_service(account))
        .map(|account| (account, histories.get(&account.id).unwrap_or(&empty)))
        .collect();
    let in_service: Vec<&AiAccount> = pairs.iter().map(|(account, _)| *account).collect();
    let pace = pool::pace(&pairs);
    let today = pool::today(&pairs, working, now, tz);
    let mean_weight = if in_service.is_empty() {
        1.0
    } else {
        in_service
            .iter()
            .map(|account| pool::plan_weight(account))
            .sum::<f64>()
            / in_service.len() as f64
    };
    let amount = |points: f64| ApiPoolAmount {
        work_seconds: work_seconds(pace, points),
        account_week_percent: points / mean_weight,
    };
    let runs_short_before = today.bottleneck_ms.and_then(|at| {
        in_service
            .iter()
            .find(|account| {
                weekly_reset_ms(account)
                    .is_some_and(|reset| (reset - at).abs() < RUNS_SHORT_RESET_TOLERANCE_MS)
            })
            .map(|account| account.label.clone())
    });
    Some(ApiPool {
        today: ApiPoolToday {
            left: amount(today.left()),
            used: amount(today.used),
            safe_per_day: amount(today.safe_per_day),
            off_day: amount(today.off_day),
            runs_short_at: today.bottleneck_ms.map(epoch_secs),
            runs_short_before,
            runs_out_at_recent_pace: today.wall_at_recent_pace_ms.map(epoch_secs),
        },
        nonstop: nonstop(pool::runway(&in_service, pace, now_ms)),
        pace: ApiPoolPace {
            session_fill_seconds: 100.0 * mean_weight / pace.session_points_per_hour * HOUR_SECS,
            session_week_percent: pace.weekly_per_session_point * 100.0,
            from_history: pace.from_history,
        },
        accounts: accounts
            .iter()
            .map(|account| {
                let blocked = blocked(account, now_ms);
                let used_today_percent = if pool::in_service(account) {
                    let history = histories.get(&account.id).unwrap_or(&empty);
                    pool::weekly_used_between(account, history, midnight_secs, now_secs, now)
                } else {
                    0.0
                };
                ApiPoolAccount {
                    id: account.id.clone(),
                    label: account.label.clone(),
                    tier: tier_label(account).map(Into::into),
                    blocked: blocked.as_ref().map(|blocked| blocked.reason.clone()),
                    blocked_until: blocked.and_then(|blocked| blocked.until_ms).map(epoch_secs),
                    used_today_percent,
                    windows: account_windows(account),
                }
            })
            .collect(),
    })
}

/// [`summarize`] at the current minute, in the system time zone. Clients show
/// minutes, so unchanged inputs give an identical pool for a whole minute.
pub fn summarize_now(
    accounts: &[AiAccount],
    histories: &HashMap<String, History>,
    working: WorkingDays,
) -> Option<ApiPool> {
    summarize(
        accounts,
        histories,
        working,
        whole_minute(Timestamp::now()),
        &TimeZone::system(),
    )
}

fn whole_minute(now: Timestamp) -> Timestamp {
    Timestamp::from_second(now.as_second().div_euclid(60) * 60).unwrap_or(now)
}

/// Weighted weekly points as work at the pool's pace.
fn work_seconds(pace: pool::Pace, points: f64) -> f64 {
    let per_hour = pace.session_points_per_hour * pace.weekly_per_session_point;
    if per_hour > 0.0 {
        points / per_hour * HOUR_SECS
    } else {
        0.0
    }
}

fn epoch_secs(ms: i64) -> f64 {
    ms as f64 / 1000.0
}

fn weekly_reset_ms(account: &AiAccount) -> Option<i64> {
    account
        .seven_day
        .as_ref()
        .and_then(|usage| usage.resets_at_ms)
}

fn nonstop(runway: Runway) -> ApiPoolNonstop {
    match runway {
        Runway::Lasts { ms } => ApiPoolNonstop::Lasts {
            work_seconds: ms as f64 / 1000.0,
        },
        Runway::BeyondHorizon => ApiPoolNonstop::BeyondHorizon {
            horizon_seconds: RUNWAY_HORIZON_MS as f64 / 1000.0,
        },
        Runway::Blocked { until_ms } => ApiPoolNonstop::Blocked {
            until: until_ms.map(epoch_secs),
        },
    }
}

fn tier_label(account: &AiAccount) -> Option<&'static str> {
    match PlanSize::of(account) {
        PlanSize::Max20x => Some("Max 20x"),
        PlanSize::Max5x => Some("Max 5x"),
        PlanSize::Base => None,
    }
}

struct Blocked {
    reason: String,
    until_ms: Option<i64>,
}

/// Why the proxy skips the account now, if it does.
fn blocked(account: &AiAccount, now_ms: i64) -> Option<Blocked> {
    let untimed = |reason: &str| {
        Some(Blocked {
            reason: reason.into(),
            until_ms: None,
        })
    };
    if account.needs_login {
        return untimed("Needs login");
    }
    if !account.enabled {
        return untimed("Disabled");
    }
    if let Some(period) = account.exhausted_period(now_ms) {
        let window = match period {
            "7d" => account.seven_day.as_ref(),
            _ => account.five_hour.as_ref(),
        };
        return Some(Blocked {
            reason: format!("{period} limit"),
            until_ms: window.and_then(|usage| usage.resets_at_ms),
        });
    }
    account
        .cooldown_until_ms
        .filter(|until| *until > now_ms)
        .map(|until| Blocked {
            reason: "429 cooldown".into(),
            until_ms: Some(until),
        })
}

/// One account's windows with their history, for its detail chart.
pub fn account_history(
    account: &AiAccount,
    history: &History,
    now: Timestamp,
) -> ApiPoolAccountHistory {
    let now_secs = epoch_secs(now.as_millisecond());
    let windows = ACCOUNT_WINDOWS
        .iter()
        .filter_map(|spec| {
            let mut window = spec.window(account)?;
            window.history = window_history(history, spec.series, &window, now_secs);
            Some(window)
        })
        .collect();
    ApiPoolAccountHistory {
        account_id: account.id.clone(),
        windows,
    }
}

/// [`account_history`] of every account, by account id.
pub fn account_histories_now(
    accounts: &[AiAccount],
    histories: &HashMap<String, History>,
) -> HashMap<String, ApiPoolAccountHistory> {
    let now = Timestamp::now();
    let empty = History::default();
    accounts
        .iter()
        .map(|account| {
            let history = histories.get(&account.id).unwrap_or(&empty);
            (account.id.clone(), account_history(account, history, now))
        })
        .collect()
}

struct AccountWindow {
    id: &'static str,
    /// Name of the window's series in the proxy's usage history.
    series: &'static str,
    label: &'static str,
    kind: ApiUsageWindowKind,
    window_seconds: f64,
    usage: fn(&AiAccount) -> Option<&AiUsage>,
}

const ACCOUNT_WINDOWS: [AccountWindow; 2] = [
    AccountWindow {
        id: "five_hour",
        series: "5h",
        label: "Session",
        kind: ApiUsageWindowKind::FiveHour,
        window_seconds: FIVE_HOUR_SECS,
        usage: |account| account.five_hour.as_ref(),
    },
    AccountWindow {
        id: "seven_day",
        series: "7d",
        label: "Weekly",
        kind: ApiUsageWindowKind::Weekly,
        window_seconds: SEVEN_DAY_SECS,
        usage: |account| account.seven_day.as_ref(),
    },
];

impl AccountWindow {
    fn window(&self, account: &AiAccount) -> Option<ApiUsageWindow> {
        let usage = (self.usage)(account)?;
        Some(ApiUsageWindow {
            id: self.id.into(),
            label: self.label.into(),
            kind: self.kind.clone(),
            used_percent: usage.utilization,
            window_seconds: self.window_seconds,
            reset_at: usage.resets_at_ms.map(epoch_secs),
            active: true,
            history: Vec::new(),
        })
    }
}

/// The account's 5 h and 7 d windows, without history.
fn account_windows(account: &AiAccount) -> Vec<ApiUsageWindow> {
    ACCOUNT_WINDOWS
        .iter()
        .filter_map(|spec| spec.window(account))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::{LimitSample, Provider, UsageSample};

    const NOW_MS: i64 = 1_800_000_000_000;
    const HOUR_MS: i64 = 60 * 60 * 1000;

    fn now() -> Timestamp {
        Timestamp::from_millisecond(NOW_MS).unwrap()
    }

    fn account(id: &str) -> AiAccount {
        AiAccount {
            id: id.into(),
            label: id.to_uppercase(),
            email: None,
            plan: Some("max".into()),
            rate_limit_tier: None,
            enabled: true,
            needs_login: false,
            cooldown_until_ms: None,
            five_hour: None,
            seven_day: None,
            requests: 0,
            last_used_ms: None,
        }
    }

    fn usage(utilization: f64, resets_at_ms: i64) -> Option<AiUsage> {
        Some(AiUsage {
            utilization,
            resets_at_ms: Some(resets_at_ms),
        })
    }

    fn sample(id: &str, at_secs: f64, limits: Vec<(&str, f64, f64, f64)>) -> UsageSample {
        UsageSample {
            version: 1,
            recorded_at: at_secs,
            provider: Provider::Claude,
            source: id.into(),
            plan: None,
            limits: limits
                .into_iter()
                .map(|(name, used, window, reset)| LimitSample {
                    name: name.into(),
                    used_percent: used,
                    window_seconds: window,
                    reset_at: Some(reset),
                })
                .collect(),
        }
    }

    fn summarize_at(accounts: &[AiAccount], histories: &HashMap<String, History>) -> ApiPool {
        summarize(
            accounts,
            histories,
            WorkingDays::all(),
            now(),
            &TimeZone::UTC,
        )
        .unwrap()
    }

    #[test]
    fn an_empty_pool_has_no_summary() {
        assert_eq!(
            summarize(
                &[],
                &HashMap::new(),
                WorkingDays::all(),
                now(),
                &TimeZone::UTC
            ),
            None
        );
    }

    #[test]
    fn accounts_out_of_service_are_listed_as_blocked_but_do_not_count() {
        let disabled = AiAccount {
            enabled: false,
            seven_day: usage(10.0, NOW_MS + 48 * HOUR_MS),
            ..account("b")
        };
        let logged_out = AiAccount {
            needs_login: true,
            enabled: false,
            ..account("c")
        };
        let summary = summarize_at(&[account("a"), disabled, logged_out], &HashMap::new());
        let listed: Vec<(&str, Option<&str>)> = summary
            .accounts
            .iter()
            .map(|a| (a.id.as_str(), a.blocked.as_deref()))
            .collect();
        assert_eq!(
            listed,
            [
                ("a", None),
                ("b", Some("Disabled")),
                ("c", Some("Needs login"))
            ]
        );
        assert_eq!(summary.accounts[0].tier.as_deref(), Some("Max 5x"));
        assert!(!summary.pace.from_history);
        assert_eq!(
            summary,
            ApiPool {
                accounts: summary.accounts.clone(),
                ..summarize_at(&[account("a")], &HashMap::new())
            },
            "accounts out of service changed the pool's numbers"
        );
    }

    #[test]
    fn a_blocked_pool_names_why_and_when_it_is_back() {
        let weekly_out = AiAccount {
            seven_day: usage(100.0, NOW_MS + 48 * HOUR_MS),
            ..account("a")
        };
        let cooling = AiAccount {
            cooldown_until_ms: Some(NOW_MS + HOUR_MS),
            rate_limit_tier: Some("default_claude_max_20x".into()),
            ..account("b")
        };
        let summary = summarize_at(&[weekly_out, cooling], &HashMap::new());
        assert_eq!(
            summary.nonstop,
            ApiPoolNonstop::Blocked {
                until: Some((NOW_MS + HOUR_MS) as f64 / 1000.0)
            }
        );
        let blocked: Vec<Option<&str>> = summary
            .accounts
            .iter()
            .map(|account| account.blocked.as_deref())
            .collect();
        assert_eq!(blocked, [Some("7d limit"), Some("429 cooldown")]);
        let until: Vec<Option<f64>> = summary
            .accounts
            .iter()
            .map(|account| account.blocked_until)
            .collect();
        assert_eq!(
            until,
            [
                Some((NOW_MS + 48 * HOUR_MS) as f64 / 1000.0),
                Some((NOW_MS + HOUR_MS) as f64 / 1000.0)
            ]
        );
        assert_eq!(summary.accounts[1].tier.as_deref(), Some("Max 20x"));
    }

    #[test]
    fn amounts_follow_the_pace_and_the_mean_account() {
        let summary = summarize_at(&[account("a")], &HashMap::new());
        // Defaults for one Max 5x account: 100 session points/h at 0.25 weekly
        // points each, so 25 weighted weekly points per hour of work.
        assert_eq!(summary.pace.session_fill_seconds, 5.0 * 3600.0);
        assert_eq!(summary.pace.session_week_percent, 25.0);
        let safe = summary.today.safe_per_day;
        assert!(
            (safe.work_seconds - safe.account_week_percent * 5.0 / 25.0 * 3600.0).abs() < 1e-6,
            "{safe:?}"
        );
        assert_eq!(summary.today.used.work_seconds, 0.0);
        assert_eq!(summary.today.runs_short_at, None);
        assert_eq!(summary.today.runs_short_before, None);
        assert!(matches!(
            summary.nonstop,
            ApiPoolNonstop::Lasts { .. } | ApiPoolNonstop::BeyondHorizon { .. }
        ));
    }

    #[test]
    fn running_short_names_the_account_whose_weekly_reset_it_is() {
        // Nearly spent until its reset in three days: a working day using the
        // long-run share runs the pool short at that reset.
        let tight = AiAccount {
            seven_day: usage(95.0, NOW_MS + 72 * HOUR_MS),
            ..account("tight")
        };
        let summary = summarize_at(&[tight], &HashMap::new());
        let at = summary.today.runs_short_at.expect("the pool runs short");
        let reset = (NOW_MS + 72 * HOUR_MS) as f64 / 1000.0;
        assert!(
            (at - reset).abs() < 60.0,
            "runs short at {at}, reset {reset}"
        );
        assert_eq!(summary.today.runs_short_before.as_deref(), Some("TIGHT"));
    }

    #[test]
    fn windows_map_the_live_usage_without_history() {
        let now_secs = NOW_MS as f64 / 1000.0;
        let weekly_reset = now_secs + 86400.0;
        let account = AiAccount {
            five_hour: usage(40.0, NOW_MS + HOUR_MS),
            seven_day: usage(12.0, NOW_MS + 24 * HOUR_MS),
            ..account("a")
        };
        let history = History::from_samples(vec![sample(
            "a",
            now_secs - 600.0,
            vec![
                ("5h", 30.0, FIVE_HOUR_SECS, now_secs + 3600.0),
                ("7d", 10.0, SEVEN_DAY_SECS, weekly_reset),
            ],
        )]);
        let histories = HashMap::from([("a".to_string(), history)]);
        let summary = summarize_at(&[account], &histories);
        let windows = &summary.accounts[0].windows;
        assert_eq!(
            windows
                .iter()
                .map(|window| (window.id.as_str(), window.label.as_str(), &window.kind))
                .collect::<Vec<_>>(),
            [
                ("five_hour", "Session", &ApiUsageWindowKind::FiveHour),
                ("seven_day", "Weekly", &ApiUsageWindowKind::Weekly),
            ]
        );
        let session = &windows[0];
        assert_eq!(session.used_percent, 40.0);
        assert_eq!(session.window_seconds, 18000.0);
        assert_eq!(session.reset_at, Some(now_secs + 3600.0));
        assert!(session.active);
        let weekly = &windows[1];
        assert_eq!(weekly.used_percent, 12.0);
        assert_eq!(weekly.window_seconds, 604800.0);
        assert_eq!(weekly.reset_at, Some(weekly_reset));
        assert!(windows.iter().all(|window| window.history.is_empty()));
        // The history still feeds the pool math: 10% at the sample to 12% live.
        assert_eq!(summary.accounts[0].used_today_percent, 2.0);
    }

    #[test]
    fn account_history_fills_each_window_from_its_series() {
        let now_secs = NOW_MS as f64 / 1000.0;
        let session_reset = now_secs + 3600.0;
        let weekly_reset = now_secs + 86400.0;
        let account = AiAccount {
            five_hour: usage(40.0, NOW_MS + HOUR_MS),
            seven_day: usage(12.0, NOW_MS + 24 * HOUR_MS),
            ..account("a")
        };
        let history = History::from_samples(vec![sample(
            "a",
            now_secs - 600.0,
            vec![
                ("5h", 30.0, FIVE_HOUR_SECS, session_reset),
                ("7d", 10.0, SEVEN_DAY_SECS, weekly_reset),
            ],
        )]);
        let detail = account_history(&account, &history, now());
        assert_eq!(detail.account_id, "a");
        let series: Vec<(&str, Vec<f64>)> = detail
            .windows
            .iter()
            .map(|window| {
                let used = window.history.iter().map(|p| p.used_percent).collect();
                (window.id.as_str(), used)
            })
            .collect();
        assert_eq!(
            series,
            [("five_hour", vec![30.0]), ("seven_day", vec![10.0])]
        );
        assert_eq!(detail.windows[0].used_percent, 40.0);

        let unknown = account_history(&account, &History::default(), now());
        assert!(unknown.windows.iter().all(|window| window.history.is_empty()));
    }

    #[test]
    fn nonstop_reports_the_runway_in_seconds() {
        // At the default pace a Max 5x account burns 5 weekly points per hour,
        // so its last 2% last 24 minutes.
        let nearly_spent = AiAccount {
            five_hour: usage(50.0, NOW_MS + 4 * HOUR_MS),
            seven_day: usage(98.0, NOW_MS + 48 * HOUR_MS),
            ..account("a")
        };
        let summary = summarize_at(std::slice::from_ref(&nearly_spent), &HashMap::new());
        let empty = History::default();
        let pace = pool::pace(&[(&nearly_spent, &empty)]);
        assert_eq!(
            pool::runway(&[&nearly_spent], pace, NOW_MS),
            Runway::Lasts { ms: 24 * 60 * 1000 }
        );
        assert_eq!(
            summary.nonstop,
            ApiPoolNonstop::Lasts {
                work_seconds: 24.0 * 60.0
            }
        );
    }

    #[test]
    fn an_overspent_day_is_over_and_runs_out_at_the_recent_pace() {
        // Friday 2027-01-15 08:00 UTC; Monday to Friday are working days.
        let day = 86400.0;
        let now_secs = NOW_MS as f64 / 1000.0;
        let reset = now_secs + 2.0 * day;
        let last_reset = reset - 7.0 * day;
        let weekly = |at: f64, used: f64, reset: f64| {
            sample("a", at, vec![("7d", used, SEVEN_DAY_SECS, reset)])
        };
        let friday_midnight = now_secs - 8.0 * 3600.0;
        let saturday_midnight = friday_midnight - 6.0 * day;
        let history = History::from_samples(vec![
            // Last Saturday used 20%, last Sunday nothing.
            weekly(saturday_midnight, 10.0, last_reset),
            weekly(saturday_midnight + day - 60.0, 30.0, last_reset),
            weekly(now_secs - day, 30.0, reset),
            weekly(friday_midnight, 40.0, reset),
        ]);
        let account = AiAccount {
            seven_day: usage(90.0, NOW_MS + 48 * HOUR_MS),
            ..account("a")
        };
        let working = WorkingDays {
            days: [true, true, true, true, true, false, false],
        };
        let summary = summarize(
            &[account],
            &HashMap::from([("a".to_string(), history)]),
            working,
            now(),
            &TimeZone::UTC,
        )
        .unwrap();
        let today = &summary.today;
        assert_eq!(today.used.account_week_percent, 50.0);
        assert_eq!(today.off_day.account_week_percent, 10.0);
        // The weekend alone needs more than the 10% left before the reset.
        assert_eq!(today.safe_per_day.account_week_percent, 0.0);
        assert_eq!(today.left.account_week_percent, -50.0);
        assert!(today.left.work_seconds < 0.0);
        let runs_out = today
            .runs_out_at_recent_pace
            .expect("the last 24 hours' pace runs out");
        assert!(runs_out > now_secs && runs_out <= reset, "{runs_out}");
    }

    #[test]
    fn today_starts_at_the_local_midnight() {
        // 08:00 UTC is 18:00 at UTC+10, whose midnight was 18 hours ago.
        let now_secs = NOW_MS as f64 / 1000.0;
        let reset = now_secs + 2.0 * 86400.0;
        let weekly = |hours_ago: f64, used: f64| {
            sample(
                "a",
                now_secs - hours_ago * 3600.0,
                vec![("7d", used, SEVEN_DAY_SECS, reset)],
            )
        };
        let history = History::from_samples(vec![
            weekly(19.0, 20.0),
            weekly(17.0, 20.0),
            weekly(9.0, 30.0),
            weekly(7.0, 30.0),
        ]);
        let account = AiAccount {
            seven_day: usage(40.0, NOW_MS + 48 * HOUR_MS),
            ..account("a")
        };
        let used_today = |tz: TimeZone| {
            let summary = summarize(
                std::slice::from_ref(&account),
                &HashMap::from([("a".to_string(), history.clone())]),
                WorkingDays::all(),
                now(),
                &tz,
            )
            .unwrap();
            (
                summary.accounts[0].used_today_percent,
                summary.today.used.account_week_percent,
            )
        };
        let east = TimeZone::fixed(jiff::tz::offset(10));
        assert_eq!(used_today(east), (20.0, 20.0));
        assert_eq!(used_today(TimeZone::UTC), (10.0, 10.0));
    }

    #[test]
    fn the_same_minute_gives_the_same_pool() {
        let account = AiAccount {
            five_hour: usage(50.0, NOW_MS + 4 * HOUR_MS),
            seven_day: usage(60.0, NOW_MS + 48 * HOUR_MS),
            ..account("a")
        };
        let at = |ms: i64| {
            summarize(
                std::slice::from_ref(&account),
                &HashMap::new(),
                WorkingDays::all(),
                whole_minute(Timestamp::from_millisecond(ms).unwrap()),
                &TimeZone::UTC,
            )
        };
        assert_eq!(at(NOW_MS + 1_000), at(NOW_MS + 59_000));
    }
}
