//! Shared agent-usage state between the daemon's usage poller and the HTTP/WS
//! routes.
//!
//! The poller (`okena-daemon-core::usage_poll`) owns the fetching and
//! publishes full [`ApiUsage`] snapshots; the AI proxy's accounts poll
//! (`okena-daemon-core::ai_proxy`) sets the account pool. The hub puts the
//! pool into the poller's Claude entry and keeps the result in a `watch`
//! channel; routes read the latest value and every WS stream forwards changes
//! as `WsOutbound::UsageChanged`. Refresh requests go the other way over an
//! unbounded channel; the poller applies the per-provider throttle. Each WS
//! stream registers as a client ([`UsageHub::register_client`]); both polls
//! pause their automatic cadence while there is none.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use okena_core::api::{ApiPool, ApiUsage, ApiUsageProvider, ApiUsageStatus};
use parking_lot::Mutex;
use tokio::sync::{mpsc, oneshot, watch};

/// How long [`UsageHub::refresh`] waits for the poller to apply a request.
const REFRESH_APPLY_TIMEOUT: Duration = Duration::from_secs(2);

/// A refresh request for the poller.
#[derive(Debug)]
pub struct UsageRefresh {
    /// One provider, or `None` for all enabled ones.
    pub provider: Option<ApiUsageProvider>,
    /// Signalled once the poller has applied the request (started the
    /// fetches it allows) and published the resulting snapshot.
    pub applied: Option<oneshot::Sender<()>>,
}

/// Cloneable handle shared by the poller, the AI proxy's accounts poll and
/// the routes.
#[derive(Clone)]
pub struct UsageHub {
    /// What the two publishers last gave; the lock also orders their merges.
    sources: Arc<Mutex<UsageSources>>,
    state: Arc<watch::Sender<ApiUsage>>,
    refresh_tx: mpsc::UnboundedSender<UsageRefresh>,
    /// Number of connected clients that receive `usage_changed`.
    clients: Arc<watch::Sender<usize>>,
}

/// A connected client, counted by [`UsageHub::clients`] until dropped.
pub struct UsageClient {
    clients: Arc<watch::Sender<usize>>,
}

impl Drop for UsageClient {
    fn drop(&mut self) {
        self.clients
            .send_modify(|count| *count = count.saturating_sub(1));
    }
}

struct UsageSources {
    polled: ApiUsage,
    pool: Option<ApiPool>,
    /// [`UsageHub::set_pool`] was called at least once.
    pool_reported: bool,
}

impl UsageSources {
    /// The poller's snapshot with the pool in the Claude entry, unless the
    /// user turned Claude usage off.
    fn merged(&self) -> ApiUsage {
        let mut usage = self.polled.clone();
        for entry in &mut usage.providers {
            if entry.provider == ApiUsageProvider::Claude
                && entry.status != ApiUsageStatus::Disabled
            {
                entry.pool = self.pool.clone();
            }
        }
        usage
    }
}

impl UsageHub {
    /// Create the hub seeded with `initial`, plus the receiver the poller
    /// drains refresh requests from.
    pub fn new(initial: ApiUsage) -> (Self, mpsc::UnboundedReceiver<UsageRefresh>) {
        let (refresh_tx, refresh_rx) = mpsc::unbounded_channel();
        let sources = UsageSources {
            polled: initial,
            pool: None,
            pool_reported: false,
        };
        (
            Self {
                state: Arc::new(watch::Sender::new(sources.merged())),
                sources: Arc::new(Mutex::new(sources)),
                refresh_tx,
                clients: Arc::new(watch::Sender::new(0)),
            },
            refresh_rx,
        )
    }

    /// A hub with nobody polling (tests, or a server without a daemon core).
    pub fn detached() -> Self {
        Self::new(ApiUsage::default()).0
    }

    /// The latest snapshot.
    pub fn snapshot(&self) -> ApiUsage {
        self.state.borrow().clone()
    }

    /// Subscribe to snapshot changes. The current value counts as seen.
    pub fn subscribe(&self) -> watch::Receiver<ApiUsage> {
        self.state.subscribe()
    }

    /// Count a client as connected for as long as the returned guard lives.
    pub fn register_client(&self) -> UsageClient {
        self.clients.send_modify(|count| *count += 1);
        UsageClient {
            clients: self.clients.clone(),
        }
    }

    /// The number of connected clients; changes when one comes or goes.
    pub fn clients(&self) -> watch::Receiver<usize> {
        self.clients.subscribe()
    }

    /// Whether the `claude` login's usage is the relevant one: the accounts
    /// poll has reported and published no pool. While a pool is set, new
    /// shells use its accounts, whose usage comes from the proxy.
    pub fn login_in_use(&self) -> bool {
        let sources = self.sources.lock();
        sources.pool_reported && sources.pool.is_none()
    }

    /// Publish the poller's snapshot; the pool set by [`Self::set_pool`] stays.
    /// Subscribers are only woken when something other than `generated_at`
    /// changed. Returns whether it was published.
    pub fn publish(&self, usage: ApiUsage) -> bool {
        self.publish_at(usage, now_epoch())
    }

