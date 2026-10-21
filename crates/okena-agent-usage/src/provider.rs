//! Provider-agnostic fetch outcome, retry/backoff policy, and the per-provider
//! state tracker that turns fetch results into an [`ApiProviderUsage`] entry.

use std::time::{Duration, Instant};

use okena_core::api::{
    ApiExtraUsage, ApiProviderUsage, ApiResetCredits, ApiUsageCredits, ApiUsageProvider,
    ApiUsageStatus, ApiUsageWindow,
};

/// Regular refresh cadence after a successful fetch.
pub const USAGE_INTERVAL: Duration = Duration::from_secs(300);
/// Base of the exponential failure backoff, and the floor for a server-sent
/// `retry-after` (avoids tight loops on `retry-after: 0`).
pub const MIN_RETRY_DELAY: Duration = Duration::from_secs(30);
/// Ceiling for any retry delay.
pub const MAX_RETRY_DELAY: Duration = Duration::from_secs(3600);
/// Delay after a 429 that carried no usable `retry-after` header.
pub const DEFAULT_RATE_LIMIT_DELAY: Duration = Duration::from_secs(600);
/// Minimum spacing between client-requested refreshes of one provider.
pub const REFRESH_THROTTLE: Duration = Duration::from_secs(60);

/// Parsed usage of one provider, before history is attached.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProviderData {
    pub plan: Option<String>,
    pub windows: Vec<ApiUsageWindow>,
    pub extra_usage: Option<ApiExtraUsage>,
    pub credits: Option<ApiUsageCredits>,
    pub reset_credits: Option<ApiResetCredits>,
    pub prepaid_balance: Option<f64>,
}

/// Result of one fetch attempt.
#[derive(Clone, Debug, PartialEq)]
pub enum FetchOutcome {
    Ok(ProviderData),
    /// No usable credentials. Not counted as a failure; checked again at the
    /// regular cadence.
    NoCredentials(String),
    /// HTTP 429. `retry_after` is the server's hint, if any.
    RateLimited {
        retry_after: Option<Duration>,
    },
    /// Anything else (network, non-2xx, bad JSON).
    Failed(String),
}

/// Exponential backoff for `failures` consecutive failures:
/// 30 s, 60 s, 120 s, … capped at one hour.
pub fn failure_backoff(failures: u32) -> Duration {
    if failures == 0 {
        return USAGE_INTERVAL;
    }
    MIN_RETRY_DELAY
        .saturating_mul(1 << failures.min(8).saturating_sub(1))
        .min(MAX_RETRY_DELAY)
}

/// Delay before retrying after a 429: the server hint (or 10 min) but never
/// less than the failure backoff, never more than an hour.
pub fn rate_limit_delay(retry_after: Option<Duration>, failures: u32) -> Duration {
    retry_after
        .unwrap_or(DEFAULT_RATE_LIMIT_DELAY)
        .max(failure_backoff(failures.max(1)))
        .min(MAX_RETRY_DELAY)
}

/// Tracks one provider's fetch state and renders it as a wire entry.
///
/// Timestamps go in twice: an [`Instant`] for scheduling (monotonic) and epoch
/// seconds for the wire. Callers pass both so the tracker stays testable.
#[derive(Clone, Debug)]
pub struct ProviderTracker {
    entry: ApiProviderUsage,
    last_attempt: Option<Instant>,
    rate_limited_until: Option<Instant>,
}

impl ProviderTracker {
    pub fn new(provider: ApiUsageProvider) -> Self {
        Self {
            entry: ApiProviderUsage::empty(provider, ApiUsageStatus::Disabled),
            last_attempt: None,
            rate_limited_until: None,
        }
    }

    pub fn entry(&self) -> &ApiProviderUsage {
        &self.entry
    }

    pub fn is_disabled(&self) -> bool {
        self.entry.status == ApiUsageStatus::Disabled
    }

