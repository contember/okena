//! The account pool reduced to the user's own clock. Every account has a 5-hour
//! and a 7-day window with its own reset, so their percentages cannot be added
//! up. Instead the pool answers two questions on one timeline: how long nonstop
//! work can go on before every account is blocked (runway), and how much today
//! may still use without the pool running short later in the week (today).
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

#[derive(Clone, Copy, Debug)]
struct WeeklyPoint {
    at_secs: f64,
    used: f64,
    reset_secs: Option<f64>,
}

/// Weekly points the account used between `from` and `to` (Unix seconds),
/// measured from its history and the live window at `now`; increments across a
/// reset count from zero.
pub(crate) fn weekly_used_between(
    account: &AiAccount,
    history: &History,
    from: f64,
    to: f64,
    now: Timestamp,
) -> f64 {
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
    points.retain(|point| point.at_secs <= to);
    let baseline = points
        .iter()
        .rposition(|point| point.at_secs <= from)
        .unwrap_or(0);
    points
        .get(baseline..)
        .unwrap_or_default()
        .windows(2)
        .map(|pair| {
            let (before, after) = (pair[0], pair[1]);
            let same_window = match (before.reset_secs, after.reset_secs) {
                (Some(a), Some(b)) => (a - b).abs() < RESET_TOLERANCE_SECS,
                _ => true,
            };
            if same_window {
                (after.used - before.used).max(0.0)
            } else {
                after.used
            }
        })
        .sum()
}

/// What today may still use without the pool running short within the next
/// week. All quantities are weighted weekly points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Today {
    /// Used since local midnight.
    pub used: f64,
    /// The most each working day may use, today included, so that no shortage
    /// occurs within the [`HORIZON_DAYS`] starting today. At most the pool's
    /// long-run share.
    pub safe_per_day: f64,
    /// Where a working day using more than `safe_per_day` runs short; `None`
    /// when the long-run share is the limit, not a moment.
    pub bottleneck_ms: Option<i64>,
    /// Used in the last 24 hours.
    pub recent_per_day: f64,
    /// When the pool runs short if every working day uses `recent_per_day`;
    /// `None` when that pace lasts the horizon.
    pub wall_at_recent_pace_ms: Option<i64>,
    /// What a non-working day uses, from the history of the last week.
    pub off_day: f64,
}

impl Today {
    pub fn left(&self) -> f64 {
        self.safe_per_day - self.used
    }
}

/// Local days the day plan looks ahead, today included.
pub(crate) const HORIZON_DAYS: usize = 7;

/// One account's weekly window in weighted points.
#[derive(Clone, Copy, Debug)]
struct WeeklyQuota {
    weight: f64,
    used: f64,
    reset_ms: Option<i64>,
}

/// One local day of the horizon.
#[derive(Clone, Copy, Debug)]
struct Day {
    start_ms: i64,
    end_ms: i64,
    working: bool,
}

pub(crate) fn today(
    accounts: &[(&AiAccount, &History)],
    working: WorkingDays,
    now: Timestamp,
    tz: &TimeZone,
) -> Today {
    let now_ms = now.as_millisecond();
    let now_secs = now_ms as f64 / 1000.0;
    let days = local_days(now, working, tz);
    let day_start = days.first().map_or(now_ms, |day| day.start_ms) as f64 / 1000.0;
    let weighted_between = |from: f64, to: f64| -> f64 {
        accounts
            .iter()
            .map(|(account, history)| {
                weekly_used_between(account, history, from, to, now) * plan_weight(account)
            })
            .sum()
    };
    let used = weighted_between(day_start, now_secs);
    let recent_per_day = weighted_between(now_secs - 86400.0, now_secs);
    let off_day = off_day_usage(accounts, working, now, tz, &weighted_between);
    let quotas: Vec<WeeklyQuota> = accounts
        .iter()
        .map(|(account, _)| {
            let (used, reset_ms) = match &account.seven_day {
                Some(usage) if usage.resets_at_ms.is_none_or(|reset| reset > now_ms) => {
                    (usage.utilization, usage.resets_at_ms)
                }
                _ => (0.0, None),
            };
            WeeklyQuota {
                weight: plan_weight(account),
                used,
                reset_ms,
            }
        })
        .collect();
    let plan = DayPlan {
        quotas: &quotas,
        days: &days,
        now_ms,
        used_today: used,
        off_day,
    };

    let weekly_capacity: f64 = quotas.iter().map(|quota| 100.0 * quota.weight).sum();
    let off_days = 7 - working.count();
    let long_run =
        ((weekly_capacity - off_day * off_days as f64) / working.count().max(1) as f64).max(0.0);
    let (safe_per_day, bottleneck_ms) = match plan.shortage(long_run) {
        None => (long_run, None),
        Some(_) => {
            let (mut low, mut high) = (0.0, long_run);
            for _ in 0..40 {
                let middle = (low + high) / 2.0;
                match plan.shortage(middle) {
                    None => low = middle,
                    Some(_) => high = middle,
                }
            }
            (low, plan.shortage(high))
        }
    };
    Today {
        used,
        safe_per_day,
        bottleneck_ms,
        recent_per_day,
        wall_at_recent_pace_ms: plan.shortage(recent_per_day),
        off_day,
    }
}

