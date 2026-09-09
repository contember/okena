//! GitHub API access for the PR/CI poll path: the base repository resolved the
//! way non-interactive `gh` resolves it, the token from the sources `gh` reads
//! (env, then one cached `gh auth token` spawn), and REST/GraphQL calls over the
//! shared [`okena_transport::http`] bus so connections are reused.

use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant};

use gix::bstr::ByteSlice;
use okena_core::process::{command, safe_output_with_timeout};
use okena_transport::http::{self, HttpRequest, HttpResponse};
use parking_lot::Mutex;
use serde_json::Value;

/// Per-request cap, matching the old per-`gh`-invocation timeout.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// How long a token from `gh auth token` is trusted before asking again.
const TOKEN_TTL: Duration = Duration::from_secs(30 * 60);
/// A missing token is re-checked this often so a fresh `gh auth login` is noticed.
const MISSING_TOKEN_TTL: Duration = Duration::from_secs(60);
const DEFAULT_HOST: &str = "github.com";

/// A GitHub repository: `owner/name` on a normalised `host`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GithubRepo {
    pub host: String,
    pub owner: String,
    pub name: String,
}

/// Whether the repo has any remote pointing at github.com (https or ssh).
///
/// Used to gate PR/CI polling: repos with no GitHub remote (local-only,
/// GitLab, Bitbucket, …) can never have GitHub PRs/checks, so every lookup
/// for them just fails after a round-trip. Skipping them is the bulk of the
/// poller's fan-out on a machine with many non-GitHub projects.
pub fn has_github_remote(path: &Path) -> bool {
    let Some(repo) = crate::gix_helpers::open(path) else {
        return false;
    };
    let names = repo.remote_names();
    for name in names.iter() {
        let Ok(remote) = repo.find_remote(name.as_ref()) else {
            continue;
        };
        for dir in [gix::remote::Direction::Fetch, gix::remote::Direction::Push] {
            if let Some(host) = remote.url(dir).and_then(|u| u.host())
                && (host == "github.com" || host.ends_with(".github.com"))
            {
                return true;
            }
        }
    }
    false
}

/// Lowercase, with any `*.github.com` alias folded to `github.com` — gh's
/// `NormalizeHostname`.
fn normalize_host(host: &str) -> String {
    let host = host.to_ascii_lowercase();
    if host.ends_with(".github.com") {
        DEFAULT_HOST.to_string()
    } else {
        host
    }
}

/// `owner/name@host` from a remote URL, per gh's `ghrepo.FromURL`: the first
/// two path segments, `.git` dropped. Handles `https://`, `ssh://` and the
/// scp-like `git@host:owner/name` form (gix parses all three).
fn repo_from_url(url: &gix::Url) -> Option<GithubRepo> {
    let host = url.host()?;
    let path = url.path.to_str().ok()?;
    let mut segments = path.trim_matches('/').split('/');
    let owner = segments.next().filter(|s| !s.is_empty())?;
    let name = segments.next().filter(|s| !s.is_empty())?;
    let name = name.strip_suffix(".git").unwrap_or(name);
    Some(GithubRepo {
        host: normalize_host(host),
        owner: owner.to_string(),
        name: name.to_string(),
    })
}

/// A git remote that maps to a GitHub repository, plus its `gh repo
/// set-default` marker (`remote.<name>.gh-resolved`) if any.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RemoteCandidate {
    name: String,
    repo: GithubRepo,
    resolved: Option<String>,
}

fn remote_candidates(repo: &gix::Repository) -> Vec<RemoteCandidate> {
    let config = repo.config_snapshot();
    let names = repo.remote_names();
    let mut candidates = Vec::new();
    for name in names.iter() {
        let Ok(remote) = repo.find_remote(name.as_ref()) else {
            continue;
        };
        // gh maps the fetch URL and falls back to the push URL.
        let parsed = remote
            .url(gix::remote::Direction::Fetch)
            .or_else(|| remote.url(gix::remote::Direction::Push))
            .and_then(repo_from_url);
        let Some(parsed) = parsed else {
            continue;
        };
        let resolved = config
            .string(format!("remote.{name}.gh-resolved").as_str())
            .map(|value| value.to_string());
        candidates.push(RemoteCandidate {
            name: name.to_string(),
            repo: parsed,
            resolved,
        });
    }
    candidates
}

