//! Account pool: persisted credentials, runtime quota state and account selection.

use crate::now_ms;
use crate::oauth::{self, Pkce, RefreshError, Tokens, Upstream};
use okena_core::ai_proxy::{
    AiAccount, AiAccountRule, AiActivity, AiPendingLogin, AiProxyStatus, AiRouting,
    AiRoutingPolicy, AiUsage, is_routable_project_id, write_private,
};
use parking_lot::Mutex;
use reqwest::header::HeaderMap;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// How long a conversation keeps preferring the account that served it last.
const AFFINITY_TTL_MS: i64 = 6 * 60 * 60 * 1000;
const LOGIN_TTL_MS: i64 = 30 * 60 * 1000;
const DEFAULT_COOLDOWN_MS: i64 = 60 * 1000;

#[derive(Clone, Serialize, Deserialize)]
struct StoredAccount {
    id: String,
    label: String,
    email: Option<String>,
    account_uuid: Option<String>,
    plan: Option<String>,
    enabled: bool,
    needs_login: bool,
    tokens: Tokens,
}

#[derive(Default, Serialize, Deserialize)]
struct StoredAccounts {
    accounts: Vec<StoredAccount>,
}

#[derive(Default)]
struct Runtime {
    cooldown_until_ms: Option<i64>,
    five_hour: Option<AiUsage>,
    seven_day: Option<AiUsage>,
    requests: u64,
    last_used_ms: Option<i64>,
}

impl Runtime {
    fn blocked_until(&self, now: i64) -> Option<i64> {
        let quota_resets = [self.five_hour.as_ref(), self.seven_day.as_ref()]
            .into_iter()
            .flatten()
            .filter(|usage| usage.is_exhausted(now))
            .map(|usage| usage.resets_at_ms.unwrap_or(now + DEFAULT_COOLDOWN_MS));
        self.cooldown_until_ms
            .filter(|until| *until > now)
            .into_iter()
            .chain(quota_resets)
            .max()
    }
}

struct Account {
    stored: StoredAccount,
    runtime: Runtime,
    /// Serializes refreshes: a refresh token is single-use.
    refresh: Arc<tokio::sync::Mutex<()>>,
}

impl Account {
    fn available(&self, now: i64) -> bool {
        self.stored.enabled && !self.stored.needs_login && self.runtime.blocked_until(now).is_none()
    }
}

struct Login {
    id: String,
    label: String,
    pkce: Pkce,
    authorize_url: String,
    error: Option<String>,
    created_ms: i64,
}

#[derive(Default)]
struct State {
    accounts: Vec<Account>,
    logins: Vec<Login>,
    /// Conversation (`x-claude-code-session-id`) → account id, last used.
    affinity: HashMap<String, (String, i64)>,
    routing: AiRouting,
}

impl State {
    fn account(&mut self, id: &str) -> anyhow::Result<&mut Account> {
        self.accounts
            .iter_mut()
            .find(|account| account.stored.id == id)
            .ok_or_else(|| anyhow::anyhow!("Unknown account"))
    }

    /// Drop rules for accounts that are not in the pool (e.g. removed meanwhile).
    fn known_accounts_only(&self, mut policy: AiRoutingPolicy) -> AiRoutingPolicy {
        policy
            .accounts
            .retain(|id, _| self.accounts.iter().any(|account| &account.stored.id == id));
        policy
    }
}

/// Read a JSON file the proxy owns; a missing file is the default value.
fn read_json<T: serde::de::DeserializeOwned + Default>(path: &Path) -> anyhow::Result<T> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|_| anyhow::anyhow!("{} is corrupt", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
        Err(error) => Err(error.into()),
    }
}

pub struct Accounts {
    path: PathBuf,
    routing_path: PathBuf,
    http: reqwest::Client,
    upstream: Upstream,
    state: Mutex<State>,
}

/// Account picked for one upstream attempt.
pub struct Selected {
    pub id: String,
    pub access_token: String,
    pub account_uuid: Option<String>,
}