/// Average use of the non-working days of the last week that the history
/// covers whole.
fn off_day_usage(
    accounts: &[(&AiAccount, &History)],
    working: WorkingDays,
    now: Timestamp,
    tz: &TimeZone,
    weighted_between: &dyn Fn(f64, f64) -> f64,
) -> f64 {
    let first_sample = accounts
        .iter()
        .flat_map(|(_, history)| history.samples())
        .map(|sample| sample.recorded_at)
        .min_by(f64::total_cmp);
    let Some(first_sample) = first_sample else {
        return 0.0;
    };
    let today = now.to_zoned(tz.clone()).date();
    let mut total = 0.0;
    let mut counted = 0;
    for back in 1..=7 {
        let Ok(date) = today.checked_sub(back.days()) else {
            continue;
        };
        if working.days[date.weekday().to_monday_zero_offset() as usize] {
            continue;
        }
        let (Some(start), Some(end)) = (
            local_midnight(date, tz),
            date.tomorrow()
                .ok()
                .and_then(|next| local_midnight(next, tz)),
        ) else {
            continue;
        };
        let (start, end) = (start as f64 / 1000.0, end as f64 / 1000.0);
        if first_sample > start {
            continue;
        }
        total += weighted_between(start, end);
        counted += 1;
    }
    if counted == 0 {
        0.0
    } else {
        total / counted as f64
    }
}

fn local_midnight(date: Date, tz: &TimeZone) -> Option<i64> {
    Some(date.to_zoned(tz.clone()).ok()?.timestamp().as_millisecond())
}

/// [`HORIZON_DAYS`] local days from today on.
fn local_days(now: Timestamp, working: WorkingDays, tz: &TimeZone) -> Vec<Day> {
    let mut days = Vec::new();
    let mut date = now.to_zoned(tz.clone()).date();
    while days.len() < HORIZON_DAYS {
        let (Some(start_ms), Some(end_ms)) = (
            local_midnight(date, tz),
            date.tomorrow()
                .ok()
                .and_then(|next| local_midnight(next, tz)),
        ) else {
            break;
        };
        days.push(Day {
            start_ms,
            end_ms,
            working: working.days[date.weekday().to_monday_zero_offset() as usize],
        });
        let Ok(next) = date.tomorrow() else {
            break;
        };
        date = next;
    }
    days
}

struct DayPlan<'a> {
    quotas: &'a [WeeklyQuota],
    days: &'a [Day],
    now_ms: i64,
    used_today: f64,
    off_day: f64,
}

