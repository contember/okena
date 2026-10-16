//! The account pool reduced to the user's own clock. Every account has a 5-hour
//! and a 7-day window with its own reset, so their percentages cannot be added
//! up. Instead the pool answers two questions that share one timeline: how long
//! work can go on before every account is blocked (runway), and how much of the
//! remaining weekly quota today may use (daily budget).
//!
//! Quantities are weighted by plan size, so 1% of a Max 20x account counts four
//! times as much as 1% of a Max 5x account.

use jiff::civil::Date;
use jiff::tz::TimeZone;
use jiff::{Timestamp, ToSpan};
use okena_core::ai_proxy::{AiAccount, AiUsage};
use okena_usage::WorkingDays;
use okena_usage::history::{History, LimitSample, UsageSample};

const HOUR_MS: i64 = 60 * 60 * 1000;
const SESSION_MS: i64 = 5 * HOUR_MS;
const WEEK_MS: i64 = 7 * 24 * HOUR_MS;
/// The runway simulation stops here; anything longer reads as "a week or more".
pub(crate) const RUNWAY_HORIZON_MS: i64 = WEEK_MS;
/// Consecutive samples further apart than this do not say when usage happened.
const MAX_SAMPLE_GAP_SECS: f64 = 15.0 * 60.0;
/// Reset timestamps jitter between polls.
const RESET_TOLERANCE_SECS: f64 = 60.0;
/// Session usage (percentage points) the history needs before its 5h:7d ratio
/// replaces the default.
const MIN_SESSION_POINTS_FOR_RATIO: f64 = 20.0;
const MIN_ACTIVE_HOURS_FOR_RATE: usize = 3;
/// Weekly points one session point costs, measured on Max accounts.
const DEFAULT_WEEKLY_PER_SESSION_POINT: f64 = 0.25;
/// A full 5-hour window used evenly, in session points per hour.
const DEFAULT_SESSION_POINTS_PER_HOUR: f64 = 20.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PlanSize {
    Max20x,
    Max5x,
    /// Pro, or a plan whose size is not known.
    Base,
}

impl PlanSize {
    /// From the tier the provider reports; a Max account without one is the
    /// smaller Max.
    pub fn of(account: &AiAccount) -> Self {
        let tier = account.rate_limit_tier.as_deref().unwrap_or_default();
        if tier.ends_with("_20x") {
            Self::Max20x
        } else if tier.ends_with("_5x") || account.plan.as_deref() == Some("max") {
            Self::Max5x
        } else {
            Self::Base
        }
    }

    /// Quota relative to Pro.
    pub fn weight(self) -> f64 {
        match self {
            Self::Max20x => 20.0,
            Self::Max5x => 5.0,
            Self::Base => 1.0,
        }
    }
}

pub(crate) fn plan_weight(account: &AiAccount) -> f64 {
    PlanSize::of(account).weight()
}

