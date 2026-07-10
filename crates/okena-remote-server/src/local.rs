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
pub use okena_core::process::is_process_alive;
use okena_transport::client::LocalEndpoint;
use okena_workspace::persistence::config_dir;
use rand::Rng as _;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::net::{IpAddr, Ipv4Addr};
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
    /// Whether desktop clients own this daemon's process lifetime.
    pub ui_owned: bool,
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
    let ui_owned = v.get("ui_owned").and_then(|v| v.as_bool()).unwrap_or(false);
    let local_endpoint = v
        .get("local_endpoint")
        .and_then(|value| serde_json::from_value::<LocalEndpoint>(value.clone()).ok());
    Some(LocalDaemon { port, host, pid, tls, ui_owned, local_endpoint })
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

/// Resolve daemon TCP bind addresses from an explicit CLI override or persisted
/// remote settings. Same-host access stays available unless the user explicitly
/// requested an unspecified bind such as `0.0.0.0`.
pub fn resolve_daemon_listen_addrs(
    listen_override: Option<IpAddr>,
    settings: &okena_workspace::persistence::AppSettings,
) -> Vec<IpAddr> {
    let loopback = IpAddr::V4(Ipv4Addr::LOCALHOST);

    if let Some(addr) = listen_override {
        return bind_addrs_with_loopback(addr);
    }

    if !settings.remote_server_enabled {
        return vec![loopback];
    }

    let configured = settings.remote_listen_address.trim();
    match configured.parse::<IpAddr>() {
        Ok(addr) => bind_addrs_with_loopback(addr),
        Err(_) => {
            if !configured.is_empty() {
                log::warn!(
                    "Invalid remote_listen_address in settings ({configured:?}); falling back to 127.0.0.1"
                );
            }
            vec![loopback]
        }
    }
}