    /// Set the AI proxy's account pool (`None` while new shells do not use
    /// the proxy). Returns whether a changed snapshot was published.
    pub fn set_pool(&self, pool: Option<ApiPool>) -> bool {
        self.set_pool_at(pool, now_epoch())
    }

    fn publish_at(&self, usage: ApiUsage, now: f64) -> bool {
        let mut sources = self.sources.lock();
        sources.polled = usage;
        self.send(sources.merged(), now)
    }

    fn set_pool_at(&self, pool: Option<ApiPool>, now: f64) -> bool {
        let mut sources = self.sources.lock();
        sources.pool_reported = true;
        if sources.pool == pool {
            return false;
        }
        sources.pool = pool;
        self.send(sources.merged(), now)
    }

    /// A published change is stamped no earlier than `now` nor the snapshot
    /// it replaces, so a pool change between two polls never repeats the
    /// poller's `generated_at` and a clock step back never makes clients drop
    /// it as older than the one they have.
    fn send(&self, mut usage: ApiUsage, now: f64) -> bool {
        self.state.send_if_modified(|current| {
            let changed =
                current.providers != usage.providers || current.working_days != usage.working_days;
            if changed {
                usage.generated_at = usage.generated_at.max(now).max(current.generated_at);
                *current = usage;
            }
            changed
        })
    }

    /// Ask the poller to refetch. Fire-and-forget; the poller throttles.
    pub fn request_refresh(&self, provider: Option<ApiUsageProvider>) {
        let _ = self.refresh_tx.send(UsageRefresh {
            provider,
            applied: None,
        });
    }

    /// Ask the poller to refetch and return the snapshot once it has applied
    /// the request, so `fetching` tells the caller whether a fetch actually
    /// started (or was already running). Falls back to the current snapshot
    /// when nobody polls or the poller does not answer within 2 s.
    pub async fn refresh(&self, provider: Option<ApiUsageProvider>) -> ApiUsage {
        let (applied, done) = oneshot::channel();
        if self
            .refresh_tx
            .send(UsageRefresh {
                provider,
                applied: Some(applied),
            })
            .is_ok()
        {
            let _ = tokio::time::timeout(REFRESH_APPLY_TIMEOUT, done).await;
        }
        self.snapshot()
    }
}

fn now_epoch() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |elapsed| elapsed.as_secs_f64())
}

#[cfg(test)]
mod tests {
    use super::*;
    use okena_core::api::{
        ApiPoolAmount, ApiPoolNonstop, ApiPoolPace, ApiPoolToday, ApiProviderUsage,
    };

    #[test]
    fn publish_ignores_timestamp_only_changes() {
        let (hub, mut refresh_rx) = UsageHub::new(ApiUsage::default());
        let mut rx = hub.subscribe();
        let mut usage = ApiUsage {
            providers: vec![ApiProviderUsage::empty(
                ApiUsageProvider::Claude,
                ApiUsageStatus::Pending,
            )],
            working_days: vec![0, 1, 2, 3, 4],
            generated_at: 1.0,
        };
        assert!(hub.publish(usage.clone()));
        assert!(rx.has_changed().unwrap());
        rx.borrow_and_update();
        usage.generated_at = 2.0;
        assert!(!hub.publish(usage));
        assert!(!rx.has_changed().unwrap());

        hub.request_refresh(Some(ApiUsageProvider::Codex));
        let request = refresh_rx.try_recv().unwrap();
        assert_eq!(request.provider, Some(ApiUsageProvider::Codex));
        assert!(request.applied.is_none());
    }

    fn pool(left_seconds: f64) -> ApiPool {
        let amount = ApiPoolAmount {
            work_seconds: left_seconds,
            account_week_percent: 1.0,
        };
        ApiPool {
            today: ApiPoolToday {
                left: amount,
                used: amount,
                safe_per_day: amount,
                off_day: amount,
                runs_short_at: None,
                runs_short_before: None,
                runs_out_at_recent_pace: None,
            },
            nonstop: ApiPoolNonstop::Blocked { until: None },
            pace: ApiPoolPace {
                session_fill_seconds: 18000.0,
                session_week_percent: 25.0,
                from_history: false,
            },
            accounts: Vec::new(),
        }
    }

    fn polled(generated_at: f64) -> ApiUsage {
        ApiUsage {
            providers: vec![
                ApiProviderUsage::empty(ApiUsageProvider::Claude, ApiUsageStatus::Pending),
                ApiProviderUsage::empty(ApiUsageProvider::Codex, ApiUsageStatus::Disabled),
            ],
            working_days: vec![0, 1, 2, 3, 4],
            generated_at,
        }
    }

    fn claude_pool(usage: &ApiUsage) -> Option<ApiPool> {
        usage.provider(ApiUsageProvider::Claude)?.pool.clone()
    }

