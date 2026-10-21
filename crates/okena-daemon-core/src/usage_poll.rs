//! Agent usage poller: fetches Claude Code / Codex / Grok usage on the
//! daemon's machine and publishes it into the remote server's [`UsageHub`].
//!
//! Which providers run is read from the daemon's own settings, the same keys
//! the desktop extensions toggle: `enabled_extensions` contains `claude-code`
//! / `codex` / `grok`, `extension_settings."claude-code".config_dir` picks
//! the Claude account, and `extension_settings.usage.working_days` is
//! forwarded so clients compute pace the same way. Settings are re-read every
//! [`SETTINGS_POLL`] (cheap: one mutex lock, no I/O), so toggling an extension
//! starts or stops polling within seconds.
//!
//! The `claude` login is polled only while it serves new shells: once the AI
//! proxy's accounts poll has published a pool, those shells use the pool
//! accounts and their usage comes from the proxy (`ai_proxy`). The automatic
//! poll runs only while a client is connected ([`UsageHub::clients`]); a fetch
//! that fell due meanwhile starts as soon as one connects. Refresh requests
//! are served with no client too.
//!
//! Scheduling per provider (see `okena_agent_usage::provider`): every 5 min
//! after a success; exponential backoff from 30 s to 1 h after failures; a 429
//! waits for `retry-after` (at least the backoff). Missing credentials are not
//! a failure — they are re-checked every 5 min without any HTTP request.
//! Client refreshes (`POST /v1/usage/refresh`) start a fetch when the
//! provider's last attempt is ≥ 60 s old and it is not rate limited, also
//! during an error backoff (see `ProviderTracker::refresh_allowed`). The
//! request is acked after the resulting snapshot is published, so the POST
//! answers with an accurate `fetching` flag.
//!
//! Fetches are blocking HTTP (`okena-transport`) and Keychain subprocesses,
//! so they run on tokio's blocking pool; nothing here can take the daemon
//! down — every failure becomes provider state. At most one fetch per
//! provider runs at a time, also across config changes and past the
//! watchdog: a slot stays taken until its blocking call has returned (a Codex
//! token refresh rotates the refresh token, and two concurrent rotations can
//! sign the user out). Every step of a fetch has its own timeout (HTTP 10 s,
//! Keychain 5 s), which also bounds how long the runtime's drop waits for a
//! fetch at daemon shutdown; [`FETCH_DEADLINE`] is a watchdog on top that turns
//! a fetch stuck anyway into an error instead of `fetching` forever.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use okena_agent_usage::pace::{USAGE_SETTINGS_KEY, WorkingDays};
use okena_agent_usage::{
    CLAUDE_EXTENSION_ID, CODEX_EXTENSION_ID, FetchOutcome, GROK_EXTENSION_ID, PROVIDERS,
    ProviderTracker,
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
        ApiUsageProvider::Grok => okena_agent_usage::fetch_grok(),
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
        ApiUsageProvider::Grok => GROK_EXTENSION_ID,
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

/// A fetch running on the blocking pool. Outlives config changes and the
/// watchdog: a new fetch for the slot only starts once this one has landed
/// (its result discarded when the config changed or the watchdog fired
/// meanwhile), so two Codex refresh-token rotations can never race each other.
struct RunningFetch {
    id: u64,
    /// Slot generation the fetch was started under.
    generation: u64,
    /// The watchdog already turned this fetch into a failure.
    timed_out: bool,
}

struct Slot {
    tracker: ProviderTracker,
    config: ProviderConfig,
    /// Bumped on every config change so a fetch started under the old config
    /// is discarded when it lands.
    generation: u64,
    next_due: Option<Instant>,
    /// A refresh request set `next_due`; it starts with no client connected.
    requested: bool,
    running: Option<RunningFetch>,
}

impl Slot {
    /// Whether the slot may start a fetch once it is due.
    fn may_start(&self, watched: bool, login_in_use: bool) -> bool {
        let paused = self.tracker.entry().provider == ApiUsageProvider::Claude && !login_in_use;
        self.config.enabled && self.running.is_none() && !paused && (watched || self.requested)
    }
}

/// Events from the per-fetch supervisor task.
enum FetchEvent {
    /// The fetch passed its deadline and is still running.
    TimedOut { index: usize, id: u64 },
    /// The blocking fetch returned (or panicked).
    Landed {
        index: usize,
        id: u64,
        outcome: FetchOutcome,
    },
}

/// Overall deadline for one fetch. Every step inside is bounded already
/// (HTTP requests 10 s each, Keychain lookups 5 s each; Codex can chain up to
/// four requests), so this only catches something unexpectedly stuck: the
/// provider then shows an error and backs off instead of `fetching` forever.
/// The fetch keeps its slot until it returns.
const FETCH_DEADLINE: Duration = Duration::from_secs(90);

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
    refresh_rx: mpsc::UnboundedReceiver<UsageRefresh>,
    fetcher: UsageFetcher,
) {
    run_usage_poll_inner(settings, hub, refresh_rx, fetcher, FETCH_DEADLINE).await;
}