pub enum Selection {
    Selected(Selected),
    /// The pool has no enabled, logged-in account: callers use their own credentials.
    NoAccounts,
    /// The policy forbids an account and no account it permits is enabled and
    /// logged in. Never served with the caller's own credentials.
    NoPermittedAccount,
    /// No usable account; the earliest cooldown end, if any account is cooling down.
    Exhausted {
        retry_at_ms: Option<i64>,
    },
}

impl Accounts {
    /// Load `accounts.json` and `routing.json` from the proxy directory.
    pub fn open(dir: &Path, http: reqwest::Client, upstream: Upstream) -> anyhow::Result<Self> {
        let path = dir.join("accounts.json");
        let routing_path = dir.join("routing.json");
        // A corrupt file is an error, never an empty pool or an open policy: one
        // holds the only copy of the refresh tokens, the other forbids accounts.
        let stored: StoredAccounts = read_json(&path)?;
        let routing: AiRouting = read_json(&routing_path)?;
        let accounts = stored
            .accounts
            .into_iter()
            .map(|stored| Account {
                stored,
                runtime: Runtime::default(),
                refresh: Arc::default(),
            })
            .collect();
        Ok(Self {
            path,
            routing_path,
            http,
            upstream,
            state: Mutex::new(State {
                accounts,
                routing,
                ..State::default()
            }),
        })
    }

    fn persist(&self, state: &State) -> anyhow::Result<()> {
        let stored = StoredAccounts {
            accounts: state
                .accounts
                .iter()
                .map(|account| account.stored.clone())
                .collect(),
        };
        write_private(&self.path, &serde_json::to_vec_pretty(&stored)?)
    }

    fn persist_routing(&self, state: &State) -> anyhow::Result<()> {
        write_private(
            &self.routing_path,
            &serde_json::to_vec_pretty(&state.routing)?,
        )
    }

    pub fn status(&self, inflight: usize, activity: AiActivity) -> AiProxyStatus {
        let mut state = self.state.lock();
        let now = now_ms();
        state
            .logins
            .retain(|login| now - login.created_ms < LOGIN_TTL_MS);
        AiProxyStatus {
            version: crate::VERSION.to_owned(),
            inflight,
            accounts: state
                .accounts
                .iter()
                .map(|account| AiAccount {
                    id: account.stored.id.clone(),
                    label: account.stored.label.clone(),
                    email: account.stored.email.clone(),
                    plan: account.stored.plan.clone(),
                    enabled: account.stored.enabled,
                    needs_login: account.stored.needs_login,
                    cooldown_until_ms: account
                        .runtime
                        .cooldown_until_ms
                        .filter(|until| *until > now),
                    five_hour: account.runtime.five_hour.clone(),
                    seven_day: account.runtime.seven_day.clone(),
                    requests: account.runtime.requests,
                    last_used_ms: account.runtime.last_used_ms,
                })
                .collect(),
            logins: state
                .logins
                .iter()
                .map(|login| AiPendingLogin {
                    id: login.id.clone(),
                    label: login.label.clone(),
                    authorize_url: login.authorize_url.clone(),
                    error: login.error.clone(),
                })
                .collect(),
            routing: state.routing.clone(),
            activity,
        }
    }

    pub fn start_login(&self, label: String) -> anyhow::Result<()> {
        let label = label.trim().to_owned();
        anyhow::ensure!(!label.is_empty(), "Account label is required");
        let pkce = Pkce::new()?;
        let login = Login {
            id: okena_core::ai_proxy::random_token()?,
            label,
            authorize_url: pkce.authorize_url()?,
            pkce,
            error: None,
            created_ms: now_ms(),
        };
        self.state.lock().logins.push(login);
        Ok(())
    }

    pub fn cancel_login(&self, login_id: &str) {
        self.state
            .lock()
            .logins
            .retain(|login| login.id != login_id);
    }

