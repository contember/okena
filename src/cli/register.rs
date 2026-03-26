use crate::cli::{CliConfig, discover_server, save_cli_config};
use crate::remote::auth::{self, PersistedToken};
use crate::workspace::persistence::config_dir;

/// Register a CLI token by writing directly to the token store.
/// Returns the plaintext bearer token.
pub fn register() -> Result<String, String> {
    // 1. Read the app secret
    let secret_path = auth::secret_path();
    let secret = std::fs::read(&secret_path).map_err(|_| {
        "No Okena config found. Has Okena been started at least once?".to_string()
    })?;
    if secret.len() != 32 {
        return Err("Invalid remote_secret (wrong size).".into());
    }

    // 2. Generate a random token (same format as AuthStore::try_pair)
    use rand::Rng;
    let mut token_bytes = [0u8; 32];
    rand::thread_rng().fill(&mut token_bytes);
    let token = base64::Engine::encode(
        &base64::engine::general_purpose::URL_SAFE_NO_PAD,
        &token_bytes,
    );

    // 3. Compute HMAC
    let token_hmac = auth::compute_hmac(&secret, token.as_bytes());
    let token_hmac_b64 = base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        &token_hmac,
    );

    // 4. Load existing tokens, append new one
    let tokens_path = auth::tokens_path();
    let mut persisted: Vec<PersistedToken> = if let Ok(data) = std::fs::read_to_string(&tokens_path)
    {
        serde_json::from_str(&data).unwrap_or_default()
    } else {
        Vec::new()
    };

    let token_id = uuid::Uuid::new_v4().to_string();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    persisted.push(PersistedToken {
        id: token_id.clone(),
        token_hmac: token_hmac_b64,
        created_at: now,
    });

    // 5. Write back
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
        let perms = std::fs::Permissions::from_mode(0o600);
        let _ = std::fs::set_permissions(&tokens_path, perms);
    }

    // 6. Save CLI config
    save_cli_config(&CliConfig {
        token: token.clone(),
        token_id,
        registered_at: now,
    })?;

    // 7. Notify running server to reload tokens
    if let Ok((host, port)) = discover_server() {
        let url = format!("http://{}:{}/v1/auth/reload", host, port);
        let _ = reqwest::blocking::Client::new()
            .post(&url)
            .timeout(std::time::Duration::from_secs(5))
            .send();
    }

    eprintln!("Registered CLI access. Token saved to {}", config_dir().join("cli.json").display());

    Ok(token)
}
