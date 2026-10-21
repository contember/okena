//! Daemon side of the AI proxy (`okena-ai`): environment for new shells,
//! starting the proxy process, forwarding `ActionRequest::AiProxy`, and
//! publishing the pool's accounts and its summary to clients.

use crate::claude_remote::PaneEnv;
use okena_agent_usage::history::History;
use okena_agent_usage::pace::{USAGE_SETTINGS_KEY, WorkingDays};
use okena_agent_usage::pool;
use okena_agent_usage::pool_summary;
use okena_ai::client::ProxyClient;
use okena_core::ai_proxy::{
    AiAccount, AiProxyCommand, AiProxyState, AiProxyStatus, ProxyEndpoint, ensure_endpoint,
    ensure_management_key, load_endpoint,
};
use okena_core::usage_history::Provider;
use okena_remote_server::usage::UsageHub;
use okena_terminal::backend::{LaunchEnvironment, TerminalBackend};
use okena_workspace::persistence::AppSettings;
use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Notify, watch};
use tokio::time::Instant;

/// How often the published accounts follow the proxy's usage and cooldowns.
const ACCOUNTS_POLL_INTERVAL: Duration = Duration::from_secs(15);
/// How long an account's usage history serves the pool summary before it is
/// read again.
const HISTORY_RELOAD_INTERVAL: Duration = Duration::from_secs(60);
/// Cap on one account's history read, so a proxy that hangs on it does not
/// hold up the accounts poll for the client's minute-long command timeout.
const HISTORY_READ_TIMEOUT: Duration = Duration::from_secs(10);
/// Longest wait between attempts to start a proxy that keeps failing to come up.
const MAX_RESTART_DELAY: Duration = Duration::from_secs(10 * 60);

fn profile_root() -> Option<PathBuf> {
    okena_core::profiles::try_current().map(|profile| profile.root.clone())
}

/// Environment for newly spawned PTYs: `shared` (the same for every terminal,
/// see [`PaneEnv::launch_environment`]) and, with the proxy enabled,
/// `ANTHROPIC_BASE_URL` naming the terminal's project, so the proxy applies
/// that project's routing policy. Shells that already run keep the environment
/// they started with.
pub(crate) fn launch_environment(
    settings: &AppSettings,
    shared: Vec<(String, Option<String>)>,
) -> LaunchEnvironment {
    let endpoint = if settings.ai_proxy_enabled
        && let Some(root) = profile_root()
    {
        ensure_endpoint(&root)
            .inspect_err(|error| log::error!("[ai-proxy] no endpoint for new shells: {error:#}"))
            .ok()
    } else {
        None
    };
    Arc::new(move |project_id| {
        let mut env = shared.clone();
        if let Some(endpoint) = &endpoint {
            let url = project_id.map_or_else(
                || endpoint.base_url(),
                |project_id| endpoint.project_url(project_id),
            );
            env.push(("ANTHROPIC_BASE_URL".into(), Some(url)));
        }
        env
    })
}

async fn connect() -> anyhow::Result<ProxyClient> {
    let root = profile_root().ok_or_else(|| anyhow::anyhow!("No active profile"))?;
    let exe = okena_remote_server::local::remember_current_executable()?;
    okena_ai::client::ensure_running(&root, &exe).await
}

/// Start the proxy when it is enabled, then refresh the shell environment in
/// case the proxy had to move to another port.
pub(crate) fn start_if_enabled(
    settings: &Arc<Mutex<AppSettings>>,
    backend: &Arc<dyn TerminalBackend>,
    pane_env: &Arc<PaneEnv>,
    runtime: &tokio::runtime::Handle,
) {
    if !settings.lock().ai_proxy_enabled {
        return;
    }
    let settings = settings.clone();
    let backend = backend.clone();
    let pane_env = pane_env.clone();
    runtime.spawn(async move {
        match connect().await {
            Ok(_) => backend.set_launch_environment(pane_env.launch_environment(&settings.lock())),
            Err(error) => log::error!("[ai-proxy] failed to start: {error:#}"),
        }
    });
}

/// Run one management command against the proxy, starting it if needed.
/// Management works whether or not new shells are routed through the proxy.
pub(crate) async fn execute(command: AiProxyCommand) -> AiProxyState {
    let client = match connect().await {
        Ok(client) => client,
        Err(error) => {
            return AiProxyState {
                status: None,
                failure: Some(format!("{error:#}")),
            };
        }
    };
    match client.command(&command).await {
        Ok(status) => AiProxyState {
            status: Some(status),
            failure: None,
        },
        // A failed command (e.g. a mistyped login code) still shows the pool.
        Err(error) => AiProxyState {
            status: client.command(&AiProxyCommand::Status).await.ok(),
            failure: Some(format!("{error:#}")),
        },
    }
}