/// gh's remote priority: `upstream` > `github` > `origin` > anything else.
fn remote_name_score(name: &str) -> u8 {
    match name.to_ascii_lowercase().as_str() {
        "upstream" => 3,
        "github" => 2,
        "origin" => 1,
        _ => 0,
    }
}

/// `owner/name` or `host/owner/name`, as stored by `gh repo set-default`.
fn repo_from_full_name(full_name: &str, host: &str) -> Option<GithubRepo> {
    let parts: Vec<&str> = full_name.split('/').collect();
    let (owner, name) = match parts.as_slice() {
        [owner, name] => (*owner, *name),
        [_, owner, name] => (*owner, *name),
        _ => return None,
    };
    if owner.is_empty() || name.is_empty() {
        return None;
    }
    Some(GithubRepo {
        host: host.to_string(),
        owner: owner.to_string(),
        name: name.to_string(),
    })
}

/// The base repository the way non-interactive `gh` picks it: only remotes on
/// `host` count, a `gh repo set-default` choice wins, else the highest-priority
/// remote name (ties keep `git remote` order).
fn select_base_repo(candidates: Vec<RemoteCandidate>, host: &str) -> Option<GithubRepo> {
    let mut candidates: Vec<RemoteCandidate> = candidates
        .into_iter()
        .filter(|candidate| candidate.repo.host == host)
        .collect();
    candidates.sort_by_key(|candidate| std::cmp::Reverse(remote_name_score(&candidate.name)));
    for candidate in &candidates {
        match candidate.resolved.as_deref() {
            Some("base") => return Some(candidate.repo.clone()),
            Some(full_name) => return repo_from_full_name(full_name, &candidate.repo.host),
            None => {}
        }
    }
    candidates
        .into_iter()
        .next()
        .map(|candidate| candidate.repo)
}

/// The one host gh considers remotes on: `GH_HOST` when set, else github.com.
fn host_filter() -> String {
    std::env::var("GH_HOST")
        .ok()
        .map(|host| host.trim().to_string())
        .filter(|host| !host.is_empty())
        .map(|host| normalize_host(&host))
        .unwrap_or_else(|| DEFAULT_HOST.to_string())
}

/// The GitHub repository `gh` would run against for this checkout.
pub(crate) fn resolve_base_repo(path: &Path) -> Option<GithubRepo> {
    let repo = crate::gix_helpers::open(path)?;
    select_base_repo(remote_candidates(&repo), &host_filter())
}

struct TokenEntry {
    token: Option<String>,
    fetched_at: Instant,
}

/// Process-wide token cache keyed by host: one `gh auth token` spawn per TTL
/// instead of one per query.
static TOKENS: Mutex<Option<HashMap<String, TokenEntry>>> = Mutex::new(None);

/// The env override gh itself honours before touching its keyring.
fn env_token(host: &str) -> Option<String> {
    let names = if host == DEFAULT_HOST {
        ["GH_TOKEN", "GITHUB_TOKEN"]
    } else {
        ["GH_ENTERPRISE_TOKEN", "GITHUB_ENTERPRISE_TOKEN"]
    };
    names.iter().find_map(|name| {
        std::env::var(name)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    })
}

fn gh_cli_token(host: &str) -> Option<String> {
    let output = safe_output_with_timeout(
        command("gh").args(["auth", "token", "--hostname", host]),
        REQUEST_TIMEOUT,
    )
    .ok()?;
    if !output.status.success() {
        return None;
    }
    let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!token.is_empty()).then_some(token)
}

fn cached_token(
    cache: &mut HashMap<String, TokenEntry>,
    host: &str,
    fetch: impl FnOnce() -> Option<String>,
) -> Option<String> {
    if let Some(entry) = cache.get(host) {
        let ttl = if entry.token.is_some() {
            TOKEN_TTL
        } else {
            MISSING_TOKEN_TTL
        };
        if entry.fetched_at.elapsed() < ttl {
            return entry.token.clone();
        }
    }
    let token = fetch();
    cache.insert(
        host.to_string(),
        TokenEntry {
            token: token.clone(),
            fetched_at: Instant::now(),
        },
    );
    token
}