/// Accounts the proxy can route to at all; blocked ones still count, since
/// their quota comes back.
pub(crate) fn in_service(account: &AiAccount) -> bool {
    account.enabled && !account.needs_login
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Pace {
    /// Weighted session points burned per hour of active work.
    pub session_points_per_hour: f64,
    /// Weekly points one session point costs.
    pub weekly_per_session_point: f64,
    /// Both values come from the usage history rather than the defaults.
    pub from_history: bool,
}

/// The pool's pace over the last week: the 75th percentile of the hours with
/// any usage, so short bursts and idle tails do not drag it down.
pub(crate) fn pace(accounts: &[(&AiAccount, &History)]) -> Pace {
    let mut session_points = 0.0;
    let mut weekly_points = 0.0;
    let mut hours = std::collections::BTreeMap::<i64, f64>::new();
    for (account, history) in accounts {
        let weight = plan_weight(account);
        let mut samples: Vec<&UsageSample> = history.samples().iter().collect();
        samples.sort_by(|a, b| a.recorded_at.total_cmp(&b.recorded_at));
        for pair in samples.windows(2) {
            let (before, after) = (pair[0], pair[1]);
            if after.recorded_at - before.recorded_at > MAX_SAMPLE_GAP_SECS {
                continue;
            }
            let Some(session) = same_window_delta(before, after, "5h") else {
                continue;
            };
            if session <= 0.0 {
                continue;
            }
            *hours
                .entry((after.recorded_at / 3600.0).floor() as i64)
                .or_default() += session * weight;
            if let Some(weekly) = same_window_delta(before, after, "7d") {
                session_points += session;
                weekly_points += weekly.max(0.0);
            }
        }
    }
    let ratio = (session_points >= MIN_SESSION_POINTS_FOR_RATIO && weekly_points > 0.0)
        .then(|| weekly_points / session_points);
    let mut per_hour: Vec<f64> = hours.into_values().collect();
    per_hour.sort_by(f64::total_cmp);
    let rate =
        (per_hour.len() >= MIN_ACTIVE_HOURS_FOR_RATE).then(|| per_hour[per_hour.len() * 3 / 4]);
    let mean_weight = if accounts.is_empty() {
        1.0
    } else {
        accounts
            .iter()
            .map(|(account, _)| plan_weight(account))
            .sum::<f64>()
            / accounts.len() as f64
    };
    Pace {
        session_points_per_hour: rate.unwrap_or(DEFAULT_SESSION_POINTS_PER_HOUR * mean_weight),
        weekly_per_session_point: ratio.unwrap_or(DEFAULT_WEEKLY_PER_SESSION_POINT),
        from_history: ratio.is_some() && rate.is_some(),
    }
}

fn same_window_delta(before: &UsageSample, after: &UsageSample, name: &str) -> Option<f64> {
    let (before, after) = (limit(before, name)?, limit(after, name)?);
    let same_reset = (before.reset_at? - after.reset_at?).abs() < RESET_TOLERANCE_SECS;
    same_reset.then_some(after.used_percent - before.used_percent)
}

fn limit<'a>(sample: &'a UsageSample, name: &str) -> Option<&'a LimitSample> {
    sample.limits.iter().find(|limit| limit.name == name)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Runway {
    /// Nonstop work at the pool's pace blocks every account after this long.
    Lasts { ms: i64 },
    /// No account blocks within [`RUNWAY_HORIZON_MS`].
    BeyondHorizon,
    /// Every account is blocked now; the first one is back at `until_ms`,
    /// unless no reset is known.
    Blocked { until_ms: Option<i64> },
}

#[derive(Clone, Debug)]
struct SimAccount {
    weight: f64,
    session: f64,
    session_reset: Option<i64>,
    weekly: f64,
    weekly_reset: Option<i64>,
    cooldown_until: i64,
}

impl SimAccount {
    fn new(account: &AiAccount, now: i64) -> Self {
        let window = |usage: Option<&AiUsage>| match usage {
            Some(usage) if usage.resets_at_ms.is_none_or(|reset| reset > now) => {
                (usage.utilization, usage.resets_at_ms)
            }
            _ => (0.0, None),
        };
        let (session, session_reset) = window(account.five_hour.as_ref());
        let (weekly, weekly_reset) = window(account.seven_day.as_ref());
        Self {
            weight: plan_weight(account),
            session,
            session_reset,
            weekly,
            weekly_reset,
            cooldown_until: account.cooldown_until_ms.unwrap_or(0),
        }
    }

    fn reset_due(&mut self, now: i64) {
        if self.session_reset.is_some_and(|reset| reset <= now) {
            self.session = 0.0;
            self.session_reset = None;
        }
        if self.weekly_reset.is_some_and(|reset| reset <= now) {
            self.weekly = 0.0;
            self.weekly_reset = None;
        }
    }

    fn usable(&self, now: i64) -> bool {
        self.session < 100.0 && self.weekly < 100.0 && self.cooldown_until <= now
    }

    /// When the account stops being blocked, or `None` if a blocking window
    /// has no known reset.
    fn unblocked_at(&self, now: i64) -> Option<i64> {
        let mut at = self.cooldown_until.max(now);
        for (used, reset) in [
            (self.session, self.session_reset),
            (self.weekly, self.weekly_reset),
        ] {
            if used >= 100.0 {
                at = at.max(reset?);
            }
        }
        Some(at)
    }

