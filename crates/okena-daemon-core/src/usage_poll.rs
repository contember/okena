//! Agent usage poller: fetches Claude Code / Codex rate-limit usage on the
//! daemon's machine and publishes it into the remote server's [`UsageHub`].
//!
//! Which providers run is read from the daemon's own settings, the same keys
//! the desktop extensions toggle: `enabled_extensions` contains `claude-code`
//! / `codex`, `extension_settings."claude-code".config_dir` picks the Claude
//! account, and `extension_settings.usage.working_days` is forwarded so
//! clients compute pace the same way. Settings are re-read every
//! [`SETTINGS_POLL`] (cheap: one mutex lock, no I/O), so toggling an extension
//! starts or stops polling within seconds.
//!
//! Scheduling per provider (see `okena_agent_usage::provider`): every 5 min
//! after a success; exponential backoff from 30 s to 1 h after failures; a 429
//! waits for `retry-after` (at least the backoff). Missing credentials are not
//! a failure — they are re-checked every 5 min without any HTTP request.
//! Client refreshes (`POST /v1/usage/refresh`) start a fetch only when the
//! provider's last attempt is ≥ 60 s old and it is not rate limited.
//!
//! Fetches are blocking HTTP (`okena-transport`) and Keychain subprocesses,
//! so they run on tokio's blocking pool; nothing here can take the daemon
//! down — every failure becomes provider state.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use okena_agent_usage::pace::{USAGE_SETTINGS_KEY, WorkingDays};
use okena_agent_usage::{
    CLAUDE_EXTENSION_ID, CODEX_EXTENSION_ID, FetchOutcome, PROVIDERS, ProviderTracker,
};
use okena_core::api::{ApiUsage, ApiUsageProvider};
use okena_remote_server::usage::{UsageHub, UsageRefresh};
use okena_workspace::persistence::AppSettings;
use parking_lot::Mutex;
use tokio::sync::mpsc;

/// How often the poller re-reads settings for extension toggles.
const SETTINGS_POLL: Duration = Duration::from_secs(2);

/// Performs one blocking fetch. `claude_dir` is set for Claude only.
pub type UsageFetcher =
    Arc<dyn Fn(ApiUsageProvider, Option<PathBuf>) -> FetchOutcome + Send + Sync>;

/// The real fetcher: provider HTTP APIs + history recording.
pub fn default_fetcher() -> UsageFetcher {
    Arc::new(|provider, claude_dir| match provider {
        ApiUsageProvider::Claude => match claude_dir {
            Some(dir) => okena_agent_usage::fetch_claude(&dir),
            None => FetchOutcome::NoCredentials("No Claude config directory".into()),
        },
        ApiUsageProvider::Codex => okena_agent_usage::fetch_codex(),
        ApiUsageProvider::Unknown => FetchOutcome::Failed("unknown provider".into()),
    })
}

/// The settings that drive one provider. `key` changes re-point the provider
/// (e.g. another Claude config dir) and restart it from `pending`.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ProviderConfig {
    enabled: bool,
    key: Option<String>,
}

fn extension_id(provider: ApiUsageProvider) -> &'static str {
    match provider {
        ApiUsageProvider::Claude => CLAUDE_EXTENSION_ID,
        ApiUsageProvider::Codex => CODEX_EXTENSION_ID,
        ApiUsageProvider::Unknown => "",
    }
}

fn provider_config(settings: &AppSettings, provider: ApiUsageProvider) -> ProviderConfig {
    let id = extension_id(provider);
    let key = match provider {
        ApiUsageProvider::Claude => settings
            .extension_settings
            .get(id)
            .and_then(|blob| blob.get("config_dir"))
            .and_then(|dir| dir.as_str())
            .map(ToOwned::to_owned),
        _ => None,
    };
    ProviderConfig {
        enabled: settings.enabled_extensions.contains(id),
        key,
    }
}

fn working_days(settings: &AppSettings) -> WorkingDays {
    WorkingDays::from_value(settings.extension_settings.get(USAGE_SETTINGS_KEY))
}

/// Snapshot to seed the hub with before the poller's first pass: enabled
/// providers `pending`, the rest `disabled`.
pub fn initial_usage(settings: &AppSettings) -> ApiUsage {
    let trackers: Vec<ProviderTracker> = PROVIDERS
        .iter()
        .map(|&provider| {
            let mut tracker = ProviderTracker::new(provider);
            if provider_config(settings, provider).enabled {
                tracker.reset_pending();
            }
            tracker
        })
        .collect();
    okena_agent_usage::snapshot(
        &trackers,
        working_days(settings),
        okena_agent_usage::now_epoch(),
    )
}

struct Slot {
    tracker: ProviderTracker,
    config: ProviderConfig,
    /// Bumped on every config change so a fetch started under the old config
    /// is discarded when it lands.
    generation: u64,
    next_due: Option<Instant>,
    in_flight: bool,
}