    pub async fn finish_login(&self, login_id: &str, code: &str) -> anyhow::Result<()> {
        let (pkce, label) = {
            let state = self.state.lock();
            let login = state
                .logins
                .iter()
                .find(|login| login.id == login_id)
                .ok_or_else(|| anyhow::anyhow!("Login expired; start it again"))?;
            (
                Pkce {
                    verifier: login.pkce.verifier.clone(),
                    state: login.pkce.state.clone(),
                },
                login.label.clone(),
            )
        };
        let result = match oauth::exchange_code(&self.http, &self.upstream, &pkce, code).await {
            Ok(result) => result,
            Err(error) => {
                if let Some(login) = self
                    .state
                    .lock()
                    .logins
                    .iter_mut()
                    .find(|login| login.id == login_id)
                {
                    login.error = Some(error.to_string());
                }
                return Err(error);
            }
        };
        let plan = oauth::fetch_plan(&self.http, &self.upstream, &result.tokens.access_token).await;

        let mut state = self.state.lock();
        state.logins.retain(|login| login.id != login_id);
        // Logging the same provider account in again replaces its credentials.
        let existing = result.account_uuid.as_ref().and_then(|uuid| {
            state
                .accounts
                .iter_mut()
                .find(|account| account.stored.account_uuid.as_ref() == Some(uuid))
        });
        if let Some(account) = existing {
            account.stored.tokens = result.tokens;
            account.stored.needs_login = false;
            account.stored.email = result.email.or(account.stored.email.take());
            account.stored.plan = plan.or(account.stored.plan.take());
            account.runtime.cooldown_until_ms = None;
        } else {
            state.accounts.push(Account {
                stored: StoredAccount {
                    id: okena_core::ai_proxy::random_token()?,
                    label,
                    email: result.email,
                    account_uuid: result.account_uuid,
                    plan,
                    enabled: true,
                    needs_login: false,
                    tokens: result.tokens,
                },
                runtime: Runtime::default(),
                refresh: Arc::default(),
            });
        }
        self.persist(&state)
    }

    pub fn remove(&self, account_id: &str) -> anyhow::Result<()> {
        let mut state = self.state.lock();
        state.account(account_id)?;
        state
            .accounts
            .retain(|account| account.stored.id != account_id);
        state.affinity.retain(|_, (id, _)| id != account_id);
        self.persist(&state)?;
        let routing = &mut state.routing;
        for policy in std::iter::once(&mut routing.default).chain(routing.projects.values_mut()) {
            policy.accounts.remove(account_id);
        }
        self.persist_routing(&state)
    }

    pub fn set_enabled(&self, account_id: &str, enabled: bool) -> anyhow::Result<()> {
        let mut state = self.state.lock();
        state.account(account_id)?.stored.enabled = enabled;
        self.persist(&state)
    }

    pub fn set_default_policy(&self, policy: AiRoutingPolicy) -> anyhow::Result<()> {
        let mut state = self.state.lock();
        state.routing.default = state.known_accounts_only(policy);
        self.persist_routing(&state)
    }

    pub fn set_project_policy(
        &self,
        project_id: &str,
        policy: Option<AiRoutingPolicy>,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(is_routable_project_id(project_id), "Invalid project id");
        let mut state = self.state.lock();
        match policy {
            Some(policy) => {
                let policy = state.known_accounts_only(policy);
                state.routing.projects.insert(project_id.to_owned(), policy);
            }
            None => {
                state.routing.projects.remove(project_id);
            }
        }
        self.persist_routing(&state)
    }

