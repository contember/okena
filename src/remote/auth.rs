use base64::Engine as _;
use hmac::{Hmac, Mac};
use parking_lot::Mutex;
use rand::Rng;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::collections::HashMap;
use std::net::IpAddr;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

type HmacSha256 = Hmac<Sha256>;

/// Token time-to-live in seconds (30 days).
pub const TOKEN_TTL_SECS: u64 = 30 * 24 * 3600;

/// A stored token record.
#[allow(dead_code)]
pub struct TokenRecord {
    pub id: String,
    pub token_hmac: Vec<u8>,
    pub created_at: SystemTime,
    pub last_used_at: Mutex<SystemTime>,
    pub name: Option<String>,
}

/// Rate limiter state for pairing attempts.
struct RateLimiter {
    /// Per-IP attempt timestamps (IP -> list of attempt times)
    per_ip: HashMap<IpAddr, Vec<Instant>>,
    /// Global attempt timestamps
    global: Vec<Instant>,
}

impl RateLimiter {
    fn new() -> Self {
        Self {
            per_ip: HashMap::new(),
            global: Vec::new(),
        }
    }

    /// Check if a pairing attempt from this IP should be rate-limited.
    /// Returns Ok(()) if allowed, Err(()) if rate-limited.
    fn check(&mut self, ip: IpAddr) -> Result<(), ()> {
        let now = Instant::now();
        let window = Duration::from_secs(60);

        // Prune old entries
        self.global.retain(|t| now.duration_since(*t) < window);

        let ip_attempts = self.per_ip.entry(ip).or_default();
        ip_attempts.retain(|t| now.duration_since(*t) < window);

        // Check global limit: 30/min
        if self.global.len() >= 30 {
            return Err(());
        }

        // Check per-IP limit: 5/min
        if ip_attempts.len() >= 5 {
            return Err(());
        }

        // Record attempt
        self.global.push(now);
        ip_attempts.push(now);

        Ok(())
    }
}

/// Manages pairing codes, token validation, and rate limiting.
pub struct AuthStore {
    inner: Mutex<AuthStoreInner>,
}

struct AuthStoreInner {
    /// HMAC key (app secret), 32 bytes
    app_secret: Vec<u8>,
    /// Current pairing code (base32, 8 chars with dash e.g. "K7M2-9QFP")
    current_code: Option<String>,
    /// When the current code was generated
    code_created_at: Instant,
    /// Stored token records (HMAC digests only)
    tokens: Vec<TokenRecord>,
    /// Rate limiter for pairing
    rate_limiter: RateLimiter,
}

impl AuthStore {
    /// Create a new AuthStore, loading or generating the app secret.
    /// Previously issued tokens are loaded from disk so clients survive restarts.
    pub fn new() -> Self {
        let app_secret = load_or_create_secret();
        let tokens = load_tokens();

        Self {
            inner: Mutex::new(AuthStoreInner {
                app_secret,
                current_code: None,
                code_created_at: Instant::now(),
                tokens,
                rate_limiter: RateLimiter::new(),
            }),
        }
    }

    /// Create an AuthStore with a given secret (for testing).
    #[cfg(test)]
    fn with_secret(secret: Vec<u8>) -> Self {
        Self {
            inner: Mutex::new(AuthStoreInner {
                app_secret: secret,
                current_code: None,
                code_created_at: Instant::now(),
                tokens: Vec::new(),
                rate_limiter: RateLimiter::new(),
            }),
        }
    }

    /// Generate a new pairing code (or return the current one if still valid).
    /// Code format: "XXXX-XXXX" using base32 chars, 60s TTL.
    pub fn get_or_create_code(&self) -> String {
        let mut inner = self.inner.lock();
        let now = Instant::now();

        // Return existing code if still valid (60s TTL)
        if let Some(ref code) = inner.current_code {
            if now.duration_since(inner.code_created_at) < Duration::from_secs(60) {
                return code.clone();
            }
        }

        // Generate new code
        let code = generate_pairing_code();
        inner.current_code = Some(code.clone());
        inner.code_created_at = now;
        code
    }

    /// Generate a fresh pairing code unconditionally (for explicit pairing dialog).
    pub fn generate_fresh_code(&self) -> String {
        let mut inner = self.inner.lock();
        let code = generate_pairing_code();
        inner.current_code = Some(code.clone());
        inner.code_created_at = Instant::now();
        code
    }

    /// Invalidate the current pairing code (called when pairing dialog closes).
    pub fn invalidate_code(&self) {
        let mut inner = self.inner.lock();
        inner.current_code = None;
    }