    fn next_event(&self, now: i64) -> Option<i64> {
        [
            self.session_reset,
            self.weekly_reset,
            Some(self.cooldown_until),
        ]
        .into_iter()
        .flatten()
        .filter(|at| *at > now)
        .min()
    }
}

/// Simulate the proxy serving nonstop work at `pace`: one account at a time,
/// the one whose weekly quota expires first, switching when it blocks and
/// returning accounts as their windows reset.
pub(crate) fn runway(accounts: &[&AiAccount], pace: Pace, now: i64) -> Runway {
    let mut pool: Vec<SimAccount> = accounts
        .iter()
        .map(|account| SimAccount::new(account, now))
        .collect();
    if pool.is_empty() {
        return Runway::Blocked { until_ms: None };
    }
    let horizon = now + RUNWAY_HORIZON_MS;
    let mut t = now;
    // Each step crosses at least one event; the bound only guards rounding.
    for _ in 0..10_000 {
        for account in &mut pool {
            account.reset_due(t);
        }
        let chosen = pool
            .iter()
            .enumerate()
            .filter(|(_, account)| account.usable(t))
            .min_by_key(|(_, account)| account.weekly_reset.unwrap_or(i64::MAX))
            .map(|(index, _)| index);
        let Some(chosen) = chosen else {
            if t == now {
                return Runway::Blocked {
                    until_ms: pool
                        .iter()
                        .filter_map(|account| account.unblocked_at(t))
                        .min(),
                };
            }
            return Runway::Lasts { ms: t - now };
        };
        let next_event = pool
            .iter()
            .filter_map(|account| account.next_event(t))
            .min()
            .unwrap_or(i64::MAX);
        let account = &mut pool[chosen];
        account.session_reset.get_or_insert(t + SESSION_MS);
        account.weekly_reset.get_or_insert(t + WEEK_MS);
        let session_per_ms = pace.session_points_per_hour / account.weight / HOUR_MS as f64;
        let weekly_per_ms = session_per_ms * pace.weekly_per_session_point;
        let until_full = |used: f64, per_ms: f64| {
            if per_ms > 0.0 {
                ((100.0 - used) / per_ms).round() as i64
            } else {
                i64::MAX
            }
        };
        let step = until_full(account.session, session_per_ms)
            .min(until_full(account.weekly, weekly_per_ms))
            .min(next_event.min(account.session_reset.unwrap_or(i64::MAX)) - t)
            .min(horizon - t)
            .max(1);
        account.session = (account.session + session_per_ms * step as f64).min(100.0);
        account.weekly = (account.weekly + weekly_per_ms * step as f64).min(100.0);
        // Rounding the step to whole milliseconds can stop a hair below the cap.
        if 100.0 - account.session < 1e-3 {
            account.session = 100.0;
        }
        if 100.0 - account.weekly < 1e-3 {
            account.weekly = 100.0;
        }
        t += step;
        if t >= horizon {
            return Runway::BeyondHorizon;
        }
    }
    Runway::BeyondHorizon
}

/// Weighted weekly points: what today has used and what it may use.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct DailyBudget {
    pub used: f64,
    pub allowance: f64,
}

impl DailyBudget {
    pub fn percent(&self) -> Option<f64> {
        (self.allowance > 0.0).then(|| self.used / self.allowance * 100.0)
    }
}

impl std::ops::AddAssign for DailyBudget {
    fn add_assign(&mut self, other: Self) {
        self.used += other.used;
        self.allowance += other.allowance;
    }
}

#[derive(Clone, Copy, Debug)]
struct WeeklyPoint {
    at_secs: f64,
    used: f64,
    reset_secs: Option<f64>,
}

