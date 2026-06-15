mod commands;
mod parser;
mod register;
mod resolve;

use crate::workspace::persistence::config_dir;
use clap::Parser as _;
use parser::{
    Cli, Command, FolderCmd, PaletteCmd, ProjectCmd, ServiceCmd, SettingsCmd, SkillCmd, TermCmd,
    ThemeCmd, WorktreeCmd,
};
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
///
/// Gating: we only engage the CLI when `args[1]` is a known CLI subcommand (or
/// an explicit help request). Anything else — empty args, `--profile`,
/// `--list-profiles`, `--new-profile`, or any other GUI flag — returns `None`
/// so GUI launch and profile handling in `main.rs` stay untouched.
pub fn try_handle_cli() -> Option<i32> {
    let args: Vec<String> = std::env::args().collect();
    let first = args.get(1)?.as_str();

    // Explicit top-level help / version request → let clap render it.
    let is_help_or_version =
        matches!(first, "-h" | "--help" | "help" | "-V" | "--version");

    // Only claim the args if the first token is one of our subcommands.
    if !is_help_or_version && !parser::subcommand_names().contains(&first) {
        return None;
    }

    match Cli::try_parse_from(&args) {
        Ok(cli) => Some(dispatch(cli)),
        Err(e) => {
            // clap formats help/usage and version into the error; print it and
            // use exit code 2 for genuine parse errors (0 for --help/--version).
            let _ = e.print();
            let code = match e.kind() {
                clap::error::ErrorKind::DisplayHelp
                | clap::error::ErrorKind::DisplayVersion => 0,
                _ => 2,
            };
            Some(code)
        }
    }
}

/// Whether a command actually honors the global `--window` flag. Used only to
/// warn when `--window` is supplied to a command that ignores it (the flag is
/// global so clap accepts it everywhere, but most commands target a specific
/// terminal/project whose window is already implied).
fn command_uses_window(cmd: &Command) -> bool {
    match cmd {
        Command::Project { cmd } => matches!(
            cmd,
            ProjectCmd::Add { .. }
                | ProjectCmd::Show { .. }
                | ProjectCmd::Hide { .. }
                | ProjectCmd::Focus { .. }
        ),
        Command::Term { cmd } => {
            matches!(cmd, TermCmd::Focus { .. } | TermCmd::Fullscreen { .. })
        }
        Command::Cmd { cmd } => matches!(cmd, PaletteCmd::Run { .. }),
        _ => false,
    }
}

/// Dispatch a parsed [`Cli`] to the matching command implementation.
fn dispatch(cli: Cli) -> i32 {
    let window = cli.window.as_deref();
    if cli.window.is_some() && !command_uses_window(&cli.command) {
        eprintln!(
            "Warning: --window is ignored by this command. Only `project add/show/hide/focus` and `term focus/fullscreen` honor it."
        );
    }
    match cli.command {
        Command::Pair => commands::cli_pair(),
        Command::Health { json } => commands::cli_health(json),
        Command::State => commands::cli_state(),
        Command::Action { json } => commands::cli_action(&json),
        Command::Services { project, json } => {
            commands::cli_services(project.as_deref(), json)
        }
        Command::Service { cmd } => match cmd {
            ServiceCmd::Start { name, project, json } => {
                commands::cli_service("start", &name, project.as_deref(), json)
            }
            ServiceCmd::Stop { name, project, json } => {
                commands::cli_service("stop", &name, project.as_deref(), json)
            }
            ServiceCmd::Restart { name, project, json } => {
                commands::cli_service("restart", &name, project.as_deref(), json)
            }
        },
        Command::Whoami { json } => commands::cli_whoami(json),
        Command::Ls { json } => commands::cli_ls(json),

        Command::Project { cmd } => match cmd {
            ProjectCmd::Add {
                path,
                name,
                hidden,
                folder,
            } => commands::cli_project_add(
                &path,
                name.as_deref(),
                hidden,
                folder.as_deref(),
                window,
            ),
            ProjectCmd::Rm { project } => commands::cli_project_rm(&project),
            ProjectCmd::Show { project } => commands::cli_project_show(&project, true, window),
            ProjectCmd::Hide { project } => commands::cli_project_show(&project, false, window),
            ProjectCmd::Rename { project, name } => {
                commands::cli_project_rename(&project, &name)
            }
            ProjectCmd::Color { project, color } => {
                commands::cli_project_color(&project, &color)
            }
            ProjectCmd::Focus { project } => commands::cli_project_focus(&project, window),
        },

        Command::Worktree { cmd } => match cmd {
            WorktreeCmd::Add {
                project,
                branch,
                new_branch,
            } => commands::cli_worktree_add(&project, &branch, new_branch),
            WorktreeCmd::Rm { worktree, force } => commands::cli_worktree_rm(&worktree, force),
        },

        Command::Folder { cmd } => match cmd {
            FolderCmd::Add { name } => commands::cli_folder_add(&name),
            FolderCmd::Rm { folder } => commands::cli_folder_rm(&folder),
            FolderCmd::Rename { folder, name } => commands::cli_folder_rename(&folder, &name),
        },

        Command::Term { cmd } => match cmd {
            TermCmd::Ls { project, json } => commands::cli_term_ls(project.as_deref(), json),
            TermCmd::New { project } => commands::cli_term_new(&project),
            TermCmd::Close { terminal } => commands::cli_term_close(&terminal),
            TermCmd::Focus { terminal } => commands::cli_term_focus(&terminal, window),
            TermCmd::Rename { terminal, name } => commands::cli_term_rename(&terminal, &name),
            TermCmd::Split { terminal, direction } => {
                commands::cli_term_split(&terminal, &direction)
            }
            TermCmd::Tab { terminal } => commands::cli_term_tab(&terminal),
            TermCmd::Minimize { terminal } => commands::cli_term_minimize(&terminal),
            TermCmd::Fullscreen { terminal, off } => {
                commands::cli_term_fullscreen(&terminal, off, window)
            }
        },

        Command::Send { terminal, text } => commands::cli_send(&terminal, &text),
        Command::Run {
            wait,
            timeout,
            terminal,
            command,
        } => commands::cli_run(&terminal, &command, wait, timeout),
        Command::Key { terminal, key } => commands::cli_key(&terminal, &key),
        Command::Read { terminal, json } => commands::cli_read(&terminal, json),

        Command::Skill { cmd } => match cmd {
            SkillCmd::Show => commands::cli_skill_show(),
            SkillCmd::Install { user, project } => commands::cli_skill_install(user, project),
        },

        Command::Settings { cmd } => match cmd {
            SettingsCmd::Show { key } => commands::cli_settings_show(key.as_deref()),
            SettingsCmd::Schema => commands::cli_settings_schema(),
            SettingsCmd::Set { key, value } => commands::cli_settings_set(&key, &value),
        },
        Command::Theme { cmd } => match cmd {
            ThemeCmd::List { json } => commands::cli_theme_list(json),
            ThemeCmd::Show { id } => commands::cli_theme_show(id.as_deref()),
            ThemeCmd::Set { id } => commands::cli_theme_set(&id),
            ThemeCmd::Save { id, json, no_activate } => {
                commands::cli_theme_save(&id, json.as_deref(), !no_activate)
            }
        },
        Command::Cmd { cmd } => match cmd {
            PaletteCmd::List { json } => commands::cli_command_list(json),
            PaletteCmd::Run { name } => commands::cli_command_run(&name, window),
        },
    }
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
                && resp.status().is_success() {
                    return Ok(config.token);
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