    #[test]
    fn the_pool_goes_into_the_claude_entry_and_survives_polls() {
        let hub = UsageHub::detached();
        let mut rx = hub.subscribe();
        assert!(hub.publish(polled(1.0)));
        rx.borrow_and_update();

        assert!(hub.set_pool(Some(pool(3600.0))));
        assert!(rx.has_changed().unwrap());
        let usage = rx.borrow_and_update().clone();
        assert_eq!(claude_pool(&usage), Some(pool(3600.0)));
        assert_eq!(usage.provider(ApiUsageProvider::Codex).unwrap().pool, None);

        // A new poll keeps the pool.
        let mut next = polled(2.0);
        next.providers[0].status = ApiUsageStatus::Ok;
        assert!(hub.publish(next));
        assert_eq!(claude_pool(&hub.snapshot()), Some(pool(3600.0)));
        assert_eq!(hub.snapshot().providers[0].status, ApiUsageStatus::Ok);

        assert!(hub.set_pool(None));
        assert_eq!(claude_pool(&hub.snapshot()), None);
    }

    #[test]
    fn an_unchanged_pool_does_not_publish() {
        let hub = UsageHub::detached();
        hub.publish(polled(1.0));
        assert!(hub.set_pool(Some(pool(3600.0))));
        let rx = hub.subscribe();
        assert!(!hub.set_pool(Some(pool(3600.0))));
        assert!(!hub.publish(polled(2.0)));
        assert!(!rx.has_changed().unwrap());
        assert!(hub.set_pool(Some(pool(1800.0))));
        assert!(rx.has_changed().unwrap());
    }

    #[test]
    fn a_published_change_is_never_older_than_now() {
        let hub = UsageHub::detached();
        assert!(hub.publish_at(polled(100.0), 50.0));
        assert_eq!(hub.snapshot().generated_at, 100.0);

        // A pool change between polls is stamped with the time it happened.
        assert!(hub.set_pool_at(Some(pool(3600.0)), 160.0));
        assert_eq!(hub.snapshot().generated_at, 160.0);

        // A poll older than that does not move the stamp back.
        let mut next = polled(130.0);
        next.providers[0].status = ApiUsageStatus::Ok;
        assert!(hub.publish_at(next.clone(), 170.0));
        assert_eq!(hub.snapshot().generated_at, 170.0);

        // Timestamp-only polls still publish nothing.
        next.generated_at = 400.0;
        assert!(!hub.publish_at(next.clone(), 400.0));
        assert_eq!(hub.snapshot().generated_at, 170.0);

        // After the clock steps back, a change keeps the newest stamp.
        next.providers[0].status = ApiUsageStatus::Error;
        next.generated_at = 80.0;
        assert!(hub.publish_at(next, 90.0));
        assert_eq!(hub.snapshot().generated_at, 170.0);
    }

    #[test]
    fn a_disabled_claude_entry_carries_no_pool() {
        let hub = UsageHub::detached();
        let mut disabled = polled(1.0);
        disabled.providers[0].status = ApiUsageStatus::Disabled;
        hub.publish(disabled);
        hub.set_pool(Some(pool(3600.0)));
        assert_eq!(claude_pool(&hub.snapshot()), None);

        // Turning Claude usage back on shows the pool the proxy last gave.
        hub.publish(polled(2.0));
        assert_eq!(claude_pool(&hub.snapshot()), Some(pool(3600.0)));
    }

    #[test]
    fn the_login_is_in_use_once_the_accounts_poll_reports_no_pool() {
        let hub = UsageHub::detached();
        assert!(!hub.login_in_use(), "not known before the first report");
        hub.set_pool(None);
        assert!(hub.login_in_use());
        hub.set_pool(Some(pool(3600.0)));
        assert!(!hub.login_in_use());
        hub.set_pool(None);
        assert!(hub.login_in_use());
    }

    #[test]
    fn clients_are_counted_until_their_guard_drops() {
        let hub = UsageHub::detached();
        let clients = hub.clients();
        assert_eq!(*clients.borrow(), 0);
        let first = hub.register_client();
        let second = hub.clone().register_client();
        assert_eq!(*clients.borrow(), 2);
        drop(first);
        assert_eq!(*clients.borrow(), 1);
        drop(second);
        assert_eq!(*clients.borrow(), 0);
    }

    #[tokio::test]
    async fn refresh_waits_for_the_poller_and_tolerates_no_poller() {
        let (hub, mut refresh_rx) = UsageHub::new(ApiUsage::default());
        let poller_hub = hub.clone();
        let poller = tokio::spawn(async move {
            let request = refresh_rx.recv().await.unwrap();
            assert_eq!(request.provider, None);
            poller_hub.publish(ApiUsage {
                working_days: vec![1],
                ..ApiUsage::default()
            });
            request.applied.unwrap().send(()).unwrap();
            refresh_rx
        });
        assert_eq!(hub.refresh(None).await.working_days, vec![1]);
        drop(poller.await.unwrap());

        // Receiver gone: returns the current snapshot immediately.
        assert_eq!(hub.refresh(None).await.working_days, vec![1]);
    }
}
