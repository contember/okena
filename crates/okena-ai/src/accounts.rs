//! Account pool: persisted credentials, runtime quota state and account selection.

use crate::now_ms;
use crate::oauth::{self, Pkce, RefreshError, Tokens, Upstream};
use okena_core::ai_proxy::{AiAccount, AiPendingLogin, AiProxyStatus, AiUsage, write_private};
use parking_lot::Mutex;
use reqwest::header::HeaderMap;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
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

struct Account {
    stored: StoredAccount,
    runtime: Runtime,
    /// Serializes refreshes: a refresh token is single-use.
    refresh: Arc<tokio::sync::Mutex<()>>,
}

impl Account {
    fn available(&self, now: i64) -> bool {
        self.stored.enabled
            && !self.stored.needs_login
            && self
                .runtime
                .cooldown_until_ms
                .is_none_or(|until| until <= now)
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
}

impl State {
    fn account(&mut self, id: &str) -> anyhow::Result<&mut Account> {
        self.accounts
            .iter_mut()
            .find(|account| account.stored.id == id)
            .ok_or_else(|| anyhow::anyhow!("Unknown account"))
    }
}

pub struct Accounts {
    path: PathBuf,
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
    /// No usable account; the earliest cooldown end, if any account is cooling down.
    Exhausted {
        retry_at_ms: Option<i64>,
    },
}

impl Accounts {
    pub fn open(path: PathBuf, http: reqwest::Client, upstream: Upstream) -> anyhow::Result<Self> {
        // A corrupt file is an error, never an empty pool: it holds the only
        // copy of the refresh tokens.
        let stored: StoredAccounts = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|_| anyhow::anyhow!("{} is corrupt", path.display()))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => StoredAccounts::default(),
            Err(error) => return Err(error.into()),
        };
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
            http,
            upstream,
            state: Mutex::new(State {
                accounts,
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

    pub fn status(&self, inflight: usize) -> AiProxyStatus {
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
        self.persist(&state)
    }

    pub fn set_enabled(&self, account_id: &str, enabled: bool) -> anyhow::Result<()> {
        let mut state = self.state.lock();
        state.account(account_id)?.stored.enabled = enabled;
        self.persist(&state)
    }

    /// Pick an account for a request: the conversation's previous account while
    /// it is usable, otherwise the least-used available one.
    pub async fn select(&self, session: Option<&str>, exclude: &[String]) -> Selection {
        let mut excluded = exclude.to_vec();
        loop {
            let candidate = {
                let state = self.state.lock();
                let now = now_ms();
                let usable = |account: &&Account| {
                    account.available(now) && !excluded.contains(&account.stored.id)
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
                        load(a)
                            .partial_cmp(&load(b))
                            .unwrap_or(std::cmp::Ordering::Equal)
                            .then(a.runtime.requests.cmp(&b.runtime.requests))
                    })
                });
                match chosen {
                    Some(account) => account.stored.id.clone(),
                    None => {
                        if !state
                            .accounts
                            .iter()
                            .any(|account| account.stored.enabled && !account.stored.needs_login)
                        {
                            return Selection::NoAccounts;
                        }
                        let retry_at_ms = state
                            .accounts
                            .iter()
                            .filter(|account| account.stored.enabled && !account.stored.needs_login)
                            .filter_map(|account| account.runtime.cooldown_until_ms)
                            .filter(|until| *until > now)
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
    fn corrupt_store_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("accounts.json");
        std::fs::write(&path, b"{broken").unwrap();
        assert!(Accounts::open(path, reqwest::Client::new(), Upstream::default()).is_err());
    }
}