fn bind_addrs_with_loopback(addr: IpAddr) -> Vec<IpAddr> {
    let loopback = IpAddr::V4(Ipv4Addr::LOCALHOST);

    if addr == loopback || addr.is_unspecified() {
        vec![addr]
    } else {
        vec![loopback, addr]
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
/// Loads or creates `remote_secret`, generates a random token in the same format
/// as `AuthStore::try_pair`, and appends its HMAC to `remote_tokens.json`
/// (`0600`).
/// An already-running daemon must be told to reload (`POST /v1/auth/reload`,
/// loopback-only); a freshly spawned daemon picks it up at startup.
pub fn mint_local_token_in(dir: &Path) -> Result<MintedToken, String> {
    let secret = auth::load_or_create_secret_in(dir)
        .map_err(|e| format!("Failed to initialize remote_secret: {e}"))?;

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
/// current exe — cargo and shipped installs place it alongside `okena`) as the
/// lighter variant. The dedicated daemon reads settings itself: same-host access
/// is always local (Unix socket + loopback), and remote bind addresses are added
/// only when the remote server setting is enabled. When that binary isn't
/// present, the UI binary runs its own `current_exe --headless` daemon — the
/// standard single-binary path. Either way the child inherits `OKENA_PROFILE`
/// from this process, so it uses the same config dir.
///
/// The caller owns the returned [`std::process::Child`]. In the UI-owned
/// lifecycle the desktop kills it when the last window closes; mint the token
/// *before* spawning so the fresh daemon loads it at startup (no reload needed).
pub fn spawn_daemon() -> std::io::Result<std::process::Child> {
    match daemon_binary_path() {
        Some(daemon) => std::process::Command::new(daemon).arg("--ui-owned").spawn(),
        None => {
            let exe = std::env::current_exe()?;
            log::info!(
                "No sibling okena-daemon binary; running the built-in `okena --headless` \
                 daemon (the single-binary path). The dedicated GPUI-free okena-daemon is \
                 the lighter variant and is used when present."
            );
            std::process::Command::new(exe)
                .args(["--headless", "--ui-owned"])
                .spawn()
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

fn wait_until_reachable_in(dir: &Path, timeout: Duration) -> Option<LocalDaemon> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(daemon) = discover_in(dir)
            && (daemon.pid == 0 || is_process_alive(daemon.pid))
            && daemon_responds_to_health(&daemon, Duration::from_millis(300))
        {
            return Some(daemon);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Probe a discovered local daemon's `/health` over its advertised transport
/// (Unix socket when present, else loopback TCP). Exposed so callers such as the
/// GUI's self-heal loop can distinguish a live-but-unreachable daemon from a
/// dead one without duplicating the transport-selection logic.
pub fn daemon_endpoint_responds(daemon: &LocalDaemon, timeout: Duration) -> bool {
    daemon_responds_to_health(daemon, timeout)
}

fn daemon_responds_to_health(daemon: &LocalDaemon, timeout: Duration) -> bool {
    let (client, url) = blocking_client_and_url(
        daemon.host(),
        daemon.port,
        "/health",
        daemon.local_endpoint.as_ref(),
    );
    matches!(
        client.get(&url).timeout(timeout).send(),
        Ok(resp) if resp.status().is_success()
    )
}

/// Result of ensuring a local daemon is available.
pub struct EnsuredDaemon {
    pub daemon: LocalDaemon,
    /// Plaintext bearer token to authenticate the client connection. `None`
    /// means the client connects over a trusted local transport.
    pub token: Option<String>,
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

/// Outcome of asking the local daemon to shut down (`POST /v1/shutdown`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShutdownOutcome {
    /// The daemon accepted immediate shutdown or armed it for the last disconnect.
    pub accepted: bool,
    /// Live client connections the daemon counted (excluding the caller, which
    /// disconnects its own loopback WS before asking).
    pub active_clients: u64,
}

/// How many times to (re)ask for shutdown, and the pause between tries. The
/// caller has just closed its own WS; a bounded retry lets the daemon notice the
/// close and decrement its live count before we conclude "another client is here".
const SHUTDOWN_MAX_ATTEMPTS: u32 = 4;
const SHUTDOWN_RETRY_DELAY: Duration = Duration::from_millis(150);

/// Ask the local daemon to shut down (`POST /v1/shutdown`). Returns whether it
/// accepted plus the live-client count it saw.
///
/// A UI-owned daemon arms shutdown while another client is connected and exits
/// after the last disconnect. A standalone daemon refuses the request.
///
/// Self-exclusion (option **a**): the caller must disconnect its OWN loopback WS
/// before calling; the daemon just counts live WS connections. The bounded retry
/// here absorbs the lag between the caller's socket close and the daemon
/// deregistering it, so "only us" reliably reads as zero. Option (b) — passing
/// the caller's own connection id to exclude — isn't available: the WS protocol
/// never tells a client its server-assigned id (see okena-core `WsOutbound`), so
/// (a) avoids inventing a new protocol message.
///
/// A transport error on the FIRST attempt is a hard error (the daemon is
/// unreachable — the caller falls back to killing it); a transport error on a
/// later attempt, after we already got a "refused" answer, is swallowed and the
/// last answer returned, so a flaky retry can never escalate into killing a
/// daemon that another client is using.
pub fn request_local_shutdown(daemon: &LocalDaemon) -> Result<ShutdownOutcome, String> {
    let (client, url) = blocking_client_and_url(
        daemon.host(),
        daemon.port,
        "/v1/shutdown",
        daemon.local_endpoint.as_ref(),
    );

    let mut outcome = ShutdownOutcome {
        accepted: false,
        active_clients: 0,
    };
    for attempt in 0..SHUTDOWN_MAX_ATTEMPTS {
        if attempt > 0 {
            std::thread::sleep(SHUTDOWN_RETRY_DELAY);
        }
        match client.post(&url).timeout(Duration::from_secs(5)).send() {
            Ok(resp) if resp.status().is_success() => {
                let body = resp
                    .json::<crate::routes::shutdown::ShutdownResponse>()
                    .map_err(|e| format!("failed to parse shutdown response: {e}"))?;
                outcome = ShutdownOutcome {
                    accepted: body.shutting_down,
                    active_clients: body.active_clients,
                };
                if outcome.accepted {
                    break;
                }
            }
            Ok(resp) => return Err(format!("shutdown endpoint returned {}", resp.status())),
            Err(_) if attempt > 0 => {
                // Already had a (refused) answer; don't let a flaky retry turn
                // into a kill. Report what we last saw.
                break;
            }
            Err(e) => return Err(format!("shutdown request failed: {e}")),
        }
    }
    Ok(outcome)
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
            && daemon_responds_to_health(&daemon, Duration::from_millis(300))
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
/// as `okena-daemon --listen 127.0.0.1` or the single-binary `okena --headless
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

fn daemon_needs_bearer_token(daemon: &LocalDaemon) -> bool {
    !matches!(daemon.local_endpoint, Some(LocalEndpoint::UnixSocket { .. }))
}

fn auth_token_for_daemon(dir: &Path, daemon: &LocalDaemon) -> Result<Option<String>, String> {
    if !daemon_needs_bearer_token(daemon) {
        return Ok(None);
    }
    let token = mint_local_token_in(dir)?.token;
    notify_auth_reload(daemon);
    Ok(Some(token))
}

/// Ensure a local daemon is reachable from an explicit config dir (testable
/// core), returning an auth token only when the transport needs bearer auth.
///
/// The patience is split by path: `attach_timeout` bounds how long an
/// already-advertised daemon may take to answer `/health`; `spawn_timeout`
/// budgets a child we just spawned to boot (workspace load happens before the
/// server binds, so boot can take several seconds under load). Keeping them
/// separate lets a caller give up on a wedged daemon quickly WITHOUT ever
/// SIGKILLing its own mid-boot child on that same short deadline.
///
/// ATTACH path — a live daemon already runs: wait until it answers `/health`,
/// then mint/reload a token only for TCP fallback. SPAWN path — none runs:
/// spawn it, wait for the advertised transport, then mint/reload only when that
/// transport needs bearer auth. We own the spawned [`std::process::Child`]
/// (`spawned = Some`), killing it on timeout or token setup failure.
pub fn ensure_local_daemon_in(
    dir: &Path,
    attach_timeout: Duration,
    spawn_timeout: Duration,
) -> Result<EnsuredDaemon, String> {
    if running_daemon_in(dir).is_some() {
        // Attach: a live pid is not enough. The daemon may still be binding
        // after a restart; do not hand an unreachable endpoint to the UI.
        if let Some(daemon) = wait_until_reachable_in(dir, attach_timeout) {
            let token = auth_token_for_daemon(dir, &daemon)?;
            return Ok(EnsuredDaemon {
                daemon,
                token,
                spawned: None,
            });
        }

        if let Some(daemon) = discover_in(dir)
            && (daemon.pid == 0 || is_process_alive(daemon.pid))
        {
            return Err(format!(
                "Local daemon pid {} did not respond to /health in time.",
                daemon.pid
            ));
        }

        // It died while we were waiting; fall through and spawn a fresh daemon.
        log::info!("Existing local daemon disappeared before it became reachable");
    }

    let mut child = spawn_daemon().map_err(|e| format!("Failed to spawn daemon: {e}"))?;
    match wait_until_reachable_in(dir, spawn_timeout) {
        Some(daemon) => {
            let spawned_this_daemon = daemon.pid == child.id();
            if !spawned_this_daemon {
                log::info!(
                    "Another daemon won startup (advertised pid {}, child pid {}); attaching",
                    daemon.pid,
                    child.id(),
                );
                let _ = child.kill();
                let _ = child.wait();
            }

            match auth_token_for_daemon(dir, &daemon) {
                Ok(token) => Ok(EnsuredDaemon {
                    daemon,
                    token,
                    spawned: spawned_this_daemon.then_some(child),
                }),
                Err(e) => {
                    if spawned_this_daemon {
                        let _ = child.kill();
                        let _ = child.wait();
                    }
                    Err(e)
                }
            }
        }
        None => {
            let _ = child.kill();
            let _ = child.wait();
            Err("Daemon did not become ready in time.".into())
        }
    }
}

/// Ensure a local daemon is reachable from the user's config dir, with caller-
/// chosen patience split by path (see [`ensure_local_daemon_in`]). The self-heal
/// recovery loop uses a short attach patience — so a wedged daemon is detected
/// (and escalated on) sooner — but the full spawn budget, so it never kills its
/// own freshly spawned child that is merely slow to boot.
pub fn ensure_local_daemon_with_timeouts(
    attach_timeout: Duration,
    spawn_timeout: Duration,
) -> Result<EnsuredDaemon, String> {
    ensure_local_daemon_in(&config_dir(), attach_timeout, spawn_timeout)
}

/// Ensure a local daemon is reachable from the user's config dir.
pub fn ensure_local_daemon() -> Result<EnsuredDaemon, String> {
    ensure_local_daemon_with_timeouts(Duration::from_secs(30), Duration::from_secs(30))
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

    fn spawn_health_server() -> u16 {
        use std::io::{Read, Write};

        let listener = std::net::TcpListener::bind((LOCAL_HOST, 0)).expect("bind health server");
        let port = listener.local_addr().expect("health server addr").port();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else {
                    break;
                };
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf);
                let body = r#"{"status":"ok","version":"test","uptime_secs":0}"#;
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });
        port
    }

    /// Stub server that answers every request with a fixed `/v1/shutdown` JSON
    /// body. Used to exercise `request_local_shutdown`'s accept/refuse handling
    /// without a live daemon (mirrors `spawn_health_server`).
    fn spawn_shutdown_server(shutting_down: bool, active_clients: u64) -> u16 {
        use std::io::{Read, Write};

        let listener = std::net::TcpListener::bind((LOCAL_HOST, 0)).expect("bind shutdown server");
        let port = listener.local_addr().expect("shutdown server addr").port();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else {
                    break;
                };
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf);
                let body = format!(
                    r#"{{"shutting_down":{shutting_down},"active_clients":{active_clients}}}"#
                );
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });
        port
    }

    fn tcp_daemon(port: u16) -> LocalDaemon {
        LocalDaemon {
            port,
            host: LOCAL_HOST.to_string(),
            pid: std::process::id(),
            tls: false,
            ui_owned: true,
            local_endpoint: None,
        }
    }

    fn remote_settings(
        enabled: bool,
        listen_address: &str,
    ) -> okena_workspace::persistence::AppSettings {
        let mut settings = okena_workspace::persistence::AppSettings::default();
        settings.remote_server_enabled = enabled;
        settings.remote_listen_address = listen_address.to_string();
        settings
    }

    #[test]
    fn resolve_daemon_listen_addrs_defaults_to_loopback_when_remote_disabled() {
        let settings = remote_settings(false, "0.0.0.0");
        assert_eq!(
            resolve_daemon_listen_addrs(None, &settings),
            vec![IpAddr::V4(Ipv4Addr::LOCALHOST)]
        );
    }

    #[test]
    fn resolve_daemon_listen_addrs_adds_loopback_for_remote_interface() {
        let remote_addr = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 10));
        let settings = remote_settings(true, "192.0.2.10");

        assert_eq!(
            resolve_daemon_listen_addrs(None, &settings),
            vec![IpAddr::V4(Ipv4Addr::LOCALHOST), remote_addr]
        );
    }

    #[test]
    fn resolve_daemon_listen_addrs_honors_unspecified_override() {
        let bind_all = IpAddr::V4(Ipv4Addr::UNSPECIFIED);
        let settings = remote_settings(false, "127.0.0.1");

        assert_eq!(resolve_daemon_listen_addrs(Some(bind_all), &settings), vec![bind_all]);
    }

    #[test]
    fn resolve_daemon_listen_addrs_falls_back_on_invalid_config() {
        let settings = remote_settings(true, "not an ip");

        assert_eq!(
            resolve_daemon_listen_addrs(None, &settings),
            vec![IpAddr::V4(Ipv4Addr::LOCALHOST)]
        );
    }

    #[test]
    fn request_shutdown_accepts_when_no_clients() {
        let port = spawn_shutdown_server(true, 0);
        let outcome = request_local_shutdown(&tcp_daemon(port)).expect("request should succeed");
        assert!(outcome.accepted);
        assert_eq!(outcome.active_clients, 0);
    }

    #[test]
    fn request_shutdown_refuses_when_clients_connected() {
        // The daemon reports a live client on every retry, so the bounded loop
        // gives up and reports the refusal instead of ever accepting.
        let port = spawn_shutdown_server(false, 2);
        let outcome = request_local_shutdown(&tcp_daemon(port)).expect("request should succeed");
        assert!(!outcome.accepted, "must not accept while a client is connected");
        assert_eq!(outcome.active_clients, 2);
    }

    #[test]
    fn request_shutdown_errors_when_unreachable() {
        // Port 9 (discard) refuses fast — the caller treats an unreachable daemon
        // as an error and falls back to its own kill path.
        let err = request_local_shutdown(&tcp_daemon(9)).expect_err("unreachable must error");
        assert!(err.contains("shutdown request failed"), "unexpected error: {err}");
    }

    #[test]
    fn discover_parses_port_pid_tls_and_lifecycle() {
        let dir = temp_dir();
        std::fs::write(
            dir.join("remote.json"),
            r#"{"port": 19123, "pid": 4242, "tls": true, "ui_owned": true}"#,
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
                ui_owned: true,
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
                ui_owned: false,
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
        let port = spawn_health_server();
        let secret = vec![3u8; 32];
        std::fs::write(dir.join("remote_secret"), &secret).unwrap();
        // pid = this process so the liveness check treats the daemon as alive.
        std::fs::write(
            dir.join("remote.json"),
            format!(r#"{{"port": {port}, "pid": {}, "tls": false}}"#, std::process::id()),
        )
        .unwrap();

        let ensured =
            ensure_local_daemon_in(&dir, Duration::from_millis(200), Duration::from_millis(200))
                .expect("attach should succeed");
        assert!(ensured.spawned.is_none(), "must not spawn when one is running");
        assert_eq!(ensured.daemon.port, port);

        // The returned token must validate against the secret — its HMAC has to
        // appear in remote_tokens.json (mirrors mint_writes_validatable_token).
        let token = ensured.token.as_ref().expect("TCP transport needs a token");
        let raw = std::fs::read_to_string(dir.join("remote_tokens.json")).unwrap();
        let persisted: Vec<PersistedToken> = serde_json::from_str(&raw).unwrap();
        let expected = base64::engine::general_purpose::STANDARD
            .encode(auth::compute_hmac(&secret, token.as_bytes()));
        assert!(
            persisted.iter().any(|t| t.token_hmac == expected),
            "minted token's HMAC must be persisted"
        );
    }

    #[test]
    fn unix_socket_daemon_does_not_need_token() {
        let dir = temp_dir();
        let daemon = LocalDaemon {
            port: 19100,
            host: LOCAL_HOST.to_string(),
            pid: std::process::id(),
            tls: false,
            ui_owned: true,
            local_endpoint: Some(LocalEndpoint::UnixSocket {
                path: "/tmp/okena-test.sock".to_string(),
            }),
        };

        let token = auth_token_for_daemon(&dir, &daemon).expect("trusted local transport");
        assert!(token.is_none());
        assert!(!dir.join("remote_secret").exists());
        assert!(!dir.join("remote_tokens.json").exists());
    }

    #[test]
    fn ensure_rejects_unreachable_live_daemon() {
        let dir = temp_dir();
        std::fs::write(dir.join("remote_secret"), vec![3u8; 32]).unwrap();
        std::fs::write(
            dir.join("remote.json"),
            format!(r#"{{"port": 9, "pid": {}, "tls": false}}"#, std::process::id()),
        )
        .unwrap();

        // Attach patience is short but the spawn budget is long: the error must
        // arrive on the ATTACH deadline, proving the branches use their own
        // timeouts (a live-but-unreachable daemon never enters the spawn path).
        let start = Instant::now();
        let err = match ensure_local_daemon_in(
            &dir,
            Duration::from_millis(120),
            Duration::from_secs(30),
        ) {
            Ok(_) => panic!("live but unreachable daemon must not be accepted"),
            Err(err) => err,
        };
        assert!(
            err.contains("did not respond to /health"),
            "unexpected error: {err}"
        );
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "attach branch must give up on attach_timeout, not spawn_timeout"
        );
    }

    #[test]
    fn ensure_attach_succeeds_within_attach_timeout_even_with_short_spawn_budget() {
        let dir = temp_dir();
        let port = spawn_health_server();
        std::fs::write(dir.join("remote_secret"), vec![3u8; 32]).unwrap();
        std::fs::write(
            dir.join("remote.json"),
            format!(r#"{{"port": {port}, "pid": {}, "tls": false}}"#, std::process::id()),
        )
        .unwrap();

        // A healthy advertised daemon attaches under attach_timeout regardless
        // of the spawn budget (which only applies to a child we spawn).
        let ensured = ensure_local_daemon_in(
            &dir,
            Duration::from_millis(500),
            Duration::from_millis(1),
        )
        .expect("attach should succeed");
        assert!(ensured.spawned.is_none());
        assert_eq!(ensured.daemon.port, port);
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
    fn mint_creates_secret_when_absent() {
        let dir = temp_dir();
        let minted = mint_local_token_in(&dir).expect("mint should create secret");
        let secret = std::fs::read(dir.join("remote_secret")).unwrap();
        assert_eq!(secret.len(), 32);

        let raw = std::fs::read_to_string(dir.join("remote_tokens.json")).unwrap();
        let persisted: Vec<PersistedToken> = serde_json::from_str(&raw).unwrap();
        let expected = base64::engine::general_purpose::STANDARD
            .encode(auth::compute_hmac(&secret, minted.token.as_bytes()));
        assert_eq!(persisted[0].token_hmac, expected);
    }

    #[test]
    fn mint_regenerates_wrong_secret_size() {
        let dir = temp_dir();
        std::fs::write(dir.join("remote_secret"), vec![1u8; 16]).unwrap();
        mint_local_token_in(&dir).expect("mint should regenerate invalid secret");
        let secret = std::fs::read(dir.join("remote_secret")).unwrap();
        assert_eq!(secret.len(), 32);
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