/// The account's share of today's budget: the weekly quota left at midnight,
/// spread evenly over the working days until it resets. Today always counts as
/// one of them. A reset during today adds the new window's first day.
pub(crate) fn daily_budget(
    account: &AiAccount,
    history: &History,
    working: WorkingDays,
    now: Timestamp,
    tz: &TimeZone,
) -> DailyBudget {
    let today = now.to_zoned(tz.clone()).date();
    let Ok(day_start) = today.to_zoned(tz.clone()) else {
        return DailyBudget::default();
    };
    let day_start = day_start.timestamp().as_millisecond() as f64 / 1000.0;
    let now_secs = now.as_millisecond() as f64 / 1000.0;
    let mut points: Vec<WeeklyPoint> = history
        .samples()
        .iter()
        .filter(|sample| sample.recorded_at <= now_secs)
        .filter_map(|sample| {
            let limit = limit(sample, "7d")?;
            Some(WeeklyPoint {
                at_secs: sample.recorded_at,
                used: limit.used_percent,
                reset_secs: limit.reset_at,
            })
        })
        .collect();
    if let Some(live) = &account.seven_day {
        points.push(WeeklyPoint {
            at_secs: now_secs,
            used: live.utilization,
            reset_secs: live.resets_at_ms.map(|ms| ms as f64 / 1000.0),
        });
    }
    points.sort_by(|a, b| a.at_secs.total_cmp(&b.at_secs));
    let baseline_index = points
        .iter()
        .rposition(|point| point.at_secs <= day_start)
        .unwrap_or(0);
    let Some(baseline) = points.get(baseline_index).copied() else {
        return DailyBudget::default();
    };

    let mut used = 0.0;
    for pair in points[baseline_index..].windows(2) {
        let (before, after) = (pair[0], pair[1]);
        let same_window = match (before.reset_secs, after.reset_secs) {
            (Some(a), Some(b)) => (a - b).abs() < RESET_TOLERANCE_SECS,
            _ => true,
        };
        used += if same_window {
            (after.used - before.used).max(0.0)
        } else {
            after.used
        };
    }

    let live_reset = account
        .seven_day
        .as_ref()
        .and_then(|usage| usage.resets_at_ms)
        .map(|ms| ms as f64 / 1000.0);
    let (left_at_midnight, reset) = match baseline.reset_secs {
        Some(reset) if reset <= day_start => (100.0, live_reset),
        reset => (100.0 - baseline.used, reset.or(live_reset)),
    };
    let tomorrow = today.tomorrow().ok();
    let week_days = days_to_spread(today, today.checked_add(6.days()).unwrap_or(today), working);
    let allowance = match reset.and_then(date_of_last_moment(tz)) {
        Some(last_day) if Some(last_day) < tomorrow => left_at_midnight + 100.0 / week_days as f64,
        Some(last_day) => left_at_midnight / days_to_spread(today, last_day, working) as f64,
        None => left_at_midnight / week_days as f64,
    };
    let weight = plan_weight(account);
    DailyBudget {
        used: used * weight,
        allowance: allowance * weight,
    }
}

/// The local date of the last moment before `reset`.
fn date_of_last_moment(tz: &TimeZone) -> impl Fn(f64) -> Option<Date> + '_ {
    move |reset| {
        Timestamp::from_millisecond((reset * 1000.0) as i64 - 1)
            .ok()
            .map(|at| at.to_zoned(tz.clone()).date())
    }
}

