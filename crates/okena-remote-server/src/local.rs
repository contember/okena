//! Client-side helpers for discovering and authenticating to a *local* Okena
//! daemon over loopback.
//!
//! Local-trust model: any process that can read the `0600` `remote_secret` in
//! the user's config dir already shares the user's filesystem trust boundary, so
//! it is authorized to mint a bearer token directly (write its HMAC into
//! `remote_tokens.json`) — no interactive pairing-code dance, which exists only
//! to bootstrap trust with *off-host* clients. A same-host UI or CLI uses this
//! to attach to the daemon transparently.
//!
//! The core functions are parameterized by config dir (`*_in`) so they unit-test
//! against a temp directory; thin wrappers bind them to
//! [`okena_workspace::persistence::config_dir`].

use crate::auth::{self, PersistedToken};
use base64::Engine as _;
use okena_transport::client::LocalEndpoint;
use okena_workspace::persistence::config_dir;
use rand::Rng as _;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Default loopback host for local clients. Newer `remote.json` files can
/// override this with `local_host` when the daemon only has an IPv6 local TCP
/// endpoint.
pub const LOCAL_HOST: &str = "127.0.0.1";

/// A local daemon discovered from `remote.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalDaemon {
    pub port: u16,
    /// Loopback host to dial for TCP clients.
    pub host: String,
    /// Daemon process id (0 if the file omitted it).
    pub pid: u32,
    /// Whether the daemon negotiates TLS (dual-stack) on its port.
    pub tls: bool,
    pub local_endpoint: Option<LocalEndpoint>,
}

impl LocalDaemon {
    /// Loopback host to dial.
    pub fn host(&self) -> &str {
        &self.host
    }
}

/// Parse `remote.json` from an explicit config dir. Returns `None` when the file
/// is absent, unreadable, malformed, or missing a port.
pub fn discover_in(dir: &Path) -> Option<LocalDaemon> {
    let data = std::fs::read_to_string(dir.join("remote.json")).ok()?;
    let v: serde_json::Value = serde_json::from_str(&data).ok()?;
    let port = u16::try_from(v.get("port")?.as_u64()?).ok()?;
    let host = v
        .get("local_host")
        .and_then(|h| h.as_str())
        .filter(|h| !h.is_empty())
        .unwrap_or(LOCAL_HOST)
        .to_string();
    let pid = v.get("pid").and_then(|p| p.as_u64()).unwrap_or(0) as u32;
    let tls = v.get("tls").and_then(|t| t.as_bool()).unwrap_or(false);
    let local_endpoint = v
        .get("local_endpoint")
        .and_then(|value| serde_json::from_value::<LocalEndpoint>(value.clone()).ok());
    Some(LocalDaemon { port, host, pid, tls, local_endpoint })
}

pub fn default_local_endpoint() -> Option<LocalEndpoint> {
    #[cfg(unix)]
    {
        Some(LocalEndpoint::UnixSocket {
            path: default_unix_socket_path(&config_dir()).to_string_lossy().into_owned(),
        })
    }
    #[cfg(not(unix))]
    {
        None
    }
}

#[cfg(unix)]
pub fn default_unix_socket_path(dir: &Path) -> PathBuf {
    let key = profile_key(dir);
    runtime_dir().join("okena").join(format!("{key}.sock"))
}

#[cfg(unix)]
fn profile_key(dir: &Path) -> String {
    let mut hasher = Sha256::new();
    hasher.update(dir.to_string_lossy().as_bytes());
    let hash = hasher.finalize();
    hash.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

#[cfg(unix)]
fn runtime_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("TMPDIR").map(PathBuf::from))
        .unwrap_or_else(std::env::temp_dir)
}

/// Parse `remote.json` from the user's config dir.
pub fn discover() -> Option<LocalDaemon> {
    discover_in(&config_dir())
}

/// Discover a live daemon from an explicit config dir (testable core). Confirms
/// the recorded process is actually alive — guards against a stale `remote.json`
/// left by a crashed daemon. A recorded pid of 0 (unknown) is "assume alive".
pub fn running_daemon_in(dir: &Path) -> Option<LocalDaemon> {
    let daemon = discover_in(dir)?;
    if daemon.pid == 0 || is_process_alive(daemon.pid) {
        Some(daemon)
    } else {
        None
    }
}