    /// Returns seconds remaining for the current code, or 0 if no code/expired.
    pub fn code_remaining_secs(&self) -> u64 {
        let inner = self.inner.lock();
        match &inner.current_code {
            Some(_) => {
                let elapsed = Instant::now().duration_since(inner.code_created_at).as_secs();
                60u64.saturating_sub(elapsed)
            }
            None => 0,
        }
    }

    /// Attempt to pair with a code. Returns a bearer token on success.
    pub fn try_pair(&self, code: &str, ip: IpAddr) -> Result<String, PairError> {
        let mut inner = self.inner.lock();

        // Rate limit check
        if inner.rate_limiter.check(ip).is_err() {
            return Err(PairError::RateLimited);
        }

        // Validate code — try in-memory first, then file-based fallback
        let in_memory_valid = match &inner.current_code {
            Some(current) => {
                let now = Instant::now();
                let not_expired = now.duration_since(inner.code_created_at) < Duration::from_secs(60);
                not_expired && constant_time_eq(current.as_bytes(), code.as_bytes())
            }
            None => false,
        };

        let file_valid = if !in_memory_valid {
            check_file_pair_code(code)
        } else {
            false
        };

        if !in_memory_valid && !file_valid {
            return Err(PairError::InvalidCode);
        }

        // Generate token (32 random bytes, base64url encoded)
        let mut token_bytes = [0u8; 32];
        rand::thread_rng().fill(&mut token_bytes);
        let token = base64::Engine::encode(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD,
            &token_bytes,
        );

        // Store HMAC of the token
        let token_hmac = compute_hmac(&inner.app_secret, token.as_bytes());
        let now = SystemTime::now();
        let record = TokenRecord {
            id: uuid::Uuid::new_v4().to_string(),
            token_hmac,
            created_at: now,
            last_used_at: Mutex::new(now),
            name: None,
        };
        inner.tokens.push(record);

        // Evict oldest tokens if we exceed the limit
        const MAX_TOKENS: usize = 64;
        let count = inner.tokens.len();
        if count > MAX_TOKENS {
            inner.tokens.drain(0..count - MAX_TOKENS);
        }

        // Invalidate the code that was used
        if in_memory_valid {
            inner.current_code = None;
        }
        if file_valid {
            let _ = std::fs::remove_file(pair_code_path());
        }

        save_tokens(&inner.tokens);

        Ok(token)
    }

    /// Validate a bearer token. Returns true if valid and not expired.
    pub fn validate_token(&self, token: &str) -> bool {
        let inner = self.inner.lock();
        let candidate_hmac = compute_hmac(&inner.app_secret, token.as_bytes());
        let now = SystemTime::now();

        for record in &inner.tokens {
            if constant_time_eq(&record.token_hmac, &candidate_hmac) {
                // Check token expiration (24 hours)
                let age = now
                    .duration_since(record.created_at)
                    .unwrap_or(Duration::MAX);
                if age >= Duration::from_secs(TOKEN_TTL_SECS) {
                    return false;
                }
                *record.last_used_at.lock() = now;
                return true;
            }
        }
        false
    }

    /// List all non-expired tokens with metadata.
    pub fn list_tokens(&self) -> Vec<TokenInfo> {
        let inner = self.inner.lock();
        let now = SystemTime::now();
        inner
            .tokens
            .iter()
            .filter_map(|record| {
                let age = now
                    .duration_since(record.created_at)
                    .unwrap_or(Duration::MAX);
                if age >= Duration::from_secs(TOKEN_TTL_SECS) {
                    return None;
                }
                let created_unix = record
                    .created_at
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                let last_used_unix = record
                    .last_used_at
                    .lock()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                let expires_at = created_unix + TOKEN_TTL_SECS;
                Some(TokenInfo {
                    id: record.id.clone(),
                    created_at: created_unix,
                    last_used_at: last_used_unix,
                    expires_at,
                    name: record.name.clone(),
                })
            })
            .collect()
    }

    /// Revoke a token by ID. Returns true if the token was found and removed.
    pub fn revoke_token(&self, id: &str) -> bool {
        let mut inner = self.inner.lock();
        let before = inner.tokens.len();
        inner.tokens.retain(|r| r.id != id);
        let removed = inner.tokens.len() < before;
        if removed {
            save_tokens(&inner.tokens);
        }
        removed
    }