/// Token for `host`: env first (as gh does), then the cached `gh auth token`.
/// The lock is held across the spawn so concurrent poll workers share one.
fn token_for(host: &str) -> Option<String> {
    if let Some(token) = env_token(host) {
        return Some(token);
    }
    let mut guard = TOKENS.lock();
    let cache = guard.get_or_insert_with(HashMap::new);
    cached_token(cache, host, || gh_cli_token(host))
}

fn forget_token(host: &str) {
    if let Some(cache) = TOKENS.lock().as_mut() {
        cache.remove(host);
    }
}

/// gh's `RESTPrefix`: api.github.com for github.com, `/api/v3/` elsewhere.
fn rest_prefix(host: &str) -> String {
    if host == DEFAULT_HOST {
        "https://api.github.com/".to_string()
    } else {
        format!("https://{host}/api/v3/")
    }
}

/// gh's `GraphQLEndpoint`.
fn graphql_endpoint(host: &str) -> String {
    if host == DEFAULT_HOST {
        "https://api.github.com/graphql".to_string()
    } else {
        format!("https://{host}/api/graphql")
    }
}

/// Why a call produced no data. `RateLimited` is kept apart so the poller can
/// park its whole fan-out; everything else is the old "`gh` failed" outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ApiError {
    RateLimited,
    Failed,
}

/// GitHub refuses with 403/429 for both the primary hourly limit
/// (`x-ratelimit-remaining: 0`) and the secondary abuse limit (only the
/// message says so); the wording matches what `gh` printed on stderr.
fn is_rate_limited(resp: &HttpResponse) -> bool {
    if !matches!(resp.status(), 403 | 429) {
        return false;
    }
    if resp.header("x-ratelimit-remaining") == Some("0") {
        return true;
    }
    resp.json::<Value>()
        .ok()
        .and_then(|body| body.get("message")?.as_str().map(str::to_lowercase))
        .is_some_and(|message| message.contains("rate limit"))
}

/// The primary limit on GraphQL arrives as a 200 with a typed error.
fn is_graphql_rate_limit(error: &Value) -> bool {
    error.get("type").and_then(Value::as_str) == Some("RATE_LIMITED")
        || error
            .get("message")
            .and_then(Value::as_str)
            .is_some_and(|message| message.to_lowercase().contains("rate limit"))
}

/// An authenticated client for one GitHub host.
pub(crate) struct GithubClient {
    host: String,
    token: String,
}

impl GithubClient {
    /// `None` when no token is obtainable — the caller treats it as `gh` failing.
    pub(crate) fn for_host(host: &str) -> Option<Self> {
        let token = token_for(host)?;
        Some(Self {
            host: host.to_string(),
            token,
        })
    }

    #[cfg(test)]
    fn with_token(host: &str, token: &str) -> Self {
        Self {
            host: host.to_string(),
            token: token.to_string(),
        }
    }

    /// Send, retrying once with a fresh token on 401, and turn a rate-limit
    /// refusal into [`ApiError::RateLimited`]. Any other status is returned.
    fn send(&mut self, build: impl Fn() -> HttpRequest) -> Result<HttpResponse, ApiError> {
        let mut resp = self.send_once(&build)?;
        if resp.status() == 401 {
            forget_token(&self.host);
            self.token = token_for(&self.host).ok_or(ApiError::Failed)?;
            resp = self.send_once(&build)?;
        }
        if is_rate_limited(&resp) {
            return Err(ApiError::RateLimited);
        }
        Ok(resp)
    }

    fn send_once(&self, build: &impl Fn() -> HttpRequest) -> Result<HttpResponse, ApiError> {
        http::send(
            build()
                .bearer(&self.token)
                .timeout(REQUEST_TIMEOUT)
                .label("github.api"),
        )
        .map_err(|_| ApiError::Failed)
    }