    /// Turn the provider off: drop all data and scheduling state except a
    /// running 429 backoff, which still applies when it is turned back on.
    pub fn set_disabled(&mut self) {
        let rate_limited_until = self.rate_limited_until;
        *self = Self::new(self.entry.provider);
        self.rate_limited_until = rate_limited_until;
    }

    /// Turn the provider on (or re-point it, e.g. a new Claude config dir):
    /// start over from `pending`. A 429 backoff survives: it is the same
    /// provider API, so toggling the setting must not bypass it.
    pub fn reset_pending(&mut self) {
        let rate_limited_until = self.rate_limited_until;
        let rate_limited_until_epoch = self.entry.rate_limited_until;
        *self = Self::new(self.entry.provider);
        self.entry.status = ApiUsageStatus::Pending;
        self.rate_limited_until = rate_limited_until;
        if rate_limited_until.is_some() {
            self.entry.rate_limited_until = rate_limited_until_epoch;
        }
    }

    /// The earliest time a new fetch may start: `now`, or the end of a
    /// running 429 backoff.
    pub fn earliest_start(&self, now: Instant) -> Instant {
        self.rate_limited_until
            .filter(|&until| until > now)
            .unwrap_or(now)
    }

    /// Record the planned next attempt (`None` = nothing planned).
    pub fn set_next_attempt(&mut self, at_epoch: Option<f64>) {
        self.entry.next_attempt_at = at_epoch;
    }

    /// Mark a fetch as started.
    pub fn begin_attempt(&mut self, now: Instant, now_epoch: f64) {
        self.entry.fetching = true;
        self.entry.last_attempt_at = Some(now_epoch);
        self.entry.next_attempt_at = None;
        self.last_attempt = Some(now);
    }

    /// Whether a client-requested refresh should start a fetch now: the
    /// provider is enabled, idle, not rate limited, and its last attempt is at
    /// least [`REFRESH_THROTTLE`] old. The error backoff only spaces the
    /// automatic poll: a user asking after an outage gets a fresh attempt.
    pub fn refresh_allowed(&self, now: Instant) -> bool {
        if self.is_disabled() || self.entry.fetching {
            return false;
        }
        if self.rate_limited_until.is_some_and(|until| now < until) {
            return false;
        }
        self.last_attempt
            .is_none_or(|last| now.saturating_duration_since(last) >= REFRESH_THROTTLE)
    }