    /// Refresh a valid token: validate the current token, generate a new one,
    /// and keep both valid until their respective expiry times.
    pub fn refresh_token(&self, current_token: &str) -> Result<String, &'static str> {
        let mut inner = self.inner.lock();
        let candidate_hmac = compute_hmac(&inner.app_secret, current_token.as_bytes());
        let now = SystemTime::now();

        // Validate the current token
        let valid = inner.tokens.iter().any(|record| {
            constant_time_eq(&record.token_hmac, &candidate_hmac)
                && now
                    .duration_since(record.created_at)
                    .unwrap_or(Duration::MAX)
                    < Duration::from_secs(TOKEN_TTL_SECS)
        });
        if !valid {
            return Err("invalid or expired token");
        }

        // Generate a new token
        let mut token_bytes = [0u8; 32];
        rand::thread_rng().fill(&mut token_bytes);
        let new_token = base64::Engine::encode(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD,
            &token_bytes,
        );

        // Store HMAC of the new token (old token remains valid until its own expiry)
        let new_hmac = compute_hmac(&inner.app_secret, new_token.as_bytes());
        inner.tokens.push(TokenRecord {
            id: uuid::Uuid::new_v4().to_string(),
            token_hmac: new_hmac,
            created_at: now,
            last_used_at: Mutex::new(now),
            name: None,
        });

        // Evict oldest tokens if we exceed the limit
        const MAX_TOKENS: usize = 64;
        let count = inner.tokens.len();
        if count > MAX_TOKENS {
            inner.tokens.drain(0..count - MAX_TOKENS);
        }

        save_tokens(&inner.tokens);

        Ok(new_token)
    }
}

/// Information about a stored token, safe to expose to clients.
#[derive(Serialize, Clone)]
pub struct TokenInfo {
    pub id: String,
    pub created_at: u64,
    pub last_used_at: u64,
    pub expires_at: u64,
    pub name: Option<String>,
}

/// Pairing errors.
#[derive(Debug)]
pub enum PairError {
    InvalidCode,
    RateLimited,
}

/// Check a pairing code against the file-based code written by `okena pair` CLI.
/// Returns true if the file exists, was modified within 60s, and the code matches.
fn check_file_pair_code(code: &str) -> bool {
    let path = pair_code_path();
    let metadata = match std::fs::metadata(&path) {
        Ok(m) => m,
        Err(_) => return false,
    };

    // Check mtime is within 60s
    let modified = match metadata.modified() {
        Ok(t) => t,
        Err(_) => return false,
    };
    let age = std::time::SystemTime::now()
        .duration_since(modified)
        .unwrap_or(Duration::from_secs(u64::MAX));
    if age > Duration::from_secs(60) {
        return false;
    }

    let file_code = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return false,
    };

    constant_time_eq(file_code.trim().as_bytes(), code.as_bytes())
}

/// Compute HMAC-SHA256.
fn compute_hmac(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC key length is always valid");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

/// Constant-time comparison using the `subtle` crate.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    use subtle::ConstantTimeEq;
    if a.len() != b.len() {
        return false;
    }
    a.ct_eq(b).into()
}

/// Path to the file-based pairing code (written by `okena pair` CLI).
pub fn pair_code_path() -> std::path::PathBuf {
    crate::workspace::persistence::config_dir().join("pair_code")
}

/// Generate a pairing code: "XXXX-XXXX" from base32 alphabet (A-Z, 2-7).
pub fn generate_pairing_code() -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let mut rng = rand::thread_rng();
    let mut code = String::with_capacity(9);

    for i in 0..8 {
        if i == 4 {
            code.push('-');
        }
        let idx = rng.gen_range(0..ALPHABET.len());
        code.push(ALPHABET[idx] as char);
    }

    code
}

/// Path to the app secret file.
fn secret_path() -> std::path::PathBuf {
    crate::workspace::persistence::config_dir().join("remote_secret")
}

/// Load existing app secret or generate a new one.
fn load_or_create_secret() -> Vec<u8> {
    let path = secret_path();

    // Try to load existing secret
    if let Ok(data) = std::fs::read(&path) {
        if data.len() == 32 {
            return data;
        }
        log::warn!("Invalid remote_secret file (wrong size), regenerating");
    }

    // Generate new secret
    let mut secret = vec![0u8; 32];
    rand::thread_rng().fill(&mut secret[..]);

    // Persist it
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(e) = std::fs::write(&path, &secret) {
        log::error!("Failed to write remote_secret: {}", e);
    } else {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o600);
            if let Err(e) = std::fs::set_permissions(&path, perms) {
                log::warn!("Failed to set remote_secret permissions: {}", e);
            }
        }
    }

    secret
}

