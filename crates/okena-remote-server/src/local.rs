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
use okena_workspace::persistence::config_dir;
use rand::Rng as _;
use std::path::Path;

/// Loopback host every local client connects on. `remote.json` records only the
/// port — the daemon always binds loopback for local use.
pub const LOCAL_HOST: &str = "127.0.0.1";

/// A local daemon discovered from `remote.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalDaemon {
    pub port: u16,
    /// Daemon process id (0 if the file omitted it).
    pub pid: u32,
    /// Whether the daemon negotiates TLS (dual-stack) on its port.
    pub tls: bool,
}

impl LocalDaemon {
    /// Loopback host to dial.
    pub fn host(&self) -> &'static str {
        LOCAL_HOST
    }
}

/// Parse `remote.json` from an explicit config dir. Returns `None` when the file
/// is absent, unreadable, malformed, or missing a port.
pub fn discover_in(dir: &Path) -> Option<LocalDaemon> {
    let data = std::fs::read_to_string(dir.join("remote.json")).ok()?;
    let v: serde_json::Value = serde_json::from_str(&data).ok()?;
    let port = u16::try_from(v.get("port")?.as_u64()?).ok()?;
    let pid = v.get("pid").and_then(|p| p.as_u64()).unwrap_or(0) as u32;
    let tls = v.get("tls").and_then(|t| t.as_bool()).unwrap_or(false);
    Some(LocalDaemon { port, pid, tls })
}

/// Parse `remote.json` from the user's config dir.
pub fn discover() -> Option<LocalDaemon> {
    discover_in(&config_dir())
}

/// Discover a local daemon and confirm its process is actually alive — guards
/// against a stale `remote.json` left by a crashed daemon. A recorded pid of 0
/// (unknown) is treated as "assume alive".
pub fn running_daemon() -> Option<LocalDaemon> {
    let daemon = discover()?;
    if daemon.pid == 0 || is_process_alive(daemon.pid) {
        Some(daemon)
    } else {
        None
    }
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
        assert_eq!(d, LocalDaemon { port: 19123, pid: 4242, tls: true });
        assert_eq!(d.host(), "127.0.0.1");
    }

    #[test]
    fn discover_defaults_missing_pid_and_tls() {
        let dir = temp_dir();
        std::fs::write(dir.join("remote.json"), r#"{"port": 19100}"#).unwrap();
        let d = discover_in(&dir).expect("should parse");
        assert_eq!(d, LocalDaemon { port: 19100, pid: 0, tls: false });
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
}