/// The pool's accounts as clients see them in `StateResponse::ai_accounts`.
#[derive(Clone)]
pub struct AiProxyAccounts {
    accounts: Arc<Mutex<Vec<AiAccount>>>,
    state_version: watch::Sender<u64>,
    refresh: Arc<Notify>,
}

impl AiProxyAccounts {
    pub(crate) fn new(state_version: watch::Sender<u64>) -> Self {
        Self {
            accounts: Arc::new(Mutex::new(Vec::new())),
            state_version,
            refresh: Arc::new(Notify::new()),
        }
    }

    pub(crate) fn snapshot(&self) -> Vec<AiAccount> {
        self.accounts.lock().clone()
    }

    /// Re-read the accounts now instead of at the next poll.
    pub(crate) fn refresh(&self) {
        self.refresh.notify_one();
    }

    fn publish(&self, accounts: Vec<AiAccount>) {
        let mut published = self.accounts.lock();
        if *published != accounts {
            *published = accounts;
            self.state_version
                .send_modify(|version| *version = version.wrapping_add(1));
        }
    }
}

/// Keep [`AiProxyAccounts`] and the pool summary in `usage` in step with the
/// proxy while new shells are routed through it. New shells get its URL
/// whether or not it listens, so a proxy that died is started again, and a
/// port it moved to reaches new shells. With routing off, or the proxy
/// unreachable, no accounts and no pool are published: the `claude` login's
/// own usage is then the relevant one.
pub(crate) async fn run_accounts_poll(
    accounts: AiProxyAccounts,
    settings: Arc<Mutex<AppSettings>>,
    backend: Arc<dyn TerminalBackend>,
    pane_env: Arc<PaneEnv>,
    usage: UsageHub,
) {
    let mut routed_endpoint: Option<ProxyEndpoint> = None;
    let mut restart = RestartBackoff::new();
    let mut histories = PoolHistories::default();
    loop {
        let enabled = settings.lock().ai_proxy_enabled;
        let read = if enabled {
            match read_or_restart(&mut restart).await {
                Ok((client, current)) => {
                    if routed_endpoint.as_ref() != Some(client.endpoint()) {
                        backend.set_launch_environment(pane_env.launch_environment(&settings.lock()));
                        routed_endpoint = Some(client.endpoint().clone());
                    }
                    Some((client, current))
                }
                Err(error) => {
                    log::warn!("[ai-proxy] proxy unreachable: {error:#}");
                    None
                }
            }
        } else {
            routed_endpoint = None;
            None
        };
        let Some((client, current)) = read else {
            accounts.publish(Vec::new());
            usage.set_pool(None);
            wait_for_next_poll(&accounts).await;
            continue;
        };
        accounts.publish(current.clone());
        // A refresh cuts the history reads short and polls again at once; the
        // pool waits for that pass, so it never shows an account without the
        // history that is still being read.
        let refresh_requested = tokio::select! {
            () = histories.reload_due(&client, &current) => false,
            () = accounts.refresh.notified() => true,
        };
        if refresh_requested {
            continue;
        }
        let pool_histories = histories.by_account().clone();
        let working =
            WorkingDays::from_value(settings.lock().extension_settings.get(USAGE_SETTINGS_KEY));
        // Off the LocalSet, which also drives the PTY loop.
        let summary = tokio::task::spawn_blocking(move || {
            pool_summary::summarize_now(&current, &pool_histories, working)
        })
        .await;
        match summary {
            Ok(summary) => {
                usage.set_pool(summary);
            }
            Err(error) => log::error!("[ai-proxy] pool summary failed: {error}"),
        }
        wait_for_next_poll(&accounts).await;
    }
}

async fn wait_for_next_poll(accounts: &AiProxyAccounts) {
    tokio::select! {
        _ = tokio::time::sleep(ACCOUNTS_POLL_INTERVAL) => {}
        _ = accounts.refresh.notified() => {}
    }
}

/// Read the running proxy's accounts; when it does not answer, start it, at
/// most as often as `restart` allows.
async fn read_or_restart(
    restart: &mut RestartBackoff,
) -> anyhow::Result<(ProxyClient, Vec<AiAccount>)> {
    match read_accounts().await {
        Ok(read) => {
            restart.reset();
            return Ok(read);
        }
        Err(error) if !restart.due() => return Err(error),
        Err(error) => log::warn!("[ai-proxy] proxy unreachable, starting it: {error:#}"),
    }
    restart.back_off();
    connect().await?;
    read_accounts().await
}