    /// Pick an account the project's policy permits: the conversation's previous
    /// account while it is usable, otherwise a preferred one before an allowed
    /// one, the least-used first.
    pub async fn select(
        &self,
        project: Option<&str>,
        session: Option<&str>,
        exclude: &[String],
    ) -> Selection {
        let mut excluded = exclude.to_vec();
        loop {
            let candidate = {
                let state = self.state.lock();
                let now = now_ms();
                let policy = state.routing.policy(project);
                let permitted =
                    |account: &&Account| policy.rule(&account.stored.id) != AiAccountRule::Forbid;
                let preferred =
                    |account: &Account| policy.rule(&account.stored.id) == AiAccountRule::Prefer;
                let usable = |account: &&Account| {
                    account.available(now)
                        && !excluded.contains(&account.stored.id)
                        && permitted(account)
                };
                let sticky = session
                    .and_then(|session| state.affinity.get(session))
                    .and_then(|(id, _)| {
                        state
                            .accounts
                            .iter()
                            .filter(usable)
                            .find(|account| &account.stored.id == id)
                    });
                let chosen = sticky.or_else(|| {
                    state.accounts.iter().filter(usable).min_by(|a, b| {
                        let load = |account: &Account| {
                            (
                                account
                                    .runtime
                                    .five_hour
                                    .as_ref()
                                    .map_or(0.0, |u| u.utilization),
                                account
                                    .runtime
                                    .seven_day
                                    .as_ref()
                                    .map_or(0.0, |u| u.utilization),
                            )
                        };
                        preferred(b)
                            .cmp(&preferred(a))
                            .then_with(|| {
                                load(a)
                                    .partial_cmp(&load(b))
                                    .unwrap_or(std::cmp::Ordering::Equal)
                            })
                            .then(a.runtime.requests.cmp(&b.runtime.requests))
                    })
                });
                match chosen {
                    Some(account) => account.stored.id.clone(),
                    None => {
                        let in_service = |account: &&Account| {
                            account.stored.enabled && !account.stored.needs_login
                        };
                        if !state
                            .accounts
                            .iter()
                            .filter(in_service)
                            .any(|account| permitted(&account))
                        {
                            return if policy.forbids_any() {
                                Selection::NoPermittedAccount
                            } else {
                                Selection::NoAccounts
                            };
                        }
                        let retry_at_ms = state
                            .accounts
                            .iter()
                            .filter(in_service)
                            .filter(permitted)
                            .filter_map(|account| account.runtime.blocked_until(now))
                            .min();
                        return Selection::Exhausted { retry_at_ms };
                    }
                }
            };
            match self.fresh_token(&candidate, None).await {
                Ok(selected) => return Selection::Selected(selected),
                Err(error) => {
                    log::warn!("[ai-proxy] account {candidate} unusable: {error}");
                    excluded.push(candidate);
                }
            }
        }
    }

    /// Return a valid access token, refreshing when it is about to expire or when
    /// `rejected` (a token the provider just refused) is still the current one.
    pub async fn fresh_token(
        &self,
        account_id: &str,
        rejected: Option<&str>,
    ) -> anyhow::Result<Selected> {
        let lock = self.state.lock().account(account_id)?.refresh.clone();
        let _refreshing = lock.lock().await;
        let (tokens, account_uuid) = {
            let mut state = self.state.lock();
            let account = state.account(account_id)?;
            anyhow::ensure!(!account.stored.needs_login, "account needs login");
            (
                account.stored.tokens.clone(),
                account.stored.account_uuid.clone(),
            )
        };
        let stale = rejected == Some(tokens.access_token.as_str());
        if !stale && !tokens.expires_soon(now_ms()) {
            return Ok(Selected {
                id: account_id.to_owned(),
                access_token: tokens.access_token,
                account_uuid,
            });
        }
        match oauth::refresh(&self.http, &self.upstream, &tokens).await {
            Ok(refreshed) => {
                let mut state = self.state.lock();
                let account = state.account(account_id)?;
                account.stored.tokens = refreshed.clone();
                // Keep serving from memory even if the write fails; the error is logged.
                if let Err(error) = self.persist(&state) {
                    log::error!("[ai-proxy] failed to persist refreshed credentials: {error}");
                }
                Ok(Selected {
                    id: account_id.to_owned(),
                    access_token: refreshed.access_token,
                    account_uuid,
                })
            }
            Err(RefreshError::Rejected) => {
                let mut state = self.state.lock();
                state.account(account_id)?.stored.needs_login = true;
                if let Err(error) = self.persist(&state) {
                    log::error!("[ai-proxy] failed to persist account state: {error}");
                }
                anyhow::bail!("refresh token rejected")
            }
            Err(RefreshError::Transient(error)) => Err(error),
        }
    }

    pub fn mark_needs_login(&self, account_id: &str) {
        let mut state = self.state.lock();
        if let Ok(account) = state.account(account_id) {
            account.stored.needs_login = true;
            if let Err(error) = self.persist(&state) {
                log::error!("[ai-proxy] failed to persist account state: {error}");
            }
        }
    }