    fn rest_get(&mut self, url: &str) -> Result<HttpResponse, ApiError> {
        let resp = self.send(|| {
            HttpRequest::get(url)
                .header("Accept", "application/vnd.github+json")
                .header("X-GitHub-Api-Version", "2022-11-28")
        })?;
        if !resp.is_success() {
            return Err(ApiError::Failed);
        }
        Ok(resp)
    }

    /// GET one REST resource (`path` is relative to the API root).
    pub(crate) fn rest_get_json(&mut self, path: &str) -> Result<Value, ApiError> {
        let url = format!("{}{path}", rest_prefix(&self.host));
        self.rest_get(&url)?.json().map_err(|_| ApiError::Failed)
    }

    /// GET a paginated REST collection, following `Link: rel="next"` and
    /// concatenating the `key` arrays — what `gh api --paginate` did.
    pub(crate) fn rest_get_all(&mut self, path: &str, key: &str) -> Result<Vec<Value>, ApiError> {
        let mut url = format!("{}{path}", rest_prefix(&self.host));
        let mut items = Vec::new();
        loop {
            let resp = self.rest_get(&url)?;
            let body: Value = resp.json().map_err(|_| ApiError::Failed)?;
            if let Some(page) = body.get(key).and_then(Value::as_array) {
                items.extend(page.iter().cloned());
            }
            match resp.next_link() {
                Some(next) => url = next.to_string(),
                None => return Ok(items),
            }
        }
    }