/// Ask a running proxy for its accounts without starting one.
async fn read_accounts() -> anyhow::Result<(ProxyClient, Vec<AiAccount>)> {
    let root = profile_root().ok_or_else(|| anyhow::anyhow!("No active profile"))?;
    let endpoint = load_endpoint(&root)?.ok_or_else(|| anyhow::anyhow!("No proxy endpoint"))?;
    let client = ProxyClient::new(endpoint, ensure_management_key(&root)?)?;
    let accounts = client.command(&AiProxyCommand::Status).await?.accounts;
    Ok((client, accounts))
}

/// Usage history of the in-service pool accounts, read from the proxy at most
/// once per [`HISTORY_RELOAD_INTERVAL`] per account. A failed read keeps the
/// history read before.
#[derive(Default)]
struct PoolHistories {
    histories: HashMap<String, History>,
    attempted_at: HashMap<String, Instant>,
    /// Accounts whose last read failed, so a lasting failure warns once.
    failing: HashSet<String>,
}

impl PoolHistories {
    fn by_account(&self) -> &HashMap<String, History> {
        &self.histories
    }

    async fn reload_due(&mut self, client: &ProxyClient, accounts: &[AiAccount]) {
        for account_id in self.due(accounts, Instant::now()) {
            let command = AiProxyCommand::UsageHistory {
                account_id: account_id.clone(),
            };
            let read = client.command(&command);
            let result = match tokio::time::timeout(HISTORY_READ_TIMEOUT, read).await {
                Ok(status) => status.and_then(|status| account_history(status, &account_id)),
                Err(_) => Err(anyhow::anyhow!("Usage history read timed out")),
            };
            self.record(account_id, result, Instant::now());
        }
    }

    /// Forget accounts that left the pool; the ids whose history is due.
    fn due(&mut self, accounts: &[AiAccount], now: Instant) -> Vec<String> {
        let in_service: Vec<&str> = accounts
            .iter()
            .filter(|account| pool::in_service(account))
            .map(|account| account.id.as_str())
            .collect();
        self.histories
            .retain(|id, _| in_service.contains(&id.as_str()));
        self.attempted_at
            .retain(|id, _| in_service.contains(&id.as_str()));
        self.failing.retain(|id| in_service.contains(&id.as_str()));
        in_service
            .into_iter()
            .filter(|id| {
                self.attempted_at
                    .get(*id)
                    .is_none_or(|at| now.duration_since(*at) >= HISTORY_RELOAD_INTERVAL)
            })
            .map(String::from)
            .collect()
    }

    fn record(&mut self, account_id: String, result: anyhow::Result<History>, now: Instant) {
        match result {
            Ok(history) => {
                if self.failing.remove(&account_id) {
                    log::info!("[ai-proxy] usage history of {account_id} available again");
                }
                self.histories.insert(account_id.clone(), history);
            }
            Err(error) if self.failing.insert(account_id.clone()) => {
                log::warn!("[ai-proxy] usage history of {account_id} unavailable: {error:#}");
            }
            Err(error) => {
                log::debug!(
                    "[ai-proxy] usage history of {account_id} still unavailable: {error:#}"
                );
            }
        }
        self.attempted_at.insert(account_id, now);
    }
}

/// The proxy's answer to `UsageHistory`, accepted only when it holds Claude
/// samples of the requested account.
fn account_history(status: AiProxyStatus, account_id: &str) -> anyhow::Result<History> {
    let history = status
        .usage_history
        .ok_or_else(|| anyhow::anyhow!("Usage history is unavailable from this proxy"))?;
    if history.account_id != account_id
        || history
            .samples
            .iter()
            .any(|sample| sample.source != account_id || sample.provider != Provider::Claude)
    {
        anyhow::bail!("Usage history belongs to another account");
    }
    Ok(History::from_samples(history.samples))
}

/// Doubling delay between start attempts, so a proxy that exits at once (e.g.
/// on a corrupt `accounts.json`) is not respawned on every poll.
struct RestartBackoff {
    next_attempt: tokio::time::Instant,
    delay: Duration,
}

impl RestartBackoff {
    fn new() -> Self {
        Self {
            next_attempt: tokio::time::Instant::now(),
            delay: ACCOUNTS_POLL_INTERVAL,
        }
    }

    fn due(&self) -> bool {
        tokio::time::Instant::now() >= self.next_attempt
    }

    fn back_off(&mut self) {
        self.next_attempt = tokio::time::Instant::now() + self.delay;
        self.delay = (self.delay * 2).min(MAX_RESTART_DELAY);
    }