/// Serializable representation of a token record for disk persistence.
#[derive(Serialize, Deserialize)]
struct PersistedToken {
    id: String,
    /// Base64-encoded HMAC digest.
    token_hmac: String,
    /// Unix timestamp (seconds since epoch).
    created_at: u64,
}

/// Path to the persisted tokens file.
fn tokens_path() -> PathBuf {
    crate::workspace::persistence::config_dir().join("remote_tokens.json")
}

/// Save token records to disk, filtering out expired tokens.
fn save_tokens(tokens: &[TokenRecord]) {
    let now = SystemTime::now();
    let persisted: Vec<PersistedToken> = tokens
        .iter()
        .filter(|t| {
            now.duration_since(t.created_at).unwrap_or(Duration::MAX)
                < Duration::from_secs(TOKEN_TTL_SECS)
        })
        .map(|t| {
            let unix = t
                .created_at
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            PersistedToken {
                id: t.id.clone(),
                token_hmac: base64::engine::general_purpose::STANDARD.encode(&t.token_hmac),
                created_at: unix,
            }
        })
        .collect();

    let json = match serde_json::to_string_pretty(&persisted) {
        Ok(j) => j,
        Err(e) => {
            log::error!("Failed to serialize tokens: {}", e);
            return;
        }
    };

    let path = tokens_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(e) = std::fs::write(&path, json.as_bytes()) {
        log::error!("Failed to write remote_tokens.json: {}", e);
    } else {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o600);
            let _ = std::fs::set_permissions(&path, perms);
        }
    }
}