    /// Apply a finished fetch. Returns the delay until the next scheduled
    /// attempt. `data.windows[*].history` should already be filled in.
    pub fn complete(&mut self, outcome: FetchOutcome, now: Instant, now_epoch: f64) -> Duration {
        self.entry.fetching = false;
        let delay = match outcome {
            FetchOutcome::Ok(data) => {
                self.entry.status = ApiUsageStatus::Ok;
                self.entry.error = None;
                self.entry.consecutive_failures = 0;
                self.entry.rate_limited_until = None;
                self.rate_limited_until = None;
                self.entry.fetched_at = Some(now_epoch);
                self.entry.plan = data.plan;
                self.entry.windows = data.windows;
                self.entry.extra_usage = data.extra_usage;
                self.entry.credits = data.credits;
                self.entry.reset_credits = data.reset_credits;
                self.entry.prepaid_balance = data.prepaid_balance;
                USAGE_INTERVAL
            }
            FetchOutcome::NoCredentials(message) => {
                // Signed out: stale numbers would mislead, so drop them.
                let provider = self.entry.provider;
                let last_attempt_at = self.entry.last_attempt_at;
                self.entry = ApiProviderUsage::empty(provider, ApiUsageStatus::NoCredentials);
                self.entry.last_attempt_at = last_attempt_at;
                self.entry.error = Some(message);
                self.rate_limited_until = None;
                USAGE_INTERVAL
            }
            FetchOutcome::RateLimited { retry_after } => {
                self.entry.consecutive_failures = self.entry.consecutive_failures.saturating_add(1);
                let delay = rate_limit_delay(retry_after, self.entry.consecutive_failures);
                self.entry.status = ApiUsageStatus::Error;
                self.entry.error = Some("Rate limited by the usage API (HTTP 429)".into());
                self.rate_limited_until = Some(now + delay);
                self.entry.rate_limited_until = Some(now_epoch + delay.as_secs_f64());
                delay
            }
            FetchOutcome::Failed(message) => {
                self.entry.consecutive_failures = self.entry.consecutive_failures.saturating_add(1);
                self.entry.status = ApiUsageStatus::Error;
                self.entry.error = Some(message);
                failure_backoff(self.entry.consecutive_failures)
            }
        };
        self.entry.next_attempt_at = Some(now_epoch + delay.as_secs_f64());
        delay
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok() -> FetchOutcome {
        FetchOutcome::Ok(ProviderData::default())
    }

    #[test]
    fn backoff_doubles_and_caps() {
        assert_eq!(failure_backoff(1), Duration::from_secs(30));
        assert_eq!(failure_backoff(2), Duration::from_secs(60));
        assert_eq!(failure_backoff(3), Duration::from_secs(120));
        assert_eq!(failure_backoff(100), MAX_RETRY_DELAY);
    }

    #[test]
    fn rate_limit_respects_server_hint_with_floor_and_cap() {
        assert_eq!(
            rate_limit_delay(Some(Duration::from_secs(0)), 1),
            MIN_RETRY_DELAY
        );
        assert_eq!(
            rate_limit_delay(Some(Duration::from_secs(120)), 1),
            Duration::from_secs(120)
        );
        assert_eq!(rate_limit_delay(None, 1), DEFAULT_RATE_LIMIT_DELAY);
        assert_eq!(
            rate_limit_delay(Some(Duration::from_secs(99_999)), 1),
            MAX_RETRY_DELAY
        );
    }

    #[test]
    fn failed_updates_preserve_data_until_recovery() {
        let now = Instant::now();
        let mut tracker = ProviderTracker::new(ApiUsageProvider::Codex);
        tracker.reset_pending();
        tracker.begin_attempt(now, 1000.0);
        let data = ProviderData {
            plan: Some("plus".into()),
            ..ProviderData::default()
        };
        assert_eq!(
            tracker.complete(FetchOutcome::Ok(data), now, 1000.0),
            USAGE_INTERVAL
        );
        let failed_at = now + USAGE_INTERVAL;
        tracker.begin_attempt(failed_at, 1300.0);
        assert_eq!(
            tracker.complete(FetchOutcome::Failed("HTTP 403".into()), failed_at, 1300.0),
            MIN_RETRY_DELAY
        );
        let entry = tracker.entry();
        assert_eq!(entry.status, ApiUsageStatus::Error);
        assert_eq!(entry.fetched_at, Some(1000.0));
        assert_eq!(entry.plan.as_deref(), Some("plus"));
        assert_eq!(entry.error.as_deref(), Some("HTTP 403"));
        assert_eq!(entry.next_attempt_at, Some(1330.0));
        assert_eq!(
            tracker.complete(FetchOutcome::Failed("HTTP 403".into()), failed_at, 1330.0),
            MIN_RETRY_DELAY * 2
        );
        assert_eq!(tracker.entry().consecutive_failures, 2);
        assert_eq!(tracker.complete(ok(), failed_at, 1400.0), USAGE_INTERVAL);
        let entry = tracker.entry();
        assert_eq!(entry.status, ApiUsageStatus::Ok);
        assert_eq!(entry.consecutive_failures, 0);
        assert!(entry.error.is_none());
        assert_eq!(entry.fetched_at, Some(1400.0));
    }

    #[test]
    fn failure_before_first_success_does_not_invent_fresh_data() {
        let now = Instant::now();
        let mut tracker = ProviderTracker::new(ApiUsageProvider::Claude);
        tracker.reset_pending();
        for _ in 0..100 {
            let delay = tracker.complete(FetchOutcome::Failed("Network".into()), now, 0.0);
            assert!(delay <= MAX_RETRY_DELAY);
            assert!(tracker.entry().fetched_at.is_none());
        }
        assert_eq!(tracker.entry().next_attempt_at, Some(3600.0));
    }

    #[test]
    fn refresh_is_throttled_and_blocked_while_rate_limited() {
        let now = Instant::now();
        let mut tracker = ProviderTracker::new(ApiUsageProvider::Claude);
        assert!(!tracker.refresh_allowed(now), "disabled never refreshes");
        tracker.reset_pending();
        assert!(tracker.refresh_allowed(now));
        tracker.begin_attempt(now, 0.0);
        assert!(!tracker.refresh_allowed(now), "in flight");
        tracker.complete(ok(), now, 0.0);
        assert!(!tracker.refresh_allowed(now + Duration::from_secs(59)));
        assert!(tracker.refresh_allowed(now + REFRESH_THROTTLE));

        let later = now + Duration::from_secs(120);
        tracker.begin_attempt(later, 120.0);
        let delay = tracker.complete(
            FetchOutcome::RateLimited {
                retry_after: Some(Duration::from_secs(900)),
            },
            later,
            120.0,
        );
        assert_eq!(delay, Duration::from_secs(900));
        assert_eq!(tracker.entry().rate_limited_until, Some(1020.0));
        assert!(!tracker.refresh_allowed(later + Duration::from_secs(600)));
        assert!(tracker.refresh_allowed(later + Duration::from_secs(900)));
    }

    #[test]
    fn missing_credentials_clears_data_without_backoff() {
        let now = Instant::now();
        let mut tracker = ProviderTracker::new(ApiUsageProvider::Claude);
        tracker.reset_pending();
        tracker.complete(ok(), now, 10.0);
        let delay = tracker.complete(FetchOutcome::NoCredentials("signed out".into()), now, 20.0);
        assert_eq!(delay, USAGE_INTERVAL);
        let entry = tracker.entry();
        assert_eq!(entry.status, ApiUsageStatus::NoCredentials);
        assert!(entry.fetched_at.is_none());
        assert_eq!(entry.consecutive_failures, 0);
        assert_eq!(entry.error.as_deref(), Some("signed out"));
    }

    #[test]
    fn refresh_ignores_the_error_backoff() {
        let now = Instant::now();
        let mut tracker = ProviderTracker::new(ApiUsageProvider::Codex);
        tracker.reset_pending();
        let mut at = now;
        for _ in 0..8 {
            at += REFRESH_THROTTLE;
            tracker.begin_attempt(at, 0.0);
            tracker.complete(FetchOutcome::Failed("HTTP 500".into()), at, 0.0);
        }
        assert_eq!(
            failure_backoff(tracker.entry().consecutive_failures),
            MAX_RETRY_DELAY
        );
        assert!(!tracker.refresh_allowed(at + Duration::from_secs(59)));
        assert!(tracker.refresh_allowed(at + REFRESH_THROTTLE));
    }

    #[test]
    fn rate_limit_survives_reset_and_disable() {
        let now = Instant::now();
        let mut tracker = ProviderTracker::new(ApiUsageProvider::Codex);
        tracker.reset_pending();
        tracker.begin_attempt(now, 100.0);
        tracker.complete(
            FetchOutcome::RateLimited {
                retry_after: Some(Duration::from_secs(900)),
            },
            now,
            100.0,
        );
        tracker.reset_pending();
        assert_eq!(tracker.entry().status, ApiUsageStatus::Pending);
        assert_eq!(tracker.entry().rate_limited_until, Some(1000.0));
        assert!(!tracker.refresh_allowed(now + Duration::from_secs(60)));
        assert_eq!(tracker.earliest_start(now), now + Duration::from_secs(900));

        tracker.set_disabled();
        assert!(tracker.entry().rate_limited_until.is_none());
        tracker.reset_pending();
        assert_eq!(
            tracker.earliest_start(now + Duration::from_secs(10)),
            now + Duration::from_secs(900)
        );
        assert_eq!(
            tracker.earliest_start(now + Duration::from_secs(901)),
            now + Duration::from_secs(901)
        );
    }
}