/// Working days from `today` to `last` inclusive; today counts even when it is
/// not a working day, since usage is happening on it.
fn days_to_spread(today: Date, last: Date, working: WorkingDays) -> usize {
    let mut count = 1;
    let mut day = today;
    while let Ok(next) = day.tomorrow() {
        if next > last {
            break;
        }
        if working.days[next.weekday().to_monday_zero_offset() as usize] {
            count += 1;
        }
        day = next;
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;
    use okena_usage::history::Provider;

    const NOW: i64 = 1_800_000_000_000;

    fn account(weight_tier: Option<&str>) -> AiAccount {
        AiAccount {
            id: "a".into(),
            label: "A".into(),
            email: None,
            plan: Some("max".into()),
            rate_limit_tier: weight_tier.map(Into::into),
            enabled: true,
            needs_login: false,
            cooldown_until_ms: None,
            five_hour: None,
            seven_day: None,
            requests: 0,
            last_used_ms: None,
        }
    }

    fn usage(utilization: f64, resets_at_ms: Option<i64>) -> Option<AiUsage> {
        Some(AiUsage {
            utilization,
            resets_at_ms,
        })
    }

    fn sample(
        at_secs: f64,
        session: Option<(f64, f64)>,
        weekly: Option<(f64, f64)>,
    ) -> UsageSample {
        let limit = |name: &str, window: f64, (used, reset): (f64, f64)| LimitSample {
            name: name.into(),
            used_percent: used,
            window_seconds: window,
            reset_at: Some(reset),
        };
        UsageSample {
            version: 1,
            recorded_at: at_secs,
            provider: Provider::Claude,
            source: "a".into(),
            plan: None,
            limits: session
                .map(|value| limit("5h", 18000.0, value))
                .into_iter()
                .chain(weekly.map(|value| limit("7d", 604800.0, value)))
                .collect(),
        }
    }

    const PACE: Pace = Pace {
        session_points_per_hour: 100.0,
        weekly_per_session_point: 0.25,
        from_history: true,
    };

    #[test]
    fn plan_weight_follows_the_tier() {
        assert_eq!(plan_weight(&account(Some("default_claude_max_20x"))), 20.0);
        assert_eq!(plan_weight(&account(Some("default_claude_max_5x"))), 5.0);
        assert_eq!(plan_weight(&account(None)), 5.0);
        let pro = AiAccount {
            plan: Some("pro".into()),
            ..account(None)
        };
        assert_eq!(plan_weight(&pro), 1.0);
    }

    #[test]
    fn runway_ends_when_the_session_window_fills() {
        // 250 weighted points/h burn 50 session points/h of a Max 5x account.
        let fresh = account(None);
        assert_eq!(
            runway(
                &[&fresh],
                Pace {
                    session_points_per_hour: 250.0,
                    ..PACE
                },
                NOW
            ),
            Runway::Lasts { ms: 2 * HOUR_MS }
        );
    }

    #[test]
    fn runway_moves_to_the_next_account_and_waits_for_resets() {
        let half = AiAccount {
            five_hour: usage(50.0, Some(NOW + 4 * HOUR_MS)),
            ..account(None)
        };
        let full = AiAccount {
            five_hour: usage(100.0, Some(NOW + HOUR_MS)),
            ..account(None)
        };
        // At 50 session points/h the half-used account fills in 1 h, just as
        // the full one resets; that one then fills in 2 h, before either
        // session window resets again.
        let pace = Pace {
            session_points_per_hour: 250.0,
            ..PACE
        };
        assert_eq!(
            runway(&[&half, &full], pace, NOW),
            Runway::Lasts { ms: 3 * HOUR_MS }
        );
    }

    #[test]
    fn blocked_pool_names_the_first_account_back() {
        let weekly_out = AiAccount {
            five_hour: usage(0.0, None),
            seven_day: usage(100.0, Some(NOW + 48 * HOUR_MS)),
            ..account(None)
        };
        let cooling = AiAccount {
            cooldown_until_ms: Some(NOW + HOUR_MS),
            ..account(None)
        };
        assert_eq!(
            runway(&[&weekly_out, &cooling], PACE, NOW),
            Runway::Blocked {
                until_ms: Some(NOW + HOUR_MS)
            }
        );
        let unknown = AiAccount {
            seven_day: usage(100.0, None),
            ..account(None)
        };
        assert_eq!(
            runway(&[&unknown], PACE, NOW),
            Runway::Blocked { until_ms: None }
        );
    }

    #[test]
    fn a_slow_pace_outlasts_the_horizon() {
        let pace = Pace {
            session_points_per_hour: 1.0,
            ..PACE
        };
        assert_eq!(runway(&[&account(None)], pace, NOW), Runway::BeyondHorizon);
    }

    #[test]
    fn pace_comes_from_session_and_weekly_deltas() {
        let reset5 = 1_000_000.0;
        let reset7 = 2_000_000.0;
        let samples: Vec<UsageSample> = (0..48)
            .map(|step| {
                let at = 3600.0 * 100.0 + step as f64 * 300.0;
                let used = step as f64;
                sample(at, Some((used, reset5)), Some((used / 4.0, reset7)))
            })
            .collect();
        let history = History::from_samples(samples);
        let fresh = account(None);
        let pace = pace(&[(&fresh, &history)]);
        assert!(pace.from_history);
        assert!((pace.weekly_per_session_point - 0.25).abs() < 1e-9);
        // 12 samples per hour, one point each, weighted by 5.
        assert!((pace.session_points_per_hour - 60.0).abs() < 1e-9);
    }

    #[test]
    fn pace_without_history_uses_defaults() {
        let fresh = account(None);
        let pace = pace(&[(&fresh, &History::default())]);
        assert!(!pace.from_history);
        assert_eq!(pace.session_points_per_hour, 100.0);
        assert_eq!(pace.weekly_per_session_point, 0.25);
    }

    fn noon(date: Date) -> Timestamp {
        date.at(12, 0, 0, 0)
            .to_zoned(TimeZone::UTC)
            .unwrap()
            .timestamp()
    }

    fn secs(at: Timestamp) -> f64 {
        at.as_millisecond() as f64 / 1000.0
    }

    #[test]
    fn daily_budget_spreads_what_was_left_at_midnight() {
        // Wednesday; the weekly window resets Friday noon.
        let today = jiff::civil::date(2026, 10, 14);
        let now = noon(today);
        let reset = secs(noon(jiff::civil::date(2026, 10, 16)));
        let history = History::from_samples(vec![
            sample(secs(now) - 13.0 * 3600.0, None, Some((40.0, reset))),
            sample(secs(now) - 2.0 * 3600.0, None, Some((46.0, reset))),
        ]);
        let live = AiAccount {
            seven_day: usage(49.0, Some((reset * 1000.0) as i64)),
            ..account(Some("default_claude_max_5x"))
        };
        let budget = daily_budget(&live, &history, WorkingDays::all(), now, &TimeZone::UTC);
        // 60 left at midnight over Wed, Thu, Fri; 9 used since; weight 5.
        assert_eq!(
            budget,
            DailyBudget {
                used: 45.0,
                allowance: 100.0
            }
        );
        let weekdays = WorkingDays {
            days: [true, true, true, false, true, false, false],
        };
        // Thursday is off: the 60 spreads over Wed and Fri only.
        let budget = daily_budget(&live, &history, weekdays, now, &TimeZone::UTC);
        assert_eq!(budget.allowance, 150.0);
    }

    #[test]
    fn daily_budget_follows_a_reset_during_the_day() {
        let today = jiff::civil::date(2026, 10, 14);
        let now = noon(today);
        let old_reset = secs(now) - 3600.0;
        let new_reset = old_reset + 7.0 * 86400.0;
        let history = History::from_samples(vec![
            sample(secs(now) - 13.0 * 3600.0, None, Some((90.0, old_reset))),
            sample(secs(now) - 2.0 * 3600.0, None, Some((96.0, old_reset))),
            sample(secs(now) - 0.5 * 3600.0, None, Some((3.0, new_reset))),
        ]);
        let live = AiAccount {
            seven_day: usage(5.0, Some((new_reset * 1000.0) as i64)),
            ..account(Some("default_claude_max_5x"))
        };
        let budget = daily_budget(&live, &history, WorkingDays::all(), now, &TimeZone::UTC);
        // Used 6 before the reset and 5 after; the expiring 10 plus a seventh
        // of the new window are today's.
        assert!((budget.used - 11.0 * 5.0).abs() < 1e-9);
        assert!((budget.allowance - (10.0 + 100.0 / 7.0) * 5.0).abs() < 1e-9);
    }

    #[test]
    fn daily_budget_without_history_uses_the_live_window() {
        let today = jiff::civil::date(2026, 10, 14);
        let now = noon(today);
        let live = AiAccount {
            seven_day: usage(
                30.0,
                Some(noon(jiff::civil::date(2026, 10, 20)).as_millisecond()),
            ),
            ..account(Some("default_claude_max_5x"))
        };
        let budget = daily_budget(
            &live,
            &History::default(),
            WorkingDays::all(),
            now,
            &TimeZone::UTC,
        );
        assert_eq!(
            budget,
            DailyBudget {
                used: 0.0,
                allowance: 70.0 / 7.0 * 5.0
            }
        );
    }
}