/// Discover a local daemon and confirm its process is actually alive — guards
/// against a stale `remote.json` left by a crashed daemon. A recorded pid of 0
/// (unknown) is treated as "assume alive".
pub fn running_daemon() -> Option<LocalDaemon> {
    running_daemon_in(&config_dir())
}

/// Check whether a process with the given pid is still running.
pub fn is_process_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // signal 0 probes for existence without delivering a signal.
        unsafe { libc::kill(pid as i32, 0) == 0 }
    }
    #[cfg(not(unix))]
    {
        // Without a cheap liveness probe, assume alive — the caller falls back
        // to a connection attempt, which fails fast if the daemon is gone.
        let _ = pid;
        true
    }
}

/// A freshly minted local bearer token plus the metadata a caller needs to
/// persist its own record of it (e.g. the CLI's `cli.json`).
#[derive(Debug, Clone)]
pub struct MintedToken {
    /// Plaintext bearer token — send as `Authorization: Bearer <token>`.
    pub token: String,
    /// Server-side record id (for later revocation).
    pub token_id: String,
    /// Unix seconds the token was created.
    pub created_at: u64,
}

/// Mint a local bearer token in an explicit config dir (testable core).
///
/// Reads `remote_secret`, generates a random token in the same format as
/// `AuthStore::try_pair`, and appends its HMAC to `remote_tokens.json` (`0600`).
/// An already-running daemon must be told to reload (`POST /v1/auth/reload`,
/// loopback-only); a freshly spawned daemon picks it up at startup.
pub fn mint_local_token_in(dir: &Path) -> Result<MintedToken, String> {
    let secret = std::fs::read(dir.join("remote_secret"))
        .map_err(|_| "No Okena config found. Has Okena been started at least once?".to_string())?;
    if secret.len() != 32 {
        return Err("Invalid remote_secret (wrong size).".into());
    }

    // Random 32-byte token, base64url (no pad) — matches AuthStore::try_pair.
    let mut token_bytes = [0u8; 32];
    rand::thread_rng().fill(&mut token_bytes);
    let token = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(token_bytes);

    let token_hmac = auth::compute_hmac(&secret, token.as_bytes());
    let token_hmac_b64 = base64::engine::general_purpose::STANDARD.encode(&token_hmac);

    let tokens_path = dir.join("remote_tokens.json");
    let mut persisted: Vec<PersistedToken> = std::fs::read_to_string(&tokens_path)
        .ok()
        .and_then(|data| serde_json::from_str(&data).ok())
        .unwrap_or_default();

    let token_id = uuid::Uuid::new_v4().to_string();
    let created_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    persisted.push(PersistedToken {
        id: token_id.clone(),
        token_hmac: token_hmac_b64,
        created_at,
    });

    let json = serde_json::to_string_pretty(&persisted)
        .map_err(|e| format!("Failed to serialize tokens: {e}"))?;
    if let Some(parent) = tokens_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(&tokens_path, json.as_bytes())
        .map_err(|e| format!("Failed to write remote_tokens.json: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tokens_path, std::fs::Permissions::from_mode(0o600));
    }

    Ok(MintedToken {
        token,
        token_id,
        created_at,
    })
}

/// Mint a local bearer token in the user's config dir.
pub fn mint_local_token() -> Result<MintedToken, String> {
    mint_local_token_in(&config_dir())
}

/// Resolve the dedicated `okena-daemon` binary as a sibling of the current
/// executable. `None` if it can't be located (caller falls back to
/// `current_exe --headless`). Honors the platform executable suffix.
fn daemon_binary_path() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let name = if cfg!(windows) { "okena-daemon.exe" } else { "okena-daemon" };
    let path = dir.join(name);
    path.exists().then_some(path)
}

/// Spawn a local daemon and writes `remote.json`.
///
/// Prefers the dedicated, GPUI-free `okena-daemon` binary (a sibling of the
/// current exe — cargo and shipped installs place it alongside `okena`). The
/// dedicated daemon reads settings itself: same-host access is always local
/// (Unix socket + loopback), and remote bind addresses are added only when the
/// remote server setting is enabled. Falls back to
/// `current_exe --headless` when that binary isn't present, so the UI-owned
/// lifecycle still works during development. Either way the child inherits
/// `OKENA_PROFILE` from this process, so it uses the same config dir.
///
/// The caller owns the returned [`std::process::Child`]. In the UI-owned
/// lifecycle the desktop kills it when the last window closes; mint the token
/// *before* spawning so the fresh daemon loads it at startup (no reload needed).
pub fn spawn_daemon() -> std::io::Result<std::process::Child> {
    match daemon_binary_path() {
        Some(daemon) => std::process::Command::new(daemon).spawn(),
        None => {
            let exe = std::env::current_exe()?;
            std::process::Command::new(exe).arg("--headless").spawn()
        }
    }
}