/// Load token records from disk, filtering out expired tokens.
fn load_tokens() -> Vec<TokenRecord> {
    let path = tokens_path();
    let data = match std::fs::read_to_string(&path) {
        Ok(d) => d,
        Err(_) => return Vec::new(),
    };

    let persisted: Vec<PersistedToken> = match serde_json::from_str(&data) {
        Ok(p) => p,
        Err(e) => {
            log::warn!("Failed to parse remote_tokens.json: {}", e);
            return Vec::new();
        }
    };

    let now = SystemTime::now();
    persisted
        .into_iter()
        .filter_map(|p| {
            let hmac_bytes = base64::engine::general_purpose::STANDARD
                .decode(&p.token_hmac)
                .ok()?;
            let created_at = SystemTime::UNIX_EPOCH + Duration::from_secs(p.created_at);
            // Filter out expired tokens
            let age = now.duration_since(created_at).unwrap_or(Duration::MAX);
            if age >= Duration::from_secs(TOKEN_TTL_SECS) {
                return None;
            }
            Some(TokenRecord {
                id: p.id,
                token_hmac: hmac_bytes,
                created_at,
                last_used_at: Mutex::new(now),
                name: None,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};
    use std::sync::LazyLock;

    /// Serialize tests that read/write the shared remote_tokens.json file.
    static TOKEN_FILE_LOCK: LazyLock<std::sync::Mutex<()>> =
        LazyLock::new(|| std::sync::Mutex::new(()));

    fn test_store() -> AuthStore {
        AuthStore::with_secret(vec![42u8; 32])
    }

    fn test_ip() -> IpAddr {
        IpAddr::V4(Ipv4Addr::LOCALHOST)
    }

    /// Helper: pair and return a valid token.
    fn pair_token(store: &AuthStore) -> String {
        let code = store.get_or_create_code();
        store.try_pair(&code, test_ip()).expect("pairing should succeed")
    }

    #[test]
    fn refresh_valid_token_returns_new_different_token() {
        let _lock = TOKEN_FILE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let store = test_store();
        let original = pair_token(&store);

        let refreshed = store.refresh_token(&original).expect("refresh should succeed");
        assert_ne!(original, refreshed, "refreshed token should differ from original");
    }

    #[test]
    fn refresh_invalid_token_returns_err() {
        let store = test_store();
        let result = store.refresh_token("totally-bogus-token");
        assert!(result.is_err(), "refreshing invalid token should fail");
    }

    #[test]
    fn both_tokens_valid_after_refresh() {
        let _lock = TOKEN_FILE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let store = test_store();
        let original = pair_token(&store);

        let refreshed = store.refresh_token(&original).expect("refresh should succeed");

        assert!(store.validate_token(&original), "original token should still be valid");
        assert!(store.validate_token(&refreshed), "refreshed token should be valid");
    }

    #[test]
    fn file_based_pair_succeeds_and_deletes_file() {
        let _lock = TOKEN_FILE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let store = test_store();
        let code = generate_pairing_code();

        // Write code to a temp file and override pair_code_path by writing directly
        let path = pair_code_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        std::fs::write(&path, &code).expect("write pair_code");

        let result = store.try_pair(&code, test_ip());
        assert!(result.is_ok(), "file-based pairing should succeed");
        assert!(!path.exists(), "pair_code file should be deleted after successful pairing");
    }

    #[test]
    fn no_in_memory_code_and_no_file_returns_invalid() {
        let _lock = TOKEN_FILE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let store = test_store();
        // No in-memory code, no file
        let _ = std::fs::remove_file(pair_code_path());

        let result = store.try_pair("ABCD-EFGH", test_ip());
        assert!(matches!(result, Err(PairError::InvalidCode)));
    }

    #[test]
    fn persisted_tokens_survive_reload() {
        let _lock = TOKEN_FILE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let secret = vec![99u8; 32];

        // Create a store, pair a token (which saves to disk)
        let store = AuthStore::with_secret(secret.clone());
        let token = pair_token(&store);
        assert!(store.validate_token(&token));

        // The try_pair call saved tokens to disk. Now load them back
        // into a fresh store with the same secret.
        let loaded_tokens = load_tokens();
        assert!(!loaded_tokens.is_empty(), "tokens should load from disk");

        let store2 = AuthStore {
            inner: Mutex::new(AuthStoreInner {
                app_secret: secret,
                current_code: None,
                code_created_at: Instant::now(),
                tokens: loaded_tokens,
                rate_limiter: RateLimiter::new(),
            }),
        };

        assert!(
            store2.validate_token(&token),
            "token should validate against reloaded store"
        );
    }

    #[test]
    fn list_tokens_returns_non_expired() {
        let _lock = TOKEN_FILE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let store = test_store();
        let _token1 = pair_token(&store);
        // Generate a fresh code for second pairing
        let code2 = store.generate_fresh_code();
        let _token2 = store.try_pair(&code2, test_ip()).expect("second pairing should succeed");

        let tokens = store.list_tokens();
        assert_eq!(tokens.len(), 2, "should list 2 paired tokens");

        // Each token should have valid metadata
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        for info in &tokens {
            assert!(!info.id.is_empty());
            assert!(info.created_at <= now);
            assert!(info.last_used_at <= now);
            assert!(info.expires_at > now);
        }
    }

    #[test]
    fn revoke_token_removes_by_id() {
        let _lock = TOKEN_FILE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let store = test_store();
        let token1 = pair_token(&store);
        let code2 = store.generate_fresh_code();
        let token2 = store.try_pair(&code2, test_ip()).expect("second pairing should succeed");

        let tokens = store.list_tokens();
        assert_eq!(tokens.len(), 2);

        // Revoke the first token by ID
        let id_to_revoke = tokens[0].id.clone();
        assert!(store.revoke_token(&id_to_revoke), "revoke should return true");

        let remaining = store.list_tokens();
        assert_eq!(remaining.len(), 1, "should have 1 token after revoke");
        assert_ne!(remaining[0].id, id_to_revoke, "revoked token should be gone");

        // The revoked token should fail validation, the other should pass
        // (We don't know which token maps to which ID, so just verify counts)
        let valid_count = [&token1, &token2]
            .iter()
            .filter(|t| store.validate_token(t))
            .count();
        assert_eq!(valid_count, 1, "exactly one token should still be valid");
    }

    #[test]
    fn revoke_nonexistent_returns_false() {
        let store = test_store();
        assert!(!store.revoke_token("nonexistent-id"), "revoking nonexistent token should return false");
    }

    #[test]
    fn save_load_round_trip_filters_expired() {
        let _lock = TOKEN_FILE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let now = SystemTime::now();
        let expired_time = now - Duration::from_secs(TOKEN_TTL_SECS + 1);
        let valid_time = now - Duration::from_secs(100);

        let tokens = vec![
            TokenRecord {
                id: "expired".to_string(),
                token_hmac: vec![1, 2, 3],
                created_at: expired_time,
                last_used_at: Mutex::new(now),
                name: None,
            },
            TokenRecord {
                id: "valid".to_string(),
                token_hmac: vec![4, 5, 6],
                created_at: valid_time,
                last_used_at: Mutex::new(now),
                name: None,
            },
        ];

        save_tokens(&tokens);
        let loaded = load_tokens();

        assert_eq!(loaded.len(), 1, "only non-expired token should survive");
        assert_eq!(loaded[0].id, "valid");
        assert_eq!(loaded[0].token_hmac, vec![4, 5, 6]);
    }
}