impl DayPlan<'_> {
    /// When the pool first cannot serve `per_day` on every working day (and
    /// the usual use of the others), drawing the quota that resets first.
    /// Today's remainder spreads over the rest of today.
    fn shortage(&self, per_day: f64) -> Option<i64> {
        let mut quotas = self.quotas.to_vec();
        for (index, day) in self.days.iter().enumerate() {
            let budget = if day.working { per_day } else { self.off_day };
            let (start, amount) = if index == 0 {
                (self.now_ms, (budget - self.used_today).max(0.0))
            } else {
                (day.start_ms, budget)
            };
            let end = day.end_ms;
            if end <= start {
                continue;
            }
            let per_ms = amount / (end - start) as f64;
            let mut t = start;
            while t < end {
                for quota in &mut quotas {
                    if let Some(reset) = quota.reset_ms.filter(|reset| *reset <= t) {
                        quota.used = 0.0;
                        quota.reset_ms = Some(reset + WEEK_MS);
                    }
                }
                let next = quotas
                    .iter()
                    .filter_map(|quota| quota.reset_ms)
                    .filter(|reset| *reset > t)
                    .min()
                    .unwrap_or(end)
                    .min(end);
                let mut need = per_ms * (next - t) as f64;
                let available: f64 = quotas
                    .iter()
                    .map(|quota| (100.0 - quota.used).max(0.0) * quota.weight)
                    .sum();
                if need > available + 1e-9 {
                    return Some(t + (available / per_ms) as i64);
                }
                quotas.sort_by_key(|quota| quota.reset_ms.unwrap_or(i64::MAX));
                for quota in &mut quotas {
                    if need <= 0.0 {
                        break;
                    }
                    let take = need.min((100.0 - quota.used).max(0.0) * quota.weight);
                    if take > 0.0 {
                        quota.used += take / quota.weight;
                        quota.reset_ms.get_or_insert(t + WEEK_MS);
                        need -= take;
                    }
                }
                t = next;
            }
        }
        None
    }
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
    fn weekly_use_counts_across_a_reset() {
        let now = noon(jiff::civil::date(2026, 10, 14));
        let old_reset = secs(now) - 3600.0;
        let new_reset = old_reset + 7.0 * 86400.0;
        let history = History::from_samples(vec![
            sample(secs(now) - 13.0 * 3600.0, None, Some((90.0, old_reset))),
            sample(secs(now) - 2.0 * 3600.0, None, Some((96.0, old_reset))),
            sample(secs(now) - 0.5 * 3600.0, None, Some((3.0, new_reset))),
        ]);
        let live = AiAccount {
            seven_day: usage(5.0, Some((new_reset * 1000.0) as i64)),
            ..account(None)
        };
        let midnight = secs(now) - 12.0 * 3600.0;
        // 6 before the reset, 5 after it.
        assert_eq!(
            weekly_used_between(&live, &history, midnight, secs(now), now),
            11.0
        );
        assert_eq!(
            weekly_used_between(&live, &history, midnight, secs(now) - 3.0 * 3600.0, now),
            0.0
        );
    }

    #[test]
    fn safe_use_is_limited_by_the_quota_left_before_the_next_reset() {
        // Wednesday noon. A has 20 left until Friday noon, B is out until
        // Saturday noon; both are Max 5x.
        let now = noon(jiff::civil::date(2026, 10, 14));
        let a = AiAccount {
            seven_day: usage(80.0, Some(now.as_millisecond() + 2 * 24 * HOUR_MS)),
            ..account(None)
        };
        let b = AiAccount {
            seven_day: usage(100.0, Some(now.as_millisecond() + 3 * 24 * HOUR_MS)),
            ..account(None)
        };
        let empty = History::default();
        let today = today(
            &[(&a, &empty), (&b, &empty)],
            WorkingDays::all(),
            now,
            &TimeZone::UTC,
        );
        // The rest of Wednesday, Thursday and half of Friday share A's 100
        // weighted points: 2.5 days of 40.
        assert!((today.safe_per_day - 40.0).abs() < 0.01);
        let bottleneck = today.bottleneck_ms.unwrap();
        assert!((bottleneck - (now.as_millisecond() + 2 * 24 * HOUR_MS)).abs() < HOUR_MS);
        assert_eq!(today.used, 0.0);
        assert_eq!(today.wall_at_recent_pace_ms, None);
    }

    #[test]
    fn a_full_pool_is_limited_by_its_long_run_share() {
        let now = noon(jiff::civil::date(2026, 10, 14));
        let fresh = account(None);
        let empty = History::default();
        let weekdays = WorkingDays {
            days: [true, true, true, true, true, false, false],
        };
        let today = today(&[(&fresh, &empty)], weekdays, now, &TimeZone::UTC);
        // 500 weighted points a week over 5 working days.
        assert_eq!(today.safe_per_day, 100.0);
        assert_eq!(today.bottleneck_ms, None);
    }

    #[test]
    fn recent_use_finds_the_wall_and_off_days_count() {
        // Wednesday noon; Sunday used 10 weekly points, the last day 30.
        let now = noon(jiff::civil::date(2026, 10, 14));
        let reset = secs(now) + 5.0 * 86400.0;
        let at = |days_back: f64| secs(now) - days_back * 86400.0;
        let history = History::from_samples(vec![
            sample(at(4.0), None, Some((5.0, reset))),
            sample(at(3.5), None, Some((10.0, reset))),
            sample(at(3.0), None, Some((15.0, reset))),
            sample(at(2.5), None, Some((20.0, reset))),
            sample(at(1.0), None, Some((40.0, reset))),
        ]);
        let live = AiAccount {
            seven_day: usage(70.0, Some((reset * 1000.0) as i64)),
            ..account(None)
        };
        let weekdays = WorkingDays {
            days: [true, true, true, true, true, false, false],
        };
        let today = today(&[(&live, &history)], weekdays, now, &TimeZone::UTC);
        assert_eq!(today.off_day, 50.0);
        assert_eq!(today.recent_per_day, 150.0);
        // Today has used its 150 already and Thursday takes the 150 left, so
        // Friday runs short at once.
        let wall = today.wall_at_recent_pace_ms.unwrap();
        assert!((wall - (now.as_millisecond() + 36 * HOUR_MS)).abs() < HOUR_MS);
    }
}