/// Poll an explicit config dir until a live daemon is discoverable or `timeout`
/// elapses (testable core). Polls every 50ms.
pub fn wait_until_ready_in(dir: &Path, timeout: Duration) -> Option<LocalDaemon> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(daemon) = discover_in(dir)
            && (daemon.pid == 0 || is_process_alive(daemon.pid))
        {
            return Some(daemon);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Poll the user's config dir until a live daemon appears or `timeout` elapses.
/// Used after [`spawn_daemon`] to wait for the daemon to bind + advertise.
pub fn wait_until_ready(timeout: Duration) -> Option<LocalDaemon> {
    wait_until_ready_in(&config_dir(), timeout)
}

/// Result of ensuring a local daemon is available.
pub struct EnsuredDaemon {
    pub daemon: LocalDaemon,
    /// Plaintext bearer token to authenticate the client connection.
    pub token: String,
    /// `Some` ONLY when we spawned the daemon in this call. UI-owned lifecycle:
    /// the caller kills only what it spawned; never kill a daemon we merely attached to.
    pub spawned: Option<std::process::Child>,
}

/// Best-effort: tell an already-running daemon to reload its token file.
/// A freshly spawned daemon reads tokens at startup, so this is only needed on the
/// attach path. Failures are ignored — the worst case is the caller's connection
/// attempt fails and retries.
pub fn notify_auth_reload(daemon: &LocalDaemon) {
    let (client, url) = blocking_client_and_url(
        daemon.host(),
        daemon.port,
        "/v1/auth/reload",
        daemon.local_endpoint.as_ref(),
    );
    let _ = client
        .post(&url)
        .timeout(Duration::from_secs(5))
        .send();
}

/// Ask the local daemon at `host:port` to restart itself (`POST /v1/restart`),
/// then block until the replacement daemon advertises a live endpoint.
///
/// Pure blocking I/O — call it off any UI/async reactor thread. Returns the
/// discovered [`LocalDaemon`] (with its possibly-NEW port: the old one can linger
/// in TIME_WAIT, so the replacement may bind a different one) or an error string.
///
/// Sequence:
/// 1. Snapshot the outgoing daemon's pid from `remote.json` (so we can wait for
///    its exit — that's what frees the lock + port for the replacement).
/// 2. POST `/v1/restart`. A non-success status is a hard error; a transport
///    error is treated as "it's already going down" (it acks then exits, so the
///    connection can drop mid-response) and we proceed.
/// 3. Wait for the old pid to die (bounded), then poll `remote.json` until a LIVE
///    daemon advertises — [`wait_until_ready`] rejects a stale file whose pid is
///    dead, so it returns only once the replacement has written its own pid+port.
pub fn restart_local_daemon(
    host: &str,
    port: u16,
    local_endpoint: Option<&LocalEndpoint>,
) -> Result<LocalDaemon, String> {
    let old_pid = running_daemon().map(|d| d.pid).unwrap_or(0);

    let (client, url) = blocking_client_and_url(host, port, "/v1/restart", local_endpoint);
    match client
        .post(&url)
        .timeout(Duration::from_secs(10))
        .send()
    {
        Ok(resp) if resp.status().is_success() => {}
        Ok(resp) => return Err(format!("restart endpoint returned {}", resp.status())),
        // Transport error: the daemon may already be tearing down. The discovery
        // poll below is the real readiness signal, so log and keep going.
        Err(e) => log::warn!("restart POST error (daemon likely exiting): {e}"),
    }

    if old_pid != 0 && !wait_for_pid_exit(old_pid, Duration::from_secs(10)) {
        return Err(format!(
            "outgoing daemon pid {old_pid} did not exit before timeout"
        ));
    }

    wait_until_ready_replacing(old_pid, Duration::from_secs(15))
        .ok_or_else(|| "daemon did not come back in time".to_string())
}

/// Poll until the replacement daemon appears. Do not accept the stale
/// `remote.json` that the outgoing process leaves behind while it is exiting.
fn wait_until_ready_replacing(old_pid: u32, timeout: Duration) -> Option<LocalDaemon> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(daemon) = discover()
            && (old_pid == 0 || daemon.pid != old_pid)
            && (daemon.pid == 0 || is_process_alive(daemon.pid))
        {
            return Some(daemon);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// CLI flag the restart relauncher passes to the replacement daemon so it waits
/// for the outgoing daemon's process to exit (releasing its port + instance lock)
/// before it tries to bind / acquire the lock. See [`spawn_replacement_daemon`]
/// and [`wait_for_pid_exit`].
pub const AWAIT_PID_FLAG: &str = "--await-pid";

/// Block until the process `pid` is no longer alive, or `timeout` elapses. Polls
/// every 50ms. A `pid` of 0 (unknown) returns immediately.
///
/// Used by the replacement daemon spawned during a self-restart: the outgoing
/// daemon spawns it and then exits, so the replacement must wait for that exit
/// before [`acquire_instance_lock`](okena_workspace::persistence::acquire_instance_lock)
/// (which fails fast against a live PID) and before its port scan (the old port
/// may linger in TIME_WAIT, but binding succeeds once the old socket is gone).
pub fn wait_for_pid_exit(pid: u32, timeout: Duration) -> bool {
    if pid == 0 {
        return true;
    }
    let deadline = Instant::now() + timeout;
    loop {
        if !is_process_alive(pid) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Parse the `--await-pid <pid>` flag the restart relauncher injects into the
/// replacement daemon's args. Returns `None` when the flag is absent or its
/// value is missing/unparseable (the caller then just boots normally).
pub fn parse_await_pid<I, S>(args: I) -> Option<u32>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        if arg.as_ref() == AWAIT_PID_FLAG {
            return iter.next().and_then(|v| v.as_ref().parse::<u32>().ok());
        }
    }
    None
}

/// Spawn a fresh daemon process to *replace* the current one, then leave it to
/// the caller to exit the current process.
///
/// Re-launches the current executable with the same args (so a daemon launched
/// as `okena-daemon --listen 127.0.0.1` or the transitional `okena --headless
/// --listen 127.0.0.1` re-launches identically), with `OKENA_PROFILE` inherited
/// from the environment so the replacement lands in the same config dir. Appends
/// `--await-pid <current_pid>` so the replacement waits for THIS process to exit
/// — releasing its port + instance lock — before binding / locking. Any existing
/// `--await-pid` pair is stripped first so it isn't passed twice.
///
/// Mirrors the updater's restart pattern (`installer::restart_app`): spawn the
/// successor, then quit. The child is detached (its handle is dropped); on Unix
/// it survives as an orphan, on Windows as an independent process.
pub fn spawn_replacement_daemon() -> std::io::Result<std::process::Child> {
    let exe = std::env::current_exe()?;
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    strip_await_pid_args(&mut args);
    let my_pid = std::process::id();
    std::process::Command::new(exe)
        .args(&args)
        .arg(AWAIT_PID_FLAG)
        .arg(my_pid.to_string())
        .spawn()
}

/// Remove any `--await-pid <pid>` pair from `args` so a chain of restarts never
/// accumulates duplicate flags (the latest restart re-appends the current pid).
fn strip_await_pid_args(args: &mut Vec<String>) {
    let mut i = 0;
    while i < args.len() {
        if args[i] == AWAIT_PID_FLAG {
            args.remove(i);
            if i < args.len() {
                args.remove(i);
            }
        } else {
            i += 1;
        }
    }
}

/// Ensure a local daemon is reachable from an explicit config dir (testable
/// core), returning a token to authenticate against it.
///
/// ATTACH path — a live daemon already runs: mint a token and tell it to reload,
/// leaving `spawned = None` (we don't own its lifecycle). SPAWN path — none runs:
/// mint the token *first* so the fresh daemon loads it at startup, spawn it, and
/// wait for it to advertise. We own the spawned [`std::process::Child`]
/// (`spawned = Some`), killing it on timeout.
pub fn ensure_local_daemon_in(
    dir: &Path,
    spawn_timeout: Duration,
) -> Result<EnsuredDaemon, String> {
    if let Some(daemon) = running_daemon_in(dir) {
        // Attach: an already-running daemon must be told to reload the new token.
        let token = mint_local_token_in(dir)?.token;
        notify_auth_reload(&daemon);
        return Ok(EnsuredDaemon {
            daemon,
            token,
            spawned: None,
        });
    }

    // Spawn: mint before spawning so the fresh daemon loads the token at startup.
    let token = mint_local_token_in(dir)?.token;
    let mut child = spawn_daemon().map_err(|e| format!("Failed to spawn daemon: {e}"))?;
    match wait_until_ready_in(dir, spawn_timeout) {
        Some(daemon) => Ok(EnsuredDaemon {
            daemon,
            token,
            spawned: Some(child),
        }),
        None => {
            let _ = child.kill();
            Err("Daemon did not become ready in time.".into())
        }
    }
}

/// Ensure a local daemon is reachable from the user's config dir, returning a
/// token to authenticate against it.
pub fn ensure_local_daemon() -> Result<EnsuredDaemon, String> {
    ensure_local_daemon_in(&config_dir(), Duration::from_secs(10))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalPairCode {
    pub code: String,
    pub expires_in: u64,
}

#[derive(Deserialize)]
struct PairCodeResponse {
    code: String,
    expires_in: u64,
}

pub fn request_pair_code(
    host: &str,
    port: u16,
    token: &str,
    local_endpoint: Option<&LocalEndpoint>,
) -> Result<LocalPairCode, String> {
    let (client, url) = blocking_client_and_url(host, port, "/v1/pair-code", local_endpoint);
    let resp = client
        .post(&url)
        .bearer_auth(token)
        .timeout(Duration::from_secs(5))
        .send()
        .map_err(|e| format!("Failed to request pairing code: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        return Err(format!("Pairing code request returned {status}: {body}"));
    }

    let body = resp
        .json::<PairCodeResponse>()
        .map_err(|e| format!("Failed to parse pairing code response: {e}"))?;
    Ok(LocalPairCode {
        code: body.code,
        expires_in: body.expires_in,
    })
}

pub fn invalidate_pair_code(
    host: &str,
    port: u16,
    token: &str,
    local_endpoint: Option<&LocalEndpoint>,
) {
    let (client, url) = blocking_client_and_url(host, port, "/v1/pair-code", local_endpoint);
    let _ = client
        .delete(&url)
        .bearer_auth(token)
        .timeout(Duration::from_secs(5))
        .send();
}

fn blocking_client_and_url(
    host: &str,
    port: u16,
    path: &str,
    local_endpoint: Option<&LocalEndpoint>,
) -> (reqwest::blocking::Client, String) {
    #[cfg(unix)]
    if let Some(LocalEndpoint::UnixSocket { path: socket_path }) = local_endpoint {
        let client = reqwest::blocking::Client::builder()
            .unix_socket(socket_path.as_str())
            .build()
            .unwrap_or_else(|e| {
                log::error!("Failed to build Unix socket HTTP client for {socket_path}: {e}");
                reqwest::blocking::Client::new()
            });
        return (client, format!("http://okena.local{path}"));
    }

    (reqwest::blocking::Client::new(), format!("http://{host}:{port}{path}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "okena-local-test-{:?}-{}",
            std::thread::current().id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn discover_parses_port_pid_tls() {
        let dir = temp_dir();
        std::fs::write(
            dir.join("remote.json"),
            r#"{"port": 19123, "pid": 4242, "tls": true}"#,
        )
        .unwrap();
        let d = discover_in(&dir).expect("should parse");
        assert_eq!(
            d,
            LocalDaemon {
                port: 19123,
                host: LOCAL_HOST.to_string(),
                pid: 4242,
                tls: true,
                local_endpoint: None,
            }
        );
        assert_eq!(d.host(), "127.0.0.1");
    }

    #[test]
    fn discover_parses_local_host() {
        let dir = temp_dir();
        std::fs::write(
            dir.join("remote.json"),
            r#"{"port": 19123, "local_host": "::1", "pid": 4242, "tls": false}"#,
        )
        .unwrap();
        let d = discover_in(&dir).expect("should parse");
        assert_eq!(d.host(), "::1");
    }

    #[test]
    fn discover_defaults_missing_pid_and_tls() {
        let dir = temp_dir();
        std::fs::write(dir.join("remote.json"), r#"{"port": 19100}"#).unwrap();
        let d = discover_in(&dir).expect("should parse");
        assert_eq!(
            d,
            LocalDaemon {
                port: 19100,
                host: LOCAL_HOST.to_string(),
                pid: 0,
                tls: false,
                local_endpoint: None,
            }
        );
    }

    #[test]
    fn discover_parses_local_endpoint() {
        let dir = temp_dir();
        std::fs::write(
            dir.join("remote.json"),
            r#"{"port": 19123, "pid": 4242, "tls": false, "local_endpoint": {"kind": "unix_socket", "path": "/tmp/okena.sock"}}"#,
        )
        .unwrap();
        let d = discover_in(&dir).expect("should parse");
        assert_eq!(
            d.local_endpoint,
            Some(LocalEndpoint::UnixSocket {
                path: "/tmp/okena.sock".to_string(),
            })
        );
    }

    #[test]
    fn discover_none_when_absent_or_malformed() {
        let dir = temp_dir();
        assert_eq!(discover_in(&dir), None);
        std::fs::write(dir.join("remote.json"), b"{not json").unwrap();
        assert_eq!(discover_in(&dir), None);
        std::fs::write(dir.join("remote.json"), r#"{"pid": 1}"#).unwrap();
        assert_eq!(discover_in(&dir), None, "missing port is a miss");
    }

    #[test]
    fn mint_writes_validatable_token() {
        let dir = temp_dir();
        let secret = vec![7u8; 32];
        std::fs::write(dir.join("remote_secret"), &secret).unwrap();

        let minted = mint_local_token_in(&dir).expect("mint should succeed");
        assert!(!minted.token.is_empty());
        assert!(!minted.token_id.is_empty());

        // The persisted HMAC must match HMAC(secret, token) — i.e. a server with
        // this secret would validate the token.
        let raw = std::fs::read_to_string(dir.join("remote_tokens.json")).unwrap();
        let persisted: Vec<PersistedToken> = serde_json::from_str(&raw).unwrap();
        assert_eq!(persisted.len(), 1);
        let expected = base64::engine::general_purpose::STANDARD
            .encode(auth::compute_hmac(&secret, minted.token.as_bytes()));
        assert_eq!(persisted[0].token_hmac, expected);
        assert_eq!(persisted[0].id, minted.token_id);
    }

    #[test]
    fn ensure_attaches_to_running_daemon() {
        let dir = temp_dir();
        let secret = vec![3u8; 32];
        std::fs::write(dir.join("remote_secret"), &secret).unwrap();
        // pid = this process so the liveness check treats the daemon as alive;
        // the port is fake, so the reload POST just fails silently.
        std::fs::write(
            dir.join("remote.json"),
            format!(r#"{{"port": 19199, "pid": {}, "tls": false}}"#, std::process::id()),
        )
        .unwrap();

        let ensured = ensure_local_daemon_in(&dir, Duration::from_millis(200))
            .expect("attach should succeed");
        assert!(ensured.spawned.is_none(), "must not spawn when one is running");
        assert_eq!(ensured.daemon.port, 19199);

        // The returned token must validate against the secret — its HMAC has to
        // appear in remote_tokens.json (mirrors mint_writes_validatable_token).
        let raw = std::fs::read_to_string(dir.join("remote_tokens.json")).unwrap();
        let persisted: Vec<PersistedToken> = serde_json::from_str(&raw).unwrap();
        let expected = base64::engine::general_purpose::STANDARD
            .encode(auth::compute_hmac(&secret, ensured.token.as_bytes()));
        assert!(
            persisted.iter().any(|t| t.token_hmac == expected),
            "minted token's HMAC must be persisted"
        );
    }

    #[test]
    fn mint_appends_to_existing_tokens() {
        let dir = temp_dir();
        std::fs::write(dir.join("remote_secret"), vec![9u8; 32]).unwrap();
        let first = mint_local_token_in(&dir).expect("first mint");
        let second = mint_local_token_in(&dir).expect("second mint");
        assert_ne!(first.token, second.token);

        let raw = std::fs::read_to_string(dir.join("remote_tokens.json")).unwrap();
        let persisted: Vec<PersistedToken> = serde_json::from_str(&raw).unwrap();
        assert_eq!(persisted.len(), 2, "second mint appends, not overwrites");
    }

    #[test]
    fn mint_errors_without_secret() {
        let dir = temp_dir();
        assert!(mint_local_token_in(&dir).is_err());
    }

    #[test]
    fn mint_errors_on_wrong_secret_size() {
        let dir = temp_dir();
        std::fs::write(dir.join("remote_secret"), vec![1u8; 16]).unwrap();
        assert!(mint_local_token_in(&dir).is_err());
    }

    #[test]
    fn current_process_is_alive() {
        assert!(is_process_alive(std::process::id()));
    }

    #[test]
    fn parse_await_pid_reads_flag_value() {
        let args = ["--listen", "127.0.0.1", AWAIT_PID_FLAG, "4242"];
        assert_eq!(parse_await_pid(args), Some(4242));
    }

    #[test]
    fn parse_await_pid_none_when_absent_or_malformed() {
        assert_eq!(parse_await_pid(["--listen", "127.0.0.1"]), None);
        assert_eq!(parse_await_pid([AWAIT_PID_FLAG]), None, "flag without value");
        assert_eq!(parse_await_pid([AWAIT_PID_FLAG, "notanum"]), None);
    }

    #[test]
    fn strip_await_pid_removes_flag_and_value() {
        let mut args = vec![
            "--listen".to_string(),
            "127.0.0.1".to_string(),
            AWAIT_PID_FLAG.to_string(),
            "99".to_string(),
        ];
        strip_await_pid_args(&mut args);
        assert_eq!(args, vec!["--listen".to_string(), "127.0.0.1".to_string()]);
    }

    #[test]
    fn wait_for_pid_exit_zero_returns_immediately() {
        // pid 0 is "unknown"; treat as already gone so the caller doesn't block.
        assert!(wait_for_pid_exit(0, Duration::from_millis(10)));
    }

    #[test]
    fn wait_for_pid_exit_times_out_on_live_pid() {
        let start = Instant::now();
        // This very process is alive, so the wait must run to the deadline.
        assert!(!wait_for_pid_exit(std::process::id(), Duration::from_millis(120)));
        assert!(start.elapsed() < Duration::from_secs(2), "should give up near the timeout");
    }

    #[test]
    fn daemon_binary_path_is_total_and_sibling_consistent() {
        // The function must never panic and must agree with the path-derivation
        // contract: when it returns Some, the path is the sibling of current_exe
        // named per the platform suffix and actually exists on disk.
        let result = daemon_binary_path();

        let exe = std::env::current_exe().expect("current_exe in test harness");
        let dir = exe.parent().expect("current_exe has a parent");
        let name = if cfg!(windows) { "okena-daemon.exe" } else { "okena-daemon" };
        let expected = dir.join(name);

        match result {
            // Some only when the sibling exists, and it must be that exact path.
            Some(path) => {
                assert_eq!(path, expected);
                assert!(path.exists(), "Some implies the sibling exists");
            }
            // None is the correct, non-panicking answer when no sibling exists
            // (caller then falls back to current_exe --headless).
            None => assert!(!expected.exists(), "None implies no sibling on disk"),
        }
    }

    #[test]
    fn wait_returns_immediately_when_daemon_present() {
        let dir = temp_dir();
        std::fs::write(
            dir.join("remote.json"),
            format!(r#"{{"port": 19100, "pid": {}, "tls": false}}"#, std::process::id()),
        )
        .unwrap();
        let found = wait_until_ready_in(&dir, Duration::from_secs(2));
        assert_eq!(found.map(|d| d.port), Some(19100));
    }

    #[test]
    fn wait_times_out_when_absent() {
        let dir = temp_dir();
        let start = Instant::now();
        assert_eq!(wait_until_ready_in(&dir, Duration::from_millis(120)), None);
        assert!(start.elapsed() < Duration::from_secs(2), "should give up near the timeout");
    }

    #[test]
    fn wait_skips_stale_dead_pid() {
        let dir = temp_dir();
        // pid 0 is treated as "unknown -> assume alive"; use a very high pid that
        // is almost certainly dead to exercise the liveness rejection path.
        std::fs::write(
            dir.join("remote.json"),
            r#"{"port": 19100, "pid": 2147483646, "tls": false}"#,
        )
        .unwrap();
        assert_eq!(wait_until_ready_in(&dir, Duration::from_millis(120)), None);
    }
}
