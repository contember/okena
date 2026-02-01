use hmac::{Hmac, Mac};
use parking_lot::Mutex;
use rand::Rng;
use sha2::Sha256;
use std::collections::HashMap;
use std::net::IpAddr;
use std::time::{Duration, Instant};

type HmacSha256 = Hmac<Sha256>;

/// A stored token record.
#[allow(dead_code)]
pub struct TokenRecord {
    pub id: String,
    pub token_hmac: Vec<u8>,
    pub created_at: Instant,
    pub last_used_at: Mutex<Instant>,
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
    pub fn new() -> Self {
        let app_secret = load_or_create_secret();

        Self {
            inner: Mutex::new(AuthStoreInner {
                app_secret,
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

    /// Attempt to pair with a code. Returns a bearer token on success.
    pub fn try_pair(&self, code: &str, ip: IpAddr) -> Result<String, PairError> {
        let mut inner = self.inner.lock();

        // Rate limit check
        if inner.rate_limiter.check(ip).is_err() {
            return Err(PairError::RateLimited);
        }

        // Validate code
        let valid = match &inner.current_code {
            Some(current) => {
                let now = Instant::now();
                let not_expired = now.duration_since(inner.code_created_at) < Duration::from_secs(60);
                not_expired && constant_time_eq(current.as_bytes(), code.as_bytes())
            }
            None => false,
        };

        if !valid {
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
        let record = TokenRecord {
            id: uuid::Uuid::new_v4().to_string(),
            token_hmac,
            created_at: Instant::now(),
            last_used_at: Mutex::new(Instant::now()),
            name: None,
        };
        inner.tokens.push(record);

        // Evict oldest tokens if we exceed the limit
        const MAX_TOKENS: usize = 64;
        let count = inner.tokens.len();
        if count > MAX_TOKENS {
            inner.tokens.drain(0..count - MAX_TOKENS);
        }

        // Rotate the pairing code (invalidate current one)
        inner.current_code = None;

        Ok(token)
    }

    /// Validate a bearer token. Returns true if valid.
    pub fn validate_token(&self, token: &str) -> bool {
        let inner = self.inner.lock();
        let candidate_hmac = compute_hmac(&inner.app_secret, token.as_bytes());

        for record in &inner.tokens {
            if constant_time_eq(&record.token_hmac, &candidate_hmac) {
                *record.last_used_at.lock() = Instant::now();
                return true;
            }
        }
        false
    }

    /// Get the number of paired tokens.
    #[allow(dead_code)]
    pub fn token_count(&self) -> usize {
        self.inner.lock().tokens.len()
    }

    /// Clear all stored tokens (for "disconnect all remotes" feature).
    #[allow(dead_code)]
    pub fn clear_tokens(&self) {
        self.inner.lock().tokens.clear();
    }
}

/// Pairing errors.
#[derive(Debug)]
pub enum PairError {
    InvalidCode,
    RateLimited,
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

/// Generate a pairing code: "XXXX-XXXX" from base32 alphabet (A-Z, 2-7).
fn generate_pairing_code() -> String {
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
