//! Shared contract of the per-profile AI proxy (`okena-ai`).
//!
//! The proxy is a separate process so `claude` sessions kept alive by dtach/tmux
//! keep a working `ANTHROPIC_BASE_URL` while the workspace daemon restarts. Its
//! endpoint (port + secret path token) is persisted in the profile, so the URL a
//! shell was started with stays valid across proxy restarts too.

use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Directory holding the proxy's endpoint, accounts, lock and log.
pub fn proxy_dir(profile_root: &Path) -> PathBuf {
    profile_root.join("ai-proxy")
}

fn endpoint_path(profile_root: &Path) -> PathBuf {
    proxy_dir(profile_root).join("endpoint.json")
}

/// Loopback address of the proxy. The token is the first path segment of every
/// inference request, so only processes that were handed the URL (Okena shells)
/// can send requests through the proxy. Managing it needs the separate
/// [management key](ensure_management_key).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProxyEndpoint {
    pub port: u16,
    pub token: String,
}

impl ProxyEndpoint {
    /// Value for `ANTHROPIC_BASE_URL`; the SDK appends `/v1/messages` to it.
    pub fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}/{}", self.port, self.token)
    }

    /// `ANTHROPIC_BASE_URL` for a terminal of `project_id`, so the proxy applies
    /// that project's [routing policy](AiRouting). An id that cannot be a path
    /// segment gets the plain URL and with it the default policy.
    pub fn project_url(&self, project_id: &str) -> String {
        if is_routable_project_id(project_id) {
            format!("{}/{PROJECT_SEGMENT}/{project_id}", self.base_url())
        } else {
            self.base_url()
        }
    }
}

/// Path segment that precedes the project id in a terminal's base URL.
pub const PROJECT_SEGMENT: &str = "p";