/// Run the poller forever (cancelled with the daemon's runtime).
pub async fn run_usage_poll(
    settings: Arc<Mutex<AppSettings>>,
    hub: UsageHub,
    refresh_rx: mpsc::UnboundedReceiver<UsageRefresh>,
) {
    run_usage_poll_with(settings, hub, refresh_rx, default_fetcher()).await;
}

/// [`run_usage_poll`] with an injectable fetcher (tests).
pub async fn run_usage_poll_with(
    settings: Arc<Mutex<AppSettings>>,
    hub: UsageHub,
    mut refresh_rx: mpsc::UnboundedReceiver<UsageRefresh>,
    fetcher: UsageFetcher,
) {
    let (result_tx, mut result_rx) = mpsc::unbounded_channel::<(usize, u64, FetchOutcome)>();
    let mut slots: Vec<Slot> = PROVIDERS
        .iter()
        .map(|&provider| Slot {
            tracker: ProviderTracker::new(provider),
            config: ProviderConfig {
                enabled: false,
                key: None,
            },
            generation: 0,
            next_due: None,
            in_flight: false,
        })
        .collect();
    let mut refresh_open = true;

    loop {
        let now = Instant::now();
        let now_epoch = okena_agent_usage::now_epoch();

        // 1. Apply settings changes.
        let (configs, days) = {
            let settings = settings.lock();
            let configs: Vec<ProviderConfig> = PROVIDERS
                .iter()
                .map(|&provider| provider_config(&settings, provider))
                .collect();
            (configs, working_days(&settings))
        };
        for (slot, config) in slots.iter_mut().zip(configs) {
            if slot.config == config {
                continue;
            }
            slot.generation = slot.generation.wrapping_add(1);
            slot.in_flight = false;
            if config.enabled {
                slot.tracker.reset_pending();
                slot.next_due = Some(now);
            } else {
                slot.tracker.set_disabled();
                slot.next_due = None;
            }
            slot.config = config;
        }

        // 2. Start due fetches.
        for (index, slot) in slots.iter_mut().enumerate() {
            let due = slot.next_due.is_some_and(|due| due <= now);
            if !slot.config.enabled || slot.in_flight || !due {
                continue;
            }
            let provider = slot.tracker.entry().provider;
            slot.tracker.begin_attempt(now, now_epoch);
            slot.in_flight = true;
            slot.next_due = None;
            let generation = slot.generation;
            let fetcher = fetcher.clone();
            let result_tx = result_tx.clone();
            // Resolved per fetch (settings dir → CLAUDE_CONFIG_DIR → ~/.claude),
            // exactly like the PTY env override.
            let dir = match provider {
                ApiUsageProvider::Claude => Some(okena_workspace::claude_env::resolve_claude_dir(
                    &settings.lock(),
                )),
                _ => None,
            };
            tokio::spawn(async move {
                let outcome = tokio::task::spawn_blocking(move || fetcher(provider, dir))
                    .await
                    .unwrap_or_else(|error| {
                        log::warn!("[usage] {} fetch task failed: {error}", provider.as_str());
                        FetchOutcome::Failed("Usage fetch crashed".into())
                    });
                let _ = result_tx.send((index, generation, outcome));
            });
        }

        // 3. Publish (no-op for subscribers when nothing changed).
        hub.publish(okena_agent_usage::snapshot(
            slots.iter().map(|slot| &slot.tracker),
            days,
            now_epoch,
        ));

        // 4. Sleep until the next due fetch, a settings re-check, a refresh
        //    request, or a finished fetch.
        let wake = slots
            .iter()
            .filter(|slot| slot.config.enabled && !slot.in_flight)
            .filter_map(|slot| slot.next_due)
            .fold(now + SETTINGS_POLL, Instant::min);
        tokio::select! {
            _ = tokio::time::sleep_until(wake.into()) => {}
            request = refresh_rx.recv(), if refresh_open => match request {
                Some(target) => {
                    let now = Instant::now();
                    for slot in &mut slots {
                        let provider = slot.tracker.entry().provider;
                        if target.is_some_and(|target| target != provider) {
                            continue;
                        }
                        if slot.tracker.refresh_allowed(now) {
                            log::debug!("[usage] refresh requested for {}", provider.as_str());
                            slot.next_due = Some(now);
                        }
                    }
                }
                None => refresh_open = false,
            },
            Some((index, generation, outcome)) = result_rx.recv() => {
                if let Some(slot) = slots.get_mut(index)
                    && slot.generation == generation
                    && slot.in_flight
                {
                    let provider = slot.tracker.entry().provider;
                    match &outcome {
                        FetchOutcome::Ok(_) => log::debug!("[usage] {} fetched", provider.as_str()),
                        FetchOutcome::NoCredentials(reason) => {
                            log::info!("[usage] {}: {reason}", provider.as_str());
                        }
                        FetchOutcome::RateLimited { retry_after } => log::warn!(
                            "[usage] {} rate limited (retry-after {:?})",
                            provider.as_str(),
                            retry_after
                        ),
                        FetchOutcome::Failed(error) => {
                            log::warn!("[usage] {} fetch failed: {error}", provider.as_str());
                        }
                    }
                    let now = Instant::now();
                    let delay = slot.tracker.complete(outcome, now, okena_agent_usage::now_epoch());
                    slot.in_flight = false;
                    slot.next_due = Some(now + delay);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use okena_agent_usage::ProviderData;
    use okena_core::api::{ApiUsageStatus, ApiUsageWindow, ApiUsageWindowKind};
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn settings_with(enabled: &[&str]) -> Arc<Mutex<AppSettings>> {
        let mut settings = AppSettings::default();
        for id in enabled {
            settings.enabled_extensions.insert((*id).to_string());
        }
        settings.extension_settings.insert(
            USAGE_SETTINGS_KEY.to_string(),
            serde_json::json!({ "working_days": [0, 1, 2, 3, 4] }),
        );
        Arc::new(Mutex::new(settings))
    }

    fn ok_outcome() -> FetchOutcome {
        FetchOutcome::Ok(ProviderData {
            plan: Some("max".into()),
            windows: vec![ApiUsageWindow {
                id: "five_hour".into(),
                label: "Session".into(),
                kind: ApiUsageWindowKind::FiveHour,
                used_percent: 42.0,
                window_seconds: 18000.0,
                reset_at: Some(1.0e10),
                active: true,
                history: Vec::new(),
            }],
            ..ProviderData::default()
        })
    }

    async fn wait_for(hub: &UsageHub, mut pred: impl FnMut(&ApiUsage) -> bool) -> ApiUsage {
        let mut rx = hub.subscribe();
        loop {
            let current = rx.borrow_and_update().clone();
            if pred(&current) {
                return current;
            }
            tokio::time::timeout(Duration::from_secs(30), rx.changed())
                .await
                .expect("usage did not reach the expected state")
                .expect("hub alive");
        }
    }

    #[test]
    fn initial_usage_reflects_enabled_extensions() {
        let settings = settings_with(&["codex"]);
        let usage = initial_usage(&settings.lock());
        assert_eq!(usage.providers.len(), 2);
        assert_eq!(usage.providers[0].status, ApiUsageStatus::Disabled);
        assert_eq!(usage.providers[1].status, ApiUsageStatus::Pending);
        assert_eq!(usage.working_days, vec![0, 1, 2, 3, 4]);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn polls_enabled_provider_throttles_refresh_and_stops_when_disabled() {
        let settings = settings_with(&["claude-code"]);
        let (hub, refresh_rx) = UsageHub::new(initial_usage(&settings.lock()));
        let calls = Arc::new(AtomicUsize::new(0));
        let fetcher: UsageFetcher = {
            let calls = calls.clone();
            Arc::new(move |provider, dir| {
                assert_eq!(provider, ApiUsageProvider::Claude, "codex is disabled");
                assert!(dir.is_some(), "claude gets its config dir");
                calls.fetch_add(1, Ordering::SeqCst);
                ok_outcome()
            })
        };
        let task = tokio::spawn(run_usage_poll_with(
            settings.clone(),
            hub.clone(),
            refresh_rx,
            fetcher,
        ));

        let usage = wait_for(&hub, |u| u.providers[0].status == ApiUsageStatus::Ok).await;
        assert_eq!(usage.providers[0].windows[0].used_percent, 42.0);
        assert_eq!(usage.providers[0].plan.as_deref(), Some("max"));
        assert!(usage.providers[0].fetched_at.is_some());
        assert_eq!(usage.providers[1].status, ApiUsageStatus::Disabled);
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // A refresh right after a fetch is throttled (60 s).
        hub.request_refresh(None);
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // Disabling the extension drops the data.
        settings.lock().enabled_extensions.clear();
        wait_for(&hub, |u| u.providers[0].status == ApiUsageStatus::Disabled).await;
        task.abort();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn failures_become_state_not_crashes() {
        let settings = settings_with(&["codex"]);
        let (hub, refresh_rx) = UsageHub::new(initial_usage(&settings.lock()));
        let fetcher: UsageFetcher =
            Arc::new(|_, _| FetchOutcome::Failed("Usage request failed: HTTP 500".into()));
        let task = tokio::spawn(run_usage_poll_with(
            settings,
            hub.clone(),
            refresh_rx,
            fetcher,
        ));
        let usage = wait_for(&hub, |u| u.providers[1].status == ApiUsageStatus::Error).await;
        let codex = &usage.providers[1];
        assert_eq!(codex.consecutive_failures, 1);
        assert_eq!(
            codex.error.as_deref(),
            Some("Usage request failed: HTTP 500")
        );
        assert!(codex.next_attempt_at.is_some());
        assert!(!codex.fetching);
        task.abort();
    }
}