    /// POST a GraphQL query and return its `data`. Any reported error fails the
    /// call the way it failed `gh`, except a rate-limit error.
    pub(crate) fn graphql(&mut self, query: &str, variables: Value) -> Result<Value, ApiError> {
        let url = graphql_endpoint(&self.host);
        let body = serde_json::json!({ "query": query, "variables": variables });
        let resp = self.send(|| {
            HttpRequest::post(&url)
                .json(&body)
                .header("Accept", "application/json")
        })?;
        if !resp.is_success() {
            return Err(ApiError::Failed);
        }
        let mut payload: Value = resp.json().map_err(|_| ApiError::Failed)?;
        if let Some(errors) = payload.get("errors").and_then(Value::as_array)
            && !errors.is_empty()
        {
            return Err(if errors.iter().any(is_graphql_rate_limit) {
                ApiError::RateLimited
            } else {
                ApiError::Failed
            });
        }
        match payload.get_mut("data") {
            Some(data) if !data.is_null() => Ok(data.take()),
            _ => Err(ApiError::Failed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use okena_transport::http::testing;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex as StdMutex};

    // The HTTP bus mock is process-global; tests that install one must not
    // overlap.
    static MOCK_LOCK: StdMutex<()> = StdMutex::new(());

    fn mock_guard() -> std::sync::MutexGuard<'static, ()> {
        MOCK_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn parse(url: &str) -> gix::Url {
        gix::url::parse(gix::bstr::BStr::new(url)).expect("valid url")
    }

    fn repo(host: &str, owner: &str, name: &str) -> GithubRepo {
        GithubRepo {
            host: host.into(),
            owner: owner.into(),
            name: name.into(),
        }
    }

    fn candidate(name: &str, repo: GithubRepo, resolved: Option<&str>) -> RemoteCandidate {
        RemoteCandidate {
            name: name.into(),
            repo,
            resolved: resolved.map(String::from),
        }
    }

    #[test]
    fn remote_url_forms_map_to_owner_and_name() {
        let expected = repo("github.com", "contember", "okena");
        for url in [
            "https://github.com/contember/okena.git",
            "https://github.com/contember/okena",
            "https://github.com/contember/okena/",
            "git@github.com:contember/okena.git",
            "ssh://git@github.com/contember/okena.git",
            "ssh://git@ssh.github.com:443/contember/okena.git",
            "https://user:pass@github.com/contember/okena.git",
            "HTTPS://GitHub.com/contember/okena",
        ] {
            assert_eq!(repo_from_url(&parse(url)), Some(expected.clone()), "{url}");
        }
    }

    #[test]
    fn remote_url_keeps_other_hosts_and_extra_segments() {
        assert_eq!(
            repo_from_url(&parse("https://ghe.example.com/team/app.git")),
            Some(repo("ghe.example.com", "team", "app"))
        );
        // gh takes the first two segments and ignores the rest.
        assert_eq!(
            repo_from_url(&parse("https://github.com/o/r/tree/main")),
            Some(repo("github.com", "o", "r"))
        );
    }

    #[test]
    fn remote_url_without_owner_and_name_is_rejected() {
        assert_eq!(repo_from_url(&parse("https://github.com/only-owner")), None);
        assert_eq!(repo_from_url(&parse("https://github.com/")), None);
        assert_eq!(repo_from_url(&parse("/local/path/repo.git")), None);
    }

    #[test]
    fn base_repo_follows_gh_remote_priority() {
        let candidates = vec![
            candidate("origin", repo("github.com", "me", "fork"), None),
            candidate("upstream", repo("github.com", "org", "main"), None),
            candidate("backup", repo("github.com", "x", "y"), None),
        ];
        assert_eq!(
            select_base_repo(candidates, "github.com"),
            Some(repo("github.com", "org", "main"))
        );

        let candidates = vec![
            candidate("origin", repo("github.com", "me", "fork"), None),
            candidate("github", repo("github.com", "org", "main"), None),
        ];
        assert_eq!(
            select_base_repo(candidates, "github.com"),
            Some(repo("github.com", "org", "main"))
        );

        // Ties keep `git remote` (alphabetical) order.
        let candidates = vec![
            candidate("alpha", repo("github.com", "a", "a"), None),
            candidate("beta", repo("github.com", "b", "b"), None),
        ];
        assert_eq!(
            select_base_repo(candidates, "github.com"),
            Some(repo("github.com", "a", "a"))
        );
    }

    #[test]
    fn base_repo_honours_gh_repo_set_default() {
        let candidates = vec![
            candidate("origin", repo("github.com", "me", "fork"), Some("base")),
            candidate("upstream", repo("github.com", "org", "main"), None),
        ];
        assert_eq!(
            select_base_repo(candidates, "github.com"),
            Some(repo("github.com", "me", "fork"))
        );

        let candidates = vec![candidate(
            "origin",
            repo("github.com", "me", "fork"),
            Some("other/target"),
        )];
        assert_eq!(
            select_base_repo(candidates, "github.com"),
            Some(repo("github.com", "other", "target"))
        );

        let candidates = vec![candidate(
            "origin",
            repo("github.com", "me", "fork"),
            Some("github.com/other/target"),
        )];
        assert_eq!(
            select_base_repo(candidates, "github.com"),
            Some(repo("github.com", "other", "target"))
        );
    }

    #[test]
    fn base_repo_only_considers_the_active_host() {
        let candidates = vec![
            candidate("upstream", repo("ghe.example.com", "org", "main"), None),
            candidate("origin", repo("github.com", "me", "fork"), None),
        ];
        assert_eq!(
            select_base_repo(candidates.clone(), "github.com"),
            Some(repo("github.com", "me", "fork"))
        );
        assert_eq!(
            select_base_repo(candidates, "ghe.example.com"),
            Some(repo("ghe.example.com", "org", "main"))
        );
        assert_eq!(select_base_repo(Vec::new(), "github.com"), None);
    }

    #[test]
    fn base_repo_is_read_from_the_checkout() {
        use crate::repository::test_support::{git_in, init_temp_repo};

        let (_tmp, path) = init_temp_repo();
        git_in(
            &path,
            &["remote", "add", "origin", "git@github.com:me/fork.git"],
        );
        git_in(
            &path,
            &[
                "remote",
                "add",
                "upstream",
                "https://github.com/org/main.git",
            ],
        );
        let candidates = remote_candidates(&gix::open(&path).expect("repo"));
        assert_eq!(
            select_base_repo(candidates, "github.com"),
            Some(repo("github.com", "org", "main"))
        );

        git_in(&path, &["config", "remote.origin.gh-resolved", "base"]);
        // Opened directly: the shared handle cache would hide the config edit.
        let candidates = remote_candidates(&gix::open(&path).expect("repo"));
        assert_eq!(
            select_base_repo(candidates, "github.com"),
            Some(repo("github.com", "me", "fork"))
        );
    }

    #[test]
    fn api_endpoints_follow_gh_instance_rules() {
        assert_eq!(rest_prefix("github.com"), "https://api.github.com/");
        assert_eq!(
            graphql_endpoint("github.com"),
            "https://api.github.com/graphql"
        );
        assert_eq!(
            rest_prefix("ghe.example.com"),
            "https://ghe.example.com/api/v3/"
        );
        assert_eq!(
            graphql_endpoint("ghe.example.com"),
            "https://ghe.example.com/api/graphql"
        );
        assert_eq!(normalize_host("SSH.GitHub.com"), "github.com");
        assert_eq!(normalize_host("GHE.Example.com"), "ghe.example.com");
    }

    #[test]
    fn token_cache_spawns_once_per_ttl() {
        let mut cache = HashMap::new();
        let calls = AtomicUsize::new(0);
        let fetch = || {
            calls.fetch_add(1, Ordering::SeqCst);
            Some("tok".to_string())
        };
        assert_eq!(
            cached_token(&mut cache, "github.com", fetch).as_deref(),
            Some("tok")
        );
        assert_eq!(
            cached_token(&mut cache, "github.com", fetch).as_deref(),
            Some("tok")
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // Expired entry: asked again.
        if let Some(stale) = Instant::now().checked_sub(TOKEN_TTL + Duration::from_secs(1)) {
            cache.get_mut("github.com").expect("cached").fetched_at = stale;
            assert_eq!(
                cached_token(&mut cache, "github.com", fetch).as_deref(),
                Some("tok")
            );
            assert_eq!(calls.load(Ordering::SeqCst), 2);
        }
    }

    #[test]
    fn a_missing_token_is_cached_briefly() {
        let mut cache = HashMap::new();
        let calls = AtomicUsize::new(0);
        let fetch = || {
            calls.fetch_add(1, Ordering::SeqCst);
            None
        };
        assert_eq!(cached_token(&mut cache, "github.com", fetch), None);
        assert_eq!(cached_token(&mut cache, "github.com", fetch), None);
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        if let Some(stale) = Instant::now().checked_sub(MISSING_TOKEN_TTL + Duration::from_secs(1))
        {
            cache.get_mut("github.com").expect("cached").fetched_at = stale;
            assert_eq!(cached_token(&mut cache, "github.com", fetch), None);
            assert_eq!(calls.load(Ordering::SeqCst), 2);
        }
    }

    fn response(status: u16, headers: &[(&str, &str)], body: &str) -> HttpResponse {
        HttpResponse::new(
            status,
            headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            body.as_bytes().to_vec(),
        )
    }

    #[test]
    fn rate_limit_is_told_apart_from_other_failures() {
        assert!(is_rate_limited(&response(
            403,
            &[("x-ratelimit-remaining", "0")],
            r#"{"message":"API rate limit exceeded for user ID 1."}"#
        )));
        assert!(is_rate_limited(&response(
            403,
            &[("X-RateLimit-Remaining", "4999")],
            r#"{"message":"You have exceeded a secondary rate limit. Please wait a few minutes before you try again."}"#
        )));
        assert!(is_rate_limited(&response(
            429,
            &[("retry-after", "60")],
            r#"{"message":"You have exceeded a secondary rate limit."}"#
        )));
        assert!(!is_rate_limited(&response(
            403,
            &[("x-ratelimit-remaining", "4000")],
            r#"{"message":"Resource not accessible by integration"}"#
        )));
        assert!(!is_rate_limited(&response(
            404,
            &[],
            r#"{"message":"Not Found"}"#
        )));
        // A success is never a rate-limit hit, whatever the body says.
        assert!(!is_rate_limited(&response(
            200,
            &[("x-ratelimit-remaining", "0")],
            r#"{"message":"rate limit"}"#
        )));
    }

    #[test]
    fn graphql_rate_limit_errors_are_recognised() {
        assert!(is_graphql_rate_limit(&serde_json::json!({
            "type": "RATE_LIMITED",
            "message": "API rate limit exceeded for user ID 1."
        })));
        assert!(is_graphql_rate_limit(&serde_json::json!({
            "message": "You have exceeded a secondary rate limit."
        })));
        assert!(!is_graphql_rate_limit(&serde_json::json!({
            "type": "NOT_FOUND",
            "message": "Could not resolve to a Repository with the name 'o/r'."
        })));
    }

    #[test]
    fn paginated_rest_collections_follow_the_link_header() {
        let _guard = mock_guard();
        let seen = Arc::new(StdMutex::new(Vec::new()));
        let seen_in_mock = seen.clone();
        let _mock = testing::mock(move |req| {
            seen_in_mock
                .lock()
                .expect("lock")
                .push(req.url().to_string());
            let next =
                "https://api.github.com/repos/o/r/commits/abc/check-runs?per_page=100&page=2";
            if req.url().ends_with("page=2") {
                Ok(response(200, &[], r#"{"check_runs":[{"name":"b"}]}"#))
            } else {
                Ok(response(
                    200,
                    &[("link", &format!("<{next}>; rel=\"next\""))],
                    r#"{"check_runs":[{"name":"a"}]}"#,
                ))
            }
        });

        let mut client = GithubClient::with_token("github.com", "tok");
        let runs = client
            .rest_get_all(
                "repos/o/r/commits/abc/check-runs?per_page=100",
                "check_runs",
            )
            .expect("two pages");
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0]["name"], "a");
        assert_eq!(runs[1]["name"], "b");
        assert_eq!(
            seen.lock().expect("lock").as_slice(),
            [
                "https://api.github.com/repos/o/r/commits/abc/check-runs?per_page=100",
                "https://api.github.com/repos/o/r/commits/abc/check-runs?per_page=100&page=2",
            ]
        );
    }

    #[test]
    fn rest_calls_carry_the_github_headers() {
        let _guard = mock_guard();
        let _mock = testing::mock(|req| {
            assert_eq!(
                req.header_value("Accept"),
                Some("application/vnd.github+json")
            );
            assert_eq!(req.header_value("X-GitHub-Api-Version"), Some("2022-11-28"));
            assert_eq!(req.header_value("Authorization"), Some("Bearer tok"));
            Ok(response(200, &[], r#"{"statuses":[]}"#))
        });
        let mut client = GithubClient::with_token("github.com", "tok");
        assert!(client.rest_get_json("repos/o/r/commits/abc/status").is_ok());
    }

    #[test]
    fn refusals_map_to_rate_limited_and_the_rest_to_failed() {
        let _guard = mock_guard();
        let _mock = testing::mock(|req| {
            let url = req.url();
            if url.ends_with("/graphql") {
                let query = req
                    .json_body()
                    .and_then(|b| b["query"].as_str())
                    .unwrap_or("");
                return Ok(if query.contains("limited") {
                    response(
                        200,
                        &[],
                        r#"{"data":null,"errors":[{"type":"RATE_LIMITED","message":"API rate limit exceeded"}]}"#,
                    )
                } else if query.contains("missing") {
                    response(
                        200,
                        &[],
                        r#"{"data":{"repository":null},"errors":[{"type":"NOT_FOUND","message":"Could not resolve"}]}"#,
                    )
                } else {
                    response(200, &[], r#"{"data":{"repository":{"ok":true}}}"#)
                });
            }
            Ok(if url.ends_with("/limited") {
                response(
                    403,
                    &[("x-ratelimit-remaining", "0")],
                    r#"{"message":"API rate limit exceeded"}"#,
                )
            } else {
                response(404, &[], r#"{"message":"Not Found"}"#)
            })
        });

        let mut client = GithubClient::with_token("github.com", "tok");
        assert_eq!(
            client.graphql("query limited", Value::Null),
            Err(ApiError::RateLimited)
        );
        assert_eq!(
            client.graphql("query missing", Value::Null),
            Err(ApiError::Failed)
        );
        assert_eq!(
            client.graphql("query fine", Value::Null),
            Ok(serde_json::json!({"repository": {"ok": true}}))
        );
        assert_eq!(client.rest_get_json("limited"), Err(ApiError::RateLimited));
        assert_eq!(client.rest_get_json("absent"), Err(ApiError::Failed));
    }
}
