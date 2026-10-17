//! Shared agent-usage state between the daemon's usage poller and the HTTP/WS
//! routes.
//!
//! The poller (`okena-daemon-core::usage_poll`) owns the fetching and
//! publishes full [`ApiUsage`] snapshots into a `watch` channel; routes read
//! the latest value and every WS stream forwards changes as
//! `WsOutbound::UsageChanged`. Refresh requests go the other way over an
//! unbounded channel; the poller applies the per-provider throttle.

use std::sync::Arc;
use std::time::Duration;

use okena_core::api::{ApiUsage, ApiUsageProvider};
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

/// Cloneable handle shared by the poller and the routes.
#[derive(Clone)]
pub struct UsageHub {
    state: Arc<watch::Sender<ApiUsage>>,
    refresh_tx: mpsc::UnboundedSender<UsageRefresh>,
}

impl UsageHub {
    /// Create the hub seeded with `initial`, plus the receiver the poller
    /// drains refresh requests from.
    pub fn new(initial: ApiUsage) -> (Self, mpsc::UnboundedReceiver<UsageRefresh>) {
        let (refresh_tx, refresh_rx) = mpsc::unbounded_channel();
        (
            Self {
                state: Arc::new(watch::Sender::new(initial)),
                refresh_tx,
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

    /// Publish a snapshot. Subscribers are only woken when something other
    /// than `generated_at` changed. Returns whether it was published.
    pub fn publish(&self, usage: ApiUsage) -> bool {
        self.state.send_if_modified(|current| {
            let changed =
                current.providers != usage.providers || current.working_days != usage.working_days;
            if changed {
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

#[cfg(test)]
mod tests {
    use super::*;
    use okena_core::api::{ApiProviderUsage, ApiUsageStatus};

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