    /// Record an upstream response: quota headers, rate-limit cooldown, affinity.
    pub fn observe(
        &self,
        account_id: &str,
        session: Option<&str>,
        status: u16,
        headers: &HeaderMap,
    ) {
        let mut state = self.state.lock();
        let now = now_ms();
        if status < 400
            && let Some(session) = session
        {
            state
                .affinity
                .retain(|_, (_, used)| now - *used < AFFINITY_TTL_MS);
            state
                .affinity
                .insert(session.to_owned(), (account_id.to_owned(), now));
        }
        let Ok(account) = state.account(account_id) else {
            return;
        };
        account.runtime.requests += 1;
        account.runtime.last_used_ms = Some(now);
        if let Some(usage) = header_usage(headers, "5h") {
            account.runtime.five_hour = Some(usage);
        }
        if let Some(usage) = header_usage(headers, "7d") {
            account.runtime.seven_day = Some(usage);
        }
        if status == 429 {
            account.runtime.cooldown_until_ms = Some(cooldown_until(headers, now));
        }
    }

    /// Refresh quota windows from the usage endpoint, so idle accounts are ranked
    /// by real numbers rather than by whatever the last response said.
    pub async fn poll_usage(&self) {
        let ids: Vec<String> = {
            let state = self.state.lock();
            state
                .accounts
                .iter()
                .filter(|account| account.stored.enabled && !account.stored.needs_login)
                .map(|account| account.stored.id.clone())
                .collect()
        };
        for id in ids {
            let selected = match self.fresh_token(&id, None).await {
                Ok(selected) => selected,
                Err(error) => {
                    log::warn!("[ai-proxy] usage poll skipped for {id}: {error}");
                    continue;
                }
            };
            match oauth::fetch_usage(&self.http, &self.upstream, &selected.access_token).await {
                Ok(usage) => {
                    let mut state = self.state.lock();
                    if let Ok(account) = state.account(&id) {
                        account.runtime.five_hour =
                            usage.five_hour.or(account.runtime.five_hour.take());
                        account.runtime.seven_day =
                            usage.seven_day.or(account.runtime.seven_day.take());
                    }
                }
                Err(error) => log::warn!("[ai-proxy] usage poll failed for {id}: {error}"),
            }
        }
    }
}

fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name)?.to_str().ok()
}

fn header_usage(headers: &HeaderMap, window: &str) -> Option<AiUsage> {
    let prefix = format!("anthropic-ratelimit-unified-{window}");
    let fraction: f64 = header(headers, &format!("{prefix}-utilization"))?
        .parse()
        .ok()?;
    Some(AiUsage {
        utilization: (fraction * 100.0).clamp(0.0, 100.0),
        resets_at_ms: header(headers, &format!("{prefix}-reset"))
            .and_then(|value| value.parse::<i64>().ok())
            .map(|seconds| seconds * 1000),
    })
}