/// Whether `project_id` can travel as a path segment of the base URL unescaped.
pub fn is_routable_project_id(project_id: &str) -> bool {
    !project_id.is_empty()
        && project_id.len() <= 128
        && project_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

pub fn load_endpoint(profile_root: &Path) -> anyhow::Result<Option<ProxyEndpoint>> {
    match std::fs::read(endpoint_path(profile_root)) {
        Ok(bytes) => Ok(Some(
            serde_json::from_slice(&bytes).context("invalid ai-proxy endpoint.json")?,
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

pub fn save_endpoint(profile_root: &Path, endpoint: &ProxyEndpoint) -> anyhow::Result<()> {
    write_private(&endpoint_path(profile_root), &serde_json::to_vec(endpoint)?)
}

/// Load the endpoint, creating it (free loopback port + random token) on first use.
pub fn ensure_endpoint(profile_root: &Path) -> anyhow::Result<ProxyEndpoint> {
    if let Some(endpoint) = load_endpoint(profile_root)? {
        return Ok(endpoint);
    }
    let endpoint = ProxyEndpoint {
        port: free_loopback_port()?,
        token: random_token()?,
    };
    create_private(
        &endpoint_path(profile_root),
        &serde_json::to_vec(&endpoint)?,
    )?;
    load_endpoint(profile_root)?.context("ai-proxy endpoint.json vanished after creation")
}

fn management_key_path(profile_root: &Path) -> PathBuf {
    proxy_dir(profile_root).join("management.key")
}

/// Load the secret that authorizes the proxy's management routes, creating it on
/// first use. Unlike the endpoint token it never enters a shell's environment.
pub fn ensure_management_key(profile_root: &Path) -> anyhow::Result<String> {
    let path = management_key_path(profile_root);
    if !path.exists() {
        create_private(&path, random_token()?.as_bytes())?;
    }
    let key = std::fs::read_to_string(&path)?;
    anyhow::ensure!(!key.is_empty(), "{} is empty", path.display());
    Ok(key)
}

pub fn free_loopback_port() -> anyhow::Result<u16> {
    Ok(std::net::TcpListener::bind(("127.0.0.1", 0))?
        .local_addr()?
        .port())
}

pub fn random_token() -> anyhow::Result<String> {
    let mut bytes = [0u8; 24];
    getrandom::getrandom(&mut bytes).map_err(|error| anyhow::anyhow!("getrandom: {error}"))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// Atomically replace `path` with `bytes`, readable only by the owner.
pub fn write_private(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    private_temp_file(path, bytes)?
        .persist(path)
        .map_err(|error| error.error)?;
    Ok(())
}

/// Create `path` with `bytes`, readable only by the owner, unless it exists.
/// The daemon and a starting proxy may race to create it; the first one wins.
fn create_private(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    match private_temp_file(path, bytes)?.persist_noclobber(path) {
        Ok(_) => Ok(()),
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(error.error.into()),
    }
}

/// Owner-only temporary file next to `path`, holding `bytes` and ready to persist.
fn private_temp_file(path: &Path, bytes: &[u8]) -> anyhow::Result<tempfile::NamedTempFile> {
    use std::io::Write;
    let dir = path.parent().context("path has no parent directory")?;
    std::fs::create_dir_all(dir)?;
    let mut file = tempfile::NamedTempFile::new_in(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    Ok(file)
}

/// Management commands, sent by clients through the daemon (`ActionRequest::AiProxy`)
/// and forwarded verbatim to the proxy. Every command answers with [`AiProxyStatus`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum AiProxyCommand {
    Status,
    /// Begin an OAuth login; the authorization URL appears in `AiProxyStatus::logins`.
    StartLogin {
        label: String,
    },
    /// Finish a login with the code shown by the provider after authorization.
    FinishLogin {
        login_id: String,
        code: String,
    },
    CancelLogin {
        login_id: String,
    },
    RemoveAccount {
        account_id: String,
    },
    SetAccountEnabled {
        account_id: String,
        enabled: bool,
    },
    /// Replace the policy for terminals whose project has no policy of its own.
    SetDefaultPolicy {
        policy: AiRoutingPolicy,
    },
    /// Replace a project's policy; `None` makes the project use the default policy.
    SetProjectPolicy {
        project_id: String,
        policy: Option<AiRoutingPolicy>,
    },
}

/// How a routing policy treats one pool account.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AiAccountRule {
    /// Used before allowed accounts while it is usable.
    Prefer,
    #[default]
    Allow,
    Forbid,
}

/// Which pool accounts may serve a terminal's requests.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiRoutingPolicy {
    /// Rule for accounts not listed in `accounts`, including accounts added later.
    pub others: AiAccountRule,
    /// Account id → rule.
    #[serde(default)]
    pub accounts: BTreeMap<String, AiAccountRule>,
}

impl AiRoutingPolicy {
    pub fn rule(&self, account_id: &str) -> AiAccountRule {
        self.accounts
            .get(account_id)
            .copied()
            .unwrap_or(self.others)
    }

    /// A policy that forbids an account never falls back to the terminal's own
    /// Claude login: that login may be the forbidden account.
    pub fn forbids_any(&self) -> bool {
        self.others == AiAccountRule::Forbid
            || self
                .accounts
                .values()
                .any(|rule| *rule == AiAccountRule::Forbid)
    }
}

/// Routing policies, owned and persisted by the proxy (`routing.json`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiRouting {
    /// Policy for terminals without a project or whose project has no policy.
    #[serde(default)]
    pub default: AiRoutingPolicy,
    /// Project id → policy. Worktree terminals carry their parent project's id.
    #[serde(default)]
    pub projects: BTreeMap<String, AiRoutingPolicy>,
}

impl AiRouting {
    pub fn policy(&self, project_id: Option<&str>) -> &AiRoutingPolicy {
        project_id
            .and_then(|project_id| self.projects.get(project_id))
            .unwrap_or(&self.default)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AiProxyStatus {
    pub version: String,
    /// Requests currently being relayed.
    pub inflight: usize,
    pub accounts: Vec<AiAccount>,
    pub logins: Vec<AiPendingLogin>,
    #[serde(default)]
    pub routing: AiRouting,
    #[serde(default)]
    pub activity: AiActivity,
}

/// What the proxy did with `/v1/messages` requests in the last hour. Kept in
/// memory only, so it starts empty when the proxy restarts.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AiActivity {
    /// When the proxy started; later than an hour ago right after a restart.
    pub since_ms: i64,
    /// One entry per account (or the own login) that answered or was rate
    /// limited in the last hour.
    pub routes: Vec<AiRouteActivity>,
    /// Failovers and failures, newest first.
    pub events: Vec<AiActivityEvent>,
}

/// Number of entries in [`AiRouteActivity::per_minute`].
pub const AI_ACTIVITY_MINUTES: usize = 60;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AiRouteActivity {
    /// The pool account; `None` is the terminal's own Claude login.
    pub account_id: Option<String>,
    /// Requests answered in each of the last 60 minutes, oldest first; the last
    /// entry covers the last 60 seconds.
    pub per_minute: Vec<u32>,
    /// Answers in the last hour that were an error, or no answer at all.
    pub errors: u32,
    /// 429 answers in the last hour.
    pub rate_limited: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AiActivityEvent {
    /// When it last happened.
    pub at_ms: i64,
    /// Local daemon project of the terminal that sent the request.
    pub project_id: Option<String>,
    pub kind: AiActivityKind,
    /// How often it happened; a repeat moves the event to the top of the list.
    pub count: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AiActivityKind {
    /// The account answered 429; the request moved to the next permitted account.
    RateLimited { account_id: String },
    /// The account's access token was rejected; the proxy refreshes it once.
    Unauthorized { account_id: String },
    /// Every permitted account is rate limited; the request got a 429.
    AllRateLimited,
    /// The policy permits no enabled, logged-in account; the request got a 400.
    NoPermittedAccount,
    /// An error answer, or (`status: None`) no answer from the provider.
    /// `account_id: None` is the terminal's own Claude login.
    Failed {
        account_id: Option<String>,
        status: Option<u16>,
        message: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AiAccount {
    pub id: String,
    pub label: String,
    pub email: Option<String>,
    /// Subscription type reported by the provider (e.g. "max").
    pub plan: Option<String>,
    pub enabled: bool,
    /// The refresh token was rejected; the account needs a new login.
    pub needs_login: bool,
    /// An HTTP 429 cooldown; quota exhaustion is derived from the usage windows.
    pub cooldown_until_ms: Option<i64>,
    pub five_hour: Option<AiUsage>,
    pub seven_day: Option<AiUsage>,
    pub requests: u64,
    pub last_used_ms: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AiUsage {
    /// Percent of the window used, 0–100.
    pub utilization: f64,
    pub resets_at_ms: Option<i64>,
}

impl AiUsage {
    /// A reset in the past no longer blocks requests, even before the next poll.
    pub fn is_exhausted(&self, now_ms: i64) -> bool {
        self.utilization >= 100.0 && self.resets_at_ms.is_none_or(|reset| reset > now_ms)
    }
}

impl AiAccount {
    /// Prefer the weekly reason when both windows are exhausted.
    pub fn exhausted_period(&self, now_ms: i64) -> Option<&'static str> {
        [
            ("7d", self.seven_day.as_ref()),
            ("5h", self.five_hour.as_ref()),
        ]
        .into_iter()
        .find_map(|(period, usage)| {
            usage
                .filter(|usage| usage.is_exhausted(now_ms))
                .map(|_| period)
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AiPendingLogin {
    pub id: String,
    pub label: String,
    pub authorize_url: String,
    pub error: Option<String>,
}

/// Daemon reply to `ActionRequest::AiProxy`. (`failure`, not `error`: a
/// top-level `error` key marks the whole action as failed on the wire.)
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AiProxyState {
    /// Proxy status, when the proxy is reachable.
    pub status: Option<AiProxyStatus>,
    /// Why the command failed or the proxy is unreachable.
    pub failure: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_is_created_once_and_reused() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load_endpoint(dir.path()).unwrap().is_none());
        let created = ensure_endpoint(dir.path()).unwrap();
        assert_eq!(created.token.len(), 48);
        assert_eq!(ensure_endpoint(dir.path()).unwrap(), created);
        assert_eq!(
            created.base_url(),
            format!("http://127.0.0.1:{}/{}", created.port, created.token)
        );
    }

    #[test]
    fn concurrent_first_use_agrees_on_one_endpoint() {
        let dir = tempfile::tempdir().unwrap();
        let barrier = std::sync::Barrier::new(8);
        let endpoints: Vec<ProxyEndpoint> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..8)
                .map(|_| {
                    scope.spawn(|| {
                        barrier.wait();
                        ensure_endpoint(dir.path()).unwrap()
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|handle| handle.join().unwrap())
                .collect()
        });
        let stored = load_endpoint(dir.path()).unwrap().unwrap();
        assert!(endpoints.iter().all(|endpoint| *endpoint == stored));
    }

    #[cfg(unix)]
    #[test]
    fn endpoint_and_management_key_files_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        ensure_endpoint(dir.path()).unwrap();
        ensure_management_key(dir.path()).unwrap();
        for path in [endpoint_path(dir.path()), management_key_path(dir.path())] {
            let mode = std::fs::metadata(path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn management_key_is_created_once_and_differs_from_the_token() {
        let dir = tempfile::tempdir().unwrap();
        let key = ensure_management_key(dir.path()).unwrap();
        assert_eq!(key.len(), 48);
        assert_eq!(ensure_management_key(dir.path()).unwrap(), key);
        assert_ne!(ensure_endpoint(dir.path()).unwrap().token, key);
    }

    #[test]
    fn empty_management_key_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        write_private(&management_key_path(dir.path()), b"").unwrap();
        assert!(ensure_management_key(dir.path()).is_err());
    }

    #[test]
    fn project_url_carries_only_path_safe_ids() {
        let endpoint = ProxyEndpoint {
            port: 4000,
            token: "tok".into(),
        };
        assert_eq!(
            endpoint.project_url("1f0c-a_B9"),
            "http://127.0.0.1:4000/tok/p/1f0c-a_B9"
        );
        for unsafe_id in ["", "a/b", "a?b", "a b", "../x"] {
            assert_eq!(endpoint.project_url(unsafe_id), endpoint.base_url());
        }
    }

    #[test]
    fn unlisted_accounts_follow_others() {
        let only_a = AiRoutingPolicy {
            others: AiAccountRule::Forbid,
            accounts: BTreeMap::from([("a".into(), AiAccountRule::Allow)]),
        };
        assert_eq!(only_a.rule("a"), AiAccountRule::Allow);
        assert_eq!(only_a.rule("added-later"), AiAccountRule::Forbid);
        assert!(only_a.forbids_any());

        let prefer_b = AiRoutingPolicy {
            others: AiAccountRule::Allow,
            accounts: BTreeMap::from([("b".into(), AiAccountRule::Prefer)]),
        };
        assert!(!prefer_b.forbids_any());
        assert!(!AiRoutingPolicy::default().forbids_any());
    }

    #[test]
    fn project_without_policy_uses_the_default() {
        let routing = AiRouting {
            default: AiRoutingPolicy {
                others: AiAccountRule::Prefer,
                accounts: BTreeMap::new(),
            },
            projects: BTreeMap::from([("x".into(), AiRoutingPolicy::default())]),
        };
        assert_eq!(routing.policy(Some("x")), &AiRoutingPolicy::default());
        assert_eq!(routing.policy(Some("y")), &routing.default);
        assert_eq!(routing.policy(None), &routing.default);
    }

    #[test]
    fn commands_use_a_command_tag() {
        let json = serde_json::to_value(AiProxyCommand::SetAccountEnabled {
            account_id: "a".into(),
            enabled: false,
        })
        .unwrap();
        assert_eq!(
            json,
            serde_json::json!({"command": "set_account_enabled", "account_id": "a", "enabled": false})
        );
    }
}