    fn reset(&mut self) {
        *self = Self::new();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::StubBackend;
    use okena_core::ai_proxy::AiAccountHistory;
    use okena_core::api::{ApiProviderUsage, ApiUsage, ApiUsageProvider, ApiUsageStatus};
    use okena_core::usage_history::{LimitSample, UsageSample};

    fn account(id: &str) -> AiAccount {
        AiAccount {
            id: id.into(),
            label: id.into(),
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

    fn sample(source: &str) -> UsageSample {
        UsageSample {
            version: 1,
            recorded_at: 1500.0,
            provider: Provider::Claude,
            source: source.into(),
            plan: None,
            limits: vec![LimitSample {
                name: "5h".into(),
                used_percent: 23.0,
                window_seconds: 18000.0,
                reset_at: Some(19000.0),
            }],
        }
    }

    fn status(history: Option<AiAccountHistory>) -> AiProxyStatus {
        AiProxyStatus {
            usage_history: history,
            ..serde_json::from_value(serde_json::json!({
                "version": "test", "inflight": 0, "accounts": [], "logins": []
            }))
            .unwrap()
        }
    }

    fn history_of(account_id: &str, samples: Vec<UsageSample>) -> Option<AiAccountHistory> {
        Some(AiAccountHistory {
            account_id: account_id.into(),
            samples,
        })
    }

    #[test]
    fn history_keeps_account_boundaries() {
        let history = account_history(status(history_of("a", vec![sample("a")])), "a").unwrap();
        assert_eq!(
            history.points("5h", 19000.0, 18000.0, 1600.0)[0].used_percent,
            23.0
        );
        assert!(account_history(status(history_of("a", vec![sample("a")])), "b").is_err());
        assert!(account_history(status(history_of("a", vec![sample("b")])), "a").is_err());
        let mut codex = sample("a");
        codex.provider = Provider::Codex;
        assert!(account_history(status(history_of("a", vec![codex])), "a").is_err());
        assert!(account_history(status(None), "a").is_err());
    }

    #[test]
    fn histories_reload_once_a_minute_and_forget_accounts_that_left() {
        let mut histories = PoolHistories::default();
        let start = Instant::now();
        let disabled = AiAccount {
            enabled: false,
            ..account("off")
        };
        let accounts = [account("a"), account("b"), disabled];
        assert_eq!(histories.due(&accounts, start), ["a", "b"]);
        histories.record(
            "a".into(),
            Ok(History::from_samples(vec![sample("a")])),
            start,
        );
        histories.record("b".into(), Err(anyhow::anyhow!("offline")), start);
        assert!(
            histories
                .due(&accounts, start + Duration::from_secs(59))
                .is_empty()
        );

        // A failed reload keeps the history read before.
        let later = start + HISTORY_RELOAD_INTERVAL;
        assert_eq!(histories.due(&accounts, later), ["a", "b"]);
        histories.record("a".into(), Err(anyhow::anyhow!("offline")), later);
        assert_eq!(histories.by_account()["a"].samples().len(), 1);
        assert!(!histories.by_account().contains_key("b"));

        assert_eq!(histories.due(&[account("b")], later), ["b"]);
        assert!(histories.by_account().is_empty());
    }

    #[tokio::test]
    async fn routing_off_publishes_no_pool() {
        let (hub, _refresh_rx) = UsageHub::new(ApiUsage {
            providers: vec![ApiProviderUsage::empty(
                ApiUsageProvider::Claude,
                ApiUsageStatus::Ok,
            )],
            ..ApiUsage::default()
        });
        let claude_pool = |hub: &UsageHub| {
            hub.snapshot()
                .provider(ApiUsageProvider::Claude)
                .and_then(|entry| entry.pool.clone())
        };
        hub.set_pool(pool_summary::summarize_now(
            &[account("a")],
            &HashMap::new(),
            WorkingDays::all(),
        ));
        assert!(claude_pool(&hub).is_some());
        let mut changes = hub.subscribe();
        let mut routing_off = AppSettings::default();
        routing_off.ai_proxy_enabled = false;
        let settings = Arc::new(Mutex::new(routing_off));
        let accounts = AiProxyAccounts::new(watch::channel(0).0);
        let poll = run_accounts_poll(
            accounts,
            settings,
            Arc::new(StubBackend),
            Arc::new(PaneEnv::resolve()),
            hub.clone(),
        );
        tokio::time::timeout(Duration::from_secs(5), async {
            tokio::select! {
                () = poll => unreachable!("the poll runs forever"),
                changed = changes.changed() => changed.unwrap(),
            }
        })
        .await
        .unwrap();
        assert_eq!(claude_pool(&hub), None);
    }
}
