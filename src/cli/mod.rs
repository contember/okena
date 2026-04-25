mod commands;
mod register;

use crate::workspace::persistence::config_dir;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// CLI config stored in `~/.config/okena/cli.json`.
#[derive(Serialize, Deserialize)]
pub struct CliConfig {
    pub token: String,
    pub token_id: String,
    pub registered_at: u64,
}

/// Try to handle a CLI subcommand. Returns `Some(exit_code)` if a subcommand
/// was matched (caller should exit), or `None` to continue with GUI startup.
pub fn try_handle_cli() -> Option<i32> {
    let args: Vec<String> = std::env::args().collect();
    let subcommand = args.get(1)?.as_str();

    let rest = &args[2..];

    let code = match subcommand {
        "pair" => commands::cli_pair(),
        "health" => commands::cli_health(rest),
        "state" => commands::cli_state(),
        "action" => {
            let json = args.get(2).map(|s| s.as_str());
            commands::cli_action(json)
        }
        "services" => commands::cli_services(rest),
        "service" => commands::cli_service(rest),
        "whoami" => commands::cli_whoami(rest),
        "--help" | "-h" | "help" => {
            print_help();
            0
        }
        _ => return None,
    };

    Some(code)
}

fn print_help() {
    eprintln!("Usage: okena [--profile <id>] <command> [args]");
    eprintln!();
    eprintln!("Profile flags:");
    eprintln!("  --profile <id>                     Use the named profile");
    eprintln!("  --list-profiles                    List all profiles and exit");
    eprintln!("  --new-profile <name>               Create a new profile and launch with it");
    eprintln!();
    eprintln!("Commands:");
    eprintln!("  state                              Print workspace state (JSON)");
    eprintln!("  action <json>                      Execute a raw action (JSON ActionRequest)");
    eprintln!("  services [project] [--json]        List services and their status");
    eprintln!("  service start <name> [project]     Start a service");
    eprintln!("  service stop <name> [project]      Stop a service");
    eprintln!("  service restart <name> [project]   Restart a service");
    eprintln!("  whoami [--json]                    Identify current terminal and project");
    eprintln!("  health [--json]                    Server health check");
    eprintln!("  pair                               Generate a pairing code for remote clients");
    eprintln!();
    eprintln!("Default output is tab-separated (grep/awk friendly).");
    eprintln!("Use --json for structured JSON output.");
    eprintln!("Authentication is automatic on first use.");
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn cli_config_path() -> PathBuf {
    config_dir().join("cli.json")
}

fn load_cli_config() -> Option<CliConfig> {
    let data = std::fs::read_to_string(cli_config_path()).ok()?;
    serde_json::from_str(&data).ok()
}

fn save_cli_config(config: &CliConfig) -> Result<(), String> {
    let path = cli_config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("Failed to create config dir: {e}"))?;
    }
    let json =
        serde_json::to_string_pretty(config).map_err(|e| format!("Failed to serialize: {e}"))?;
    std::fs::write(&path, json.as_bytes())
        .map_err(|e| format!("Failed to write cli.json: {e}"))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        let _ = std::fs::set_permissions(&path, perms);
    }

    Ok(())
}

/// Discover a running Okena instance by reading `remote.json`.
/// Returns `(host, port)`.
fn discover_server() -> Result<(String, u16), String> {
    let path = config_dir().join("remote.json");
    let data =
        std::fs::read_to_string(&path).map_err(|_| "Okena is not running (no remote.json).")?;
    let json: serde_json::Value =
        serde_json::from_str(&data).map_err(|_| "Invalid remote.json.")?;

    let port = json
        .get("port")
        .and_then(|v| v.as_u64())
        .ok_or("Missing port in remote.json.")? as u16;

    let pid = json.get("pid").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    if pid != 0 && !is_process_alive(pid) {
        return Err("Okena is not running (stale remote.json).".to_string());
    }

    Ok(("127.0.0.1".to_string(), port))
}

fn is_process_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        unsafe { libc::kill(pid as i32, 0) == 0 }
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        true
    }
}

/// Ensure we have a valid token, auto-registering if needed.
/// Returns the bearer token string.
fn ensure_token() -> Result<String, String> {
    // Try existing token
    if let Some(config) = load_cli_config() {
        // Quick validation: try an authenticated request
        if let Ok((host, port)) = discover_server() {
            let url = format!("http://{}:{}/v1/tokens", host, port);
            let client = reqwest::blocking::Client::new();
            if let Ok(resp) = client
                .get(&url)
                .header("Authorization", format!("Bearer {}", config.token))
                .timeout(std::time::Duration::from_secs(5))
                .send()
            {
                if resp.status().is_success() {
                    return Ok(config.token);
                }
            }
        }
    }

    // Token missing or invalid — register
    register::register()
}

fn api_get(path: &str, token: &str) -> Result<String, String> {
    let (host, port) = discover_server()?;
    let url = format!("http://{}:{}{}", host, port, path);
    let client = reqwest::blocking::Client::new();
    let resp = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", token))
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .map_err(|e| format!("Request failed: {e}"))?;

    if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
        return Err("Token expired or revoked. Delete ~/.config/okena/cli.json and retry.".into());
    }
    if !resp.status().is_success() {
        return Err(format!("Server returned {}", resp.status()));
    }

    resp.text().map_err(|e| format!("Failed to read body: {e}"))
}

fn api_post(path: &str, token: &str, body: &str) -> Result<String, String> {
    let (host, port) = discover_server()?;
    let url = format!("http://{}:{}{}", host, port, path);
    let client = reqwest::blocking::Client::new();
    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .body(body.to_string())
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .map_err(|e| format!("Request failed: {e}"))?;

    if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
        return Err("Token expired or revoked. Delete ~/.config/okena/cli.json and retry.".into());
    }
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        return Err(format!("Server returned {}: {}", status, body));
    }

    resp.text().map_err(|e| format!("Failed to read body: {e}"))
}