/// Resolve the Claude config dir for a fetch (settings dir →
/// `CLAUDE_CONFIG_DIR` → `~/.claude`, exactly like the PTY env override).
/// Called on the blocking pool: it touches the filesystem.
fn resolve_claude_dir(config_dir: Option<String>) -> PathBuf {
    let mut settings = AppSettings::default();
    if let Some(dir) = config_dir {
        settings.extension_settings.insert(
            CLAUDE_EXTENSION_ID.to_string(),
            serde_json::json!({ "config_dir": dir }),
        );
    }
    okena_workspace::claude_env::resolve_claude_dir(&settings)
}

async fn run_usage_poll_inner(
    settings: Arc<Mutex<AppSettings>>,
    hub: UsageHub,
    mut refresh_rx: mpsc::UnboundedReceiver<UsageRefresh>,
    fetcher: UsageFetcher,
    fetch_deadline: Duration,
) {
    let (event_tx, mut event_rx) = mpsc::unbounded_channel::<FetchEvent>();
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
            requested: false,
            running: None,
        })
        .collect();
    let mut clients = hub.clients();
    let mut refresh_open = true;
    let mut next_fetch_id: u64 = 0;
    // Refresh requests applied since the last publish; acked after it.
    let mut pending_acks: Vec<tokio::sync::oneshot::Sender<()>> = Vec::new();

    loop {
        let now = Instant::now();
        let now_epoch = okena_agent_usage::now_epoch();

        // 1. Apply settings changes. A running fetch is left alone: it keeps
        //    blocking new fetches until it lands, then its result is dropped.
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
            if config.enabled {
                slot.tracker.reset_pending();
                slot.next_due = Some(slot.tracker.earliest_start(now));
            } else {
                slot.tracker.set_disabled();
                slot.next_due = None;
            }
            slot.config = config;
        }

        // 2. Start due fetches.
        let watched = *clients.borrow_and_update() > 0;
        let login_in_use = hub.login_in_use();
        for (index, slot) in slots.iter_mut().enumerate() {
            let due = slot.next_due.is_some_and(|due| due <= now);
            if !due || !slot.may_start(watched, login_in_use) {
                continue;
            }
            let provider = slot.tracker.entry().provider;
            slot.tracker.begin_attempt(now, now_epoch);
            slot.next_due = None;
            slot.requested = false;
            next_fetch_id = next_fetch_id.wrapping_add(1);
            let id = next_fetch_id;
            slot.running = Some(RunningFetch {
                id,
                generation: slot.generation,
                timed_out: false,
            });
            let fetcher = fetcher.clone();
            let event_tx = event_tx.clone();
            let config_dir = slot.config.key.clone();
            tokio::spawn(async move {
                let mut join = tokio::task::spawn_blocking(move || {
                    let dir = match provider {
                        ApiUsageProvider::Claude => Some(resolve_claude_dir(config_dir)),
                        _ => None,
                    };
                    fetcher(provider, dir)
                });
                let joined = match tokio::time::timeout(fetch_deadline, &mut join).await {
                    Ok(joined) => joined,
                    Err(_) => {
                        let _ = event_tx.send(FetchEvent::TimedOut { index, id });
                        join.await
                    }
                };
                let outcome = joined.unwrap_or_else(|error| {
                    log::warn!("[usage] {} fetch task failed: {error}", provider.as_str());
                    FetchOutcome::Failed("Usage fetch crashed".into())
                });
                let _ = event_tx.send(FetchEvent::Landed { index, id, outcome });
            });
        }

        // 3. Publish (no-op for subscribers when nothing changed), then tell
        //    refresh callers their request is reflected.
        hub.publish(okena_agent_usage::snapshot(
            slots.iter().map(|slot| &slot.tracker),
            days,
            now_epoch,
        ));
        for ack in pending_acks.drain(..) {
            let _ = ack.send(());
        }

        // 4. Sleep until the next due fetch, a settings re-check, a refresh
        //    request, a client connecting, or a fetch event.
        let wake = slots
            .iter()
            .filter(|slot| slot.may_start(watched, login_in_use))
            .filter_map(|slot| slot.next_due)
            .fold(now + SETTINGS_POLL, Instant::min);
        tokio::select! {
            _ = tokio::time::sleep_until(wake.into()) => {}
            Ok(()) = clients.changed() => {}
            request = refresh_rx.recv(), if refresh_open => match request {
                Some(UsageRefresh { provider: target, applied }) => {
                    let now = Instant::now();
                    let login_in_use = hub.login_in_use();
                    for slot in &mut slots {
                        let provider = slot.tracker.entry().provider;
                        if target.is_some_and(|target| target != provider) {
                            continue;
                        }
                        if slot.tracker.refresh_allowed(now) && slot.may_start(true, login_in_use) {
                            log::debug!("[usage] refresh requested for {}", provider.as_str());
                            slot.next_due = Some(now);
                            slot.requested = true;
                        }
                    }
                    pending_acks.extend(applied);
                }
                None => refresh_open = false,
            },
            Some(event) = event_rx.recv() => match event {
                FetchEvent::TimedOut { index, id } => {
                    if let Some(slot) = slots.get_mut(index)
                        && let Some(running) = slot.running.as_mut()
                        && running.id == id
                        && running.generation == slot.generation
                    {
                        running.timed_out = true;
                        let provider = slot.tracker.entry().provider;
                        log::warn!(
                            "[usage] {} fetch exceeded {:?}; marking it failed",
                            provider.as_str(),
                            fetch_deadline
                        );
                        let now = Instant::now();
                        let delay = slot.tracker.complete(
                            FetchOutcome::Failed("Usage fetch timed out".into()),
                            now,
                            okena_agent_usage::now_epoch(),
                        );
                        slot.next_due = Some(now + delay);
                    }
                }
                FetchEvent::Landed { index, id, outcome } => {
                    let Some(slot) = slots.get_mut(index) else { continue };
                    let Some(running) = slot.running.take_if(|running| running.id == id) else {
                        continue;
                    };
                    let provider = slot.tracker.entry().provider;
                    if running.generation != slot.generation || running.timed_out {
                        log::debug!("[usage] discarding stale {} fetch result", provider.as_str());
                        continue;
                    }
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

    /// A hub with one client connected and no pool, as with routing off.
    fn watched_hub(
        settings: &Arc<Mutex<AppSettings>>,
    ) -> (
        UsageHub,
        mpsc::UnboundedReceiver<UsageRefresh>,
        okena_remote_server::usage::UsageClient,
    ) {
        let (hub, refresh_rx) = UsageHub::new(initial_usage(&settings.lock()));
        hub.set_pool(None);
        let client = hub.register_client();
        (hub, refresh_rx, client)
    }

    fn counting_fetcher() -> (Arc<AtomicUsize>, UsageFetcher) {
        let calls = Arc::new(AtomicUsize::new(0));
        let fetcher: UsageFetcher = {
            let calls = calls.clone();
            Arc::new(move |_, _| {
                calls.fetch_add(1, Ordering::SeqCst);
                ok_outcome()
            })
        };
        (calls, fetcher)
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
        assert_eq!(usage.providers.len(), 3);
        assert_eq!(usage.providers[0].status, ApiUsageStatus::Disabled);
        assert_eq!(usage.providers[1].status, ApiUsageStatus::Pending);
        assert_eq!(usage.providers[2].provider, ApiUsageProvider::Grok);
        assert_eq!(usage.providers[2].status, ApiUsageStatus::Disabled);
        assert_eq!(usage.working_days, vec![0, 1, 2, 3, 4]);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn grok_toggle_starts_and_stops_polling() {
        let settings = settings_with(&[]);
        let (hub, refresh_rx, _client) = watched_hub(&settings);
        let calls = Arc::new(AtomicUsize::new(0));
        let fetcher: UsageFetcher = {
            let calls = calls.clone();
            Arc::new(move |provider, dir| {
                assert_eq!(provider, ApiUsageProvider::Grok, "only grok is enabled");
                assert!(dir.is_none(), "only claude gets a config dir");
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
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(calls.load(Ordering::SeqCst), 0);

        settings
            .lock()
            .enabled_extensions
            .insert(GROK_EXTENSION_ID.to_string());
        let usage = wait_for(&hub, |u| u.providers[2].status == ApiUsageStatus::Ok).await;
        assert_eq!(usage.providers[2].windows[0].used_percent, 42.0);
        assert_eq!(usage.providers[0].status, ApiUsageStatus::Disabled);
        assert_eq!(usage.providers[1].status, ApiUsageStatus::Disabled);
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        settings.lock().enabled_extensions.clear();
        wait_for(&hub, |u| u.providers[2].status == ApiUsageStatus::Disabled).await;
        task.abort();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn polls_enabled_provider_throttles_refresh_and_stops_when_disabled() {
        let settings = settings_with(&["claude-code"]);
        let (hub, refresh_rx, _client) = watched_hub(&settings);
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

        // A refresh right after a fetch is throttled (60 s), and the answer
        // says so: nothing is fetching.
        let answer = hub.refresh(None).await;
        assert!(answer.providers.iter().all(|p| !p.fetching));
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
        let (hub, refresh_rx, _client) = watched_hub(&settings);
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

    /// A fetcher whose calls block until released; records the peak number
    /// of concurrent calls.
    struct GatedFetcher {
        calls: AtomicUsize,
        active: AtomicUsize,
        peak: AtomicUsize,
        gate: std::sync::Mutex<std::sync::mpsc::Receiver<FetchOutcome>>,
    }

    fn gated() -> (Arc<GatedFetcher>, std::sync::mpsc::Sender<FetchOutcome>, UsageFetcher) {
        let (tx, rx) = std::sync::mpsc::channel();
        let state = Arc::new(GatedFetcher {
            calls: AtomicUsize::new(0),
            active: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
            gate: std::sync::Mutex::new(rx),
        });
        let fetcher: UsageFetcher = {
            let state = state.clone();
            Arc::new(move |_, _| {
                state.calls.fetch_add(1, Ordering::SeqCst);
                let active = state.active.fetch_add(1, Ordering::SeqCst) + 1;
                state.peak.fetch_max(active, Ordering::SeqCst);
                let outcome = state
                    .gate
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(30))
                    .unwrap_or_else(|_| FetchOutcome::Failed("gate closed".into()));
                state.active.fetch_sub(1, Ordering::SeqCst);
                outcome
            })
        };
        (state, tx, fetcher)
    }

    async fn wait_until(mut pred: impl FnMut() -> bool) {
        for _ in 0..300 {
            if pred() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("condition not reached");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn config_change_waits_for_the_running_fetch_and_discards_it() {
        let settings = settings_with(&["codex"]);
        let (hub, refresh_rx, _client) = watched_hub(&settings);
        let (state, release, fetcher) = gated();
        let task = tokio::spawn(run_usage_poll_with(
            settings.clone(),
            hub.clone(),
            refresh_rx,
            fetcher,
        ));
        wait_until(|| state.calls.load(Ordering::SeqCst) == 1).await;

        // Toggle Codex off and on while the first fetch is still running.
        settings.lock().enabled_extensions.clear();
        wait_for(&hub, |u| u.providers[1].status == ApiUsageStatus::Disabled).await;
        settings
            .lock()
            .enabled_extensions
            .insert(CODEX_EXTENSION_ID.to_string());
        let usage = wait_for(&hub, |u| u.providers[1].status == ApiUsageStatus::Pending).await;
        assert!(!usage.providers[1].fetching);
        // Neither a refresh nor the new config starts a second fetch yet.
        let answer = hub.refresh(Some(ApiUsageProvider::Codex)).await;
        assert!(!answer.providers[1].fetching);
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(state.calls.load(Ordering::SeqCst), 1);

        // The old fetch lands: its result is dropped, then the new one runs.
        release
            .send(FetchOutcome::Failed("stale result".into()))
            .unwrap();
        wait_until(|| state.calls.load(Ordering::SeqCst) == 2).await;
        release.send(ok_outcome()).unwrap();
        let usage = wait_for(&hub, |u| u.providers[1].status == ApiUsageStatus::Ok).await;
        assert_eq!(usage.providers[1].consecutive_failures, 0);
        assert_eq!(state.peak.load(Ordering::SeqCst), 1, "fetches overlapped");
        task.abort();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn hung_fetch_becomes_an_error_without_overlapping() {
        let settings = settings_with(&["codex"]);
        let (hub, refresh_rx, _client) = watched_hub(&settings);
        let (state, release, fetcher) = gated();
        let task = tokio::spawn(run_usage_poll_inner(
            settings,
            hub.clone(),
            refresh_rx,
            fetcher,
            Duration::from_millis(200),
        ));
        let usage = wait_for(&hub, |u| u.providers[1].status == ApiUsageStatus::Error).await;
        let codex = &usage.providers[1];
        assert!(!codex.fetching);
        assert_eq!(codex.error.as_deref(), Some("Usage fetch timed out"));
        assert_eq!(codex.consecutive_failures, 1);

        // The late result is discarded and no second fetch overlapped it.
        release.send(ok_outcome()).unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(hub.snapshot().providers[1].status, ApiUsageStatus::Error);
        assert_eq!(state.peak.load(Ordering::SeqCst), 1);
        task.abort();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn polls_only_while_a_client_is_connected_but_serves_refreshes() {
        let settings = settings_with(&["codex"]);
        let (hub, refresh_rx) = UsageHub::new(initial_usage(&settings.lock()));
        let (calls, fetcher) = counting_fetcher();
        let task = tokio::spawn(run_usage_poll_with(
            settings,
            hub.clone(),
            refresh_rx,
            fetcher,
        ));
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(calls.load(Ordering::SeqCst), 0, "polled with no client");

        // A refresh (e.g. `okena usage` after a POST) runs without a client.
        let answer = hub.refresh(Some(ApiUsageProvider::Codex)).await;
        assert!(answer.providers[1].fetching);
        wait_for(&hub, |u| u.providers[1].status == ApiUsageStatus::Ok).await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        task.abort();

        // The first client starts the fetch that fell due meanwhile.
        let settings = settings_with(&["codex"]);
        let (hub, refresh_rx) = UsageHub::new(initial_usage(&settings.lock()));
        let (calls, fetcher) = counting_fetcher();
        let task = tokio::spawn(run_usage_poll_with(
            settings,
            hub.clone(),
            refresh_rx,
            fetcher,
        ));
        tokio::time::sleep(Duration::from_millis(300)).await;
        let _client = hub.register_client();
        wait_for(&hub, |u| u.providers[1].status == ApiUsageStatus::Ok).await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        task.abort();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_login_is_not_polled_while_a_pool_is_published() {
        let settings = settings_with(&["claude-code"]);
        let (hub, refresh_rx) = UsageHub::new(initial_usage(&settings.lock()));
        let _client = hub.register_client();
        let (calls, fetcher) = counting_fetcher();
        let task = tokio::spawn(run_usage_poll_with(
            settings,
            hub.clone(),
            refresh_rx,
            fetcher,
        ));
        // Before the accounts poll reported, the login waits for it.
        tokio::time::sleep(Duration::from_millis(300)).await;
        hub.set_pool(okena_agent_usage::pool_summary::summarize_now(
            &[pool_account()],
            &std::collections::HashMap::new(),
            WorkingDays::all(),
        ));
        let answer = hub.refresh(Some(ApiUsageProvider::Claude)).await;
        assert!(!answer.providers[0].fetching);
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(calls.load(Ordering::SeqCst), 0, "polled the login");

        // Without the pool, the login's due fetch runs.
        hub.set_pool(None);
        wait_for(&hub, |u| u.providers[0].status == ApiUsageStatus::Ok).await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        task.abort();
    }

    fn pool_account() -> okena_core::ai_proxy::AiAccount {
        okena_core::ai_proxy::AiAccount {
            id: "a".into(),
            label: "A".into(),
            email: None,
            plan: None,
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
}