/// Rate-limited until the provider's reset time, else `retry-after`, else a minute.
fn cooldown_until(headers: &HeaderMap, now: i64) -> i64 {
    let reset = header(headers, "anthropic-ratelimit-unified-reset")
        .and_then(|value| value.parse::<i64>().ok())
        .map(|seconds| seconds * 1000)
        .filter(|reset| *reset > now);
    let retry_after = header(headers, "retry-after")
        .and_then(|value| value.parse::<i64>().ok())
        .map(|seconds| now + seconds * 1000);
    reset.or(retry_after).unwrap_or(now + DEFAULT_COOLDOWN_MS)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(*name, value.parse().unwrap());
        }
        map
    }

    #[test]
    fn quota_headers_become_percent() {
        let usage = header_usage(
            &headers(&[
                ("anthropic-ratelimit-unified-5h-utilization", "0.42"),
                ("anthropic-ratelimit-unified-5h-reset", "1800000000"),
            ]),
            "5h",
        )
        .unwrap();
        assert!((usage.utilization - 42.0).abs() < 1e-9);
        assert_eq!(usage.resets_at_ms, Some(1_800_000_000_000));
    }

    #[test]
    fn cooldown_prefers_provider_reset() {
        let now = 1_000_000;
        assert_eq!(
            cooldown_until(
                &headers(&[
                    ("anthropic-ratelimit-unified-reset", "2000"),
                    ("retry-after", "5")
                ]),
                now
            ),
            2_000_000
        );
        assert_eq!(
            cooldown_until(&headers(&[("retry-after", "5")]), now),
            now + 5_000
        );
        assert_eq!(
            cooldown_until(&HeaderMap::new(), now),
            now + DEFAULT_COOLDOWN_MS
        );
    }

    #[test]
    fn quota_and_429_cooldowns_expire_independently() {
        let mut runtime = Runtime {
            cooldown_until_ms: Some(2_000),
            five_hour: Some(AiUsage {
                utilization: 0.0,
                resets_at_ms: None,
            }),
            seven_day: Some(AiUsage {
                utilization: 100.0,
                resets_at_ms: Some(3_000),
            }),
            ..Runtime::default()
        };
        assert_eq!(runtime.blocked_until(1_000), Some(3_000));
        assert_eq!(runtime.blocked_until(2_000), Some(3_000));
        assert_eq!(runtime.blocked_until(3_000), None);
        runtime.seven_day = Some(AiUsage {
            utilization: 7.0,
            resets_at_ms: Some(3_000),
        });
        assert_eq!(runtime.blocked_until(1_000), Some(2_000));
        assert_eq!(runtime.blocked_until(2_000), None);
        runtime.seven_day = Some(AiUsage {
            utilization: 100.0,
            resets_at_ms: None,
        });
        assert_eq!(
            runtime.blocked_until(3_000),
            Some(3_000 + DEFAULT_COOLDOWN_MS)
        );
    }

    #[tokio::test]
    async fn exhausted_quota_is_skipped_without_waiting_for_a_429() {
        let dir = tempfile::tempdir().unwrap();
        let stored = StoredAccounts {
            accounts: ["a", "b"]
                .into_iter()
                .map(|id| StoredAccount {
                    id: id.into(),
                    label: id.into(),
                    email: None,
                    account_uuid: None,
                    plan: None,
                    enabled: true,
                    needs_login: false,
                    tokens: Tokens {
                        access_token: format!("token-{id}"),
                        refresh_token: format!("refresh-{id}"),
                        expires_at_ms: i64::MAX / 2,
                    },
                })
                .collect(),
        };
        write_private(
            &dir.path().join("accounts.json"),
            &serde_json::to_vec(&stored).unwrap(),
        )
        .unwrap();
        let accounts =
            Accounts::open(dir.path(), reqwest::Client::new(), Upstream::default()).unwrap();
        let reset = (now_ms() / 1000 + 3_600).to_string();
        accounts.observe(
            "a",
            Some("conversation"),
            200,
            &headers(&[
                ("anthropic-ratelimit-unified-5h-utilization", "0"),
                ("anthropic-ratelimit-unified-7d-utilization", "1"),
                ("anthropic-ratelimit-unified-7d-reset", &reset),
            ]),
        );
        accounts.observe(
            "b",
            None,
            200,
            &headers(&[("anthropic-ratelimit-unified-5h-utilization", "0.23")]),
        );
        let Selection::Selected(selected) = accounts.select(None, Some("conversation"), &[]).await
        else {
            panic!("the account with remaining quota should be selected");
        };
        assert_eq!(selected.id, "b");
        let status = accounts.status(0, AiActivity::default());
        assert_eq!(status.accounts[0].cooldown_until_ms, None);
        assert_eq!(status.accounts[0].exhausted_period(now_ms()).unwrap(), "7d");

        accounts.observe(
            "b",
            None,
            200,
            &headers(&[
                ("anthropic-ratelimit-unified-5h-utilization", "1"),
                ("anthropic-ratelimit-unified-5h-reset", &reset),
            ]),
        );
        let Selection::Exhausted { retry_at_ms } = accounts.select(None, None, &[]).await else {
            panic!("both accounts have exhausted their quota");
        };
        assert_eq!(retry_at_ms, Some(reset.parse::<i64>().unwrap() * 1000));
    }

    #[test]
    fn corrupt_store_is_an_error() {
        for file in ["accounts.json", "routing.json"] {
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(dir.path().join(file), b"{broken").unwrap();
            let opened = Accounts::open(dir.path(), reqwest::Client::new(), Upstream::default());
            assert!(opened.is_err(), "{file}");
        }
    }
}
