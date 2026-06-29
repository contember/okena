use crate::{CliConfig, discover_server, save_cli_config};
use okena_remote_server::local;
use okena_workspace::persistence::config_dir;

/// Register a CLI token by minting one against the local `remote_secret`, then
/// notifying any running server to reload. Returns the plaintext bearer token.
pub fn register() -> Result<String, String> {
    // Mint a token authorized by read access to the local 0600 `remote_secret`.
    let minted = local::mint_local_token()?;

    // Save CLI config so subsequent invocations reuse the token.
    save_cli_config(&CliConfig {
        token: minted.token.clone(),
        token_id: minted.token_id,
        registered_at: minted.created_at,
    })?;

    // Notify a running server to reload tokens (loopback-only route). A server
    // that isn't running yet will pick the token up from disk at startup.
    if let Ok(server) = discover_server()
        && let Ok((client, url)) = server.client_and_url("/v1/auth/reload") {
        let _ = client
            .post(&url)
            .timeout(std::time::Duration::from_secs(5))
            .send();
    }

    eprintln!(
        "Registered CLI access. Token saved to {}",
        config_dir().join("cli.json").display()
    );

    Ok(minted.token)
}
