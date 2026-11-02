pub mod claude;
pub mod claude_remote;
pub mod commands;
pub mod parser;
pub mod register;
pub mod resolve;

use clap::Parser as _;
use okena_core::process::is_process_alive;
use okena_transport::client::{LocalEndpoint, RemoteConnectionConfig};
use okena_workspace::persistence::config_dir;
use parser::{
    ClaudeCmd, ClaudeHookCmd, ClaudeRemoteCmd, Cli, Command, FolderCmd, PaletteCmd, ProjectCmd,
    ServiceCmd, SettingsCmd, SkillCmd, TermCmd, ThemeCmd, UpdateCmd, WindowCmd, WorktreeCmd,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Top-level subcommand Claude Code runs as hooks and as an MCP channel server.
pub const CLAUDE_REMOTE_SUBCOMMAND: &str = "claude-remote";

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
/// Gating: see [`cli_args`]. Unclaimed arguments (none, GUI flags such as
/// `--profile`, `--headless`, macOS' `-psn_…`, or a path dropped onto the app)
/// return `None` so GUI launch and profile handling in `main.rs` stay untouched.
pub fn try_handle_cli() -> Option<i32> {
    let args = cli_args(&std::env::args().collect::<Vec<_>>())?;
    match Cli::try_parse_from(&args) {
        Ok(cli) => Some(dispatch(cli)),
        Err(e) => Some(print_parse_error(&e)),
    }
}

/// Exit code for arguments the CLI claims but clap rejects (an unknown or
/// mistyped subcommand), after printing clap's message; `None` otherwise.
/// `main` calls this before it touches the profile, so a command an older
/// build does not know changes nothing on disk. `--help` and `--version` are
/// clap "errors" too and come out here with exit 0.
pub fn reject_unparsable_args() -> Option<i32> {
    let args = cli_args(&std::env::args().collect::<Vec<_>>())?;
    Cli::try_parse_from(&args)
        .err()
        .map(|e| print_parse_error(&e))
}

fn print_parse_error(e: &clap::Error) -> i32 {
    // clap formats help/usage and version into the error; print it and use
    // exit code 2 for genuine parse errors (0 for --help/--version).
    let _ = e.print();
    match e.kind() {
        clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion => 0,
        _ => 2,
    }
}

/// The GUI's own flags and whether each takes a value: everything `main.rs`,
/// the daemon spawn and Okena's relaunches (restart, profile switch, revert)
/// pass to the binary. Anything else belongs to the CLI.
const GUI_FLAGS: &[(&str, bool)] = &[
    ("--profile", true),
    ("--new-profile", true),
    ("--list-profiles", false),
    ("--headless", false),
    ("--ui-owned", false),
    ("--listen", true),
    ("--remote", false),
    ("--await-pid", true),
    ("--complete-config-restore", false),
    ("--revert-target-exe", true),
];

/// The arguments for clap when `args` (as `std::env::args`, the program
/// first) are a CLI invocation, `None` when they launch the GUI.
///
/// The GUI's flags (with their values) are skipped, so `okena --profile work
/// ls` runs `ls` in profile `work` (main resolves the profile from the same
/// arguments). The first argument after them decides: nothing, an empty
/// string, `-psn_…` (older macOS launchers) or a path (a folder dropped onto
/// the app, `okena .`) launch the GUI as before; anything else is the CLI's,
/// so an unknown or mistyped subcommand, or a flag the GUI does not know,
/// fails with clap's error and exit 2 instead of starting a second instance.
pub(crate) fn cli_args(args: &[String]) -> Option<Vec<String>> {
    let mut rest = args.iter().skip(1);
    while let Some(arg) = rest.next() {
        if arg.is_empty() || arg.starts_with("-psn_") {
            continue;
        }
        let flag = arg.split_once('=').map_or(arg.as_str(), |(name, _)| name);
        if let Some(&(_, takes_value)) = GUI_FLAGS.iter().find(|(name, _)| *name == flag) {
            if takes_value && !arg.contains('=') {
                rest.next();
            }
            continue;
        }
        if looks_like_path(arg) {
            return None;
        }
        let program = args.first().cloned().unwrap_or_else(|| "okena".into());
        return Some(
            std::iter::once(program)
                .chain(std::iter::once(arg.clone()))
                .chain(rest.cloned())
                .collect(),
        );
    }
    None
}

/// A path rather than a subcommand: subcommand names never contain a path
/// separator, and `.`, `..` or a Windows drive (`C:`) are paths on their own.
fn looks_like_path(arg: &str) -> bool {
    arg.contains('/')
        || arg.contains('\\')
        || arg == "."
        || arg == ".."
        || (arg.len() == 2 && arg.ends_with(':') && arg.as_bytes()[0].is_ascii_alphabetic())
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
                | ProjectCmd::Clone { .. }
                | ProjectCmd::Show { .. }
                | ProjectCmd::Hide { .. }
                | ProjectCmd::Focus { .. }
        ),
        Command::Term { cmd } => {
            matches!(cmd, TermCmd::Focus { .. } | TermCmd::Fullscreen { .. })
        }
        Command::Worktree { cmd } => matches!(cmd, WorktreeCmd::Add { .. }),
        Command::Window { .. } => true,
        Command::Cmd { cmd } => matches!(cmd, PaletteCmd::Run { .. }),
        _ => false,
    }
}

/// Dispatch a parsed [`Cli`] to the matching command implementation.
fn dispatch(cli: Cli) -> i32 {
    let window = cli.window.as_deref();
    if cli.window.is_some() && !command_uses_window(&cli.command) {
        eprintln!(
            "Warning: --window is ignored by this command. Only `project add/clone/show/hide/focus`, `worktree add`, `term focus/fullscreen` and `window layout` honor it."
        );
    }
    match cli.command {
        Command::Pair => commands::cli_pair(),
        Command::Health { json } => commands::cli_health(json),
        Command::State => commands::cli_state(),
        Command::Action { json } => commands::cli_action(&json),
        Command::Services { project, json } => commands::cli_services(project.as_deref(), json),
        Command::Service { cmd } => match cmd {
            ServiceCmd::Start {
                name,
                project,
                json,
            } => commands::cli_service("start", &name, project.as_deref(), json),
            ServiceCmd::Stop {
                name,
                project,
                json,
            } => commands::cli_service("stop", &name, project.as_deref(), json),
            ServiceCmd::Restart {
                name,
                project,
                json,
            } => commands::cli_service("restart", &name, project.as_deref(), json),
        },
        Command::Usage { json } => commands::cli_usage(json),
        Command::Whoami { json } => commands::cli_whoami(json),
        Command::Ls { json } => commands::cli_ls(json),
        Command::Window { cmd } => match cmd {
            WindowCmd::Layout {
                mode,
                min_width,
                min_height,
            } => commands::cli_window_layout(mode.mode(), min_width, min_height, window),
        },

        Command::Project { cmd } => match cmd {
            ProjectCmd::Add {
                path,
                name,
                hidden,
                folder,
            } => {
                commands::cli_project_add(&path, name.as_deref(), hidden, folder.as_deref(), window)
            }
            ProjectCmd::Clone {
                url,
                into,
                dir,
                name,
                hidden,
                folder,
            } => commands::cli_project_clone(
                &url,
                into.as_deref(),
                dir.as_deref(),
                name.as_deref(),
                hidden,
                folder.as_deref(),
                window,
            ),
            ProjectCmd::Rm { project } => commands::cli_project_rm(&project),
            ProjectCmd::Show { project } => commands::cli_project_show(&project, true, window),
            ProjectCmd::Hide { project } => commands::cli_project_show(&project, false, window),
            ProjectCmd::Rename { project, name } => commands::cli_project_rename(&project, &name),
            ProjectCmd::Color { project, color } => commands::cli_project_color(&project, &color),
            ProjectCmd::Focus { project } => commands::cli_project_focus(&project, window),
        },

        Command::Worktree { cmd } => match cmd {
            WorktreeCmd::Add {
                project,
                branch,
                new_branch,
                wait,
                layout,
                name,
            } => commands::cli_worktree_add(
                &project,
                &branch,
                new_branch,
                window,
                commands::WorktreeAddOptions {
                    wait,
                    layout: layout.map(Into::into),
                    terminal_name: name,
                },
            ),
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
            TermCmd::Split {
                terminal,
                direction,
            } => commands::cli_term_split(&terminal, &direction),
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
            ThemeCmd::Save {
                id,
                json,
                no_activate,
            } => commands::cli_theme_save(&id, json.as_deref(), !no_activate),
        },
        Command::Cmd { cmd } => match cmd {
            PaletteCmd::List { json } => commands::cli_command_list(json),
            PaletteCmd::Run { name } => commands::cli_command_run(&name, window),
        },
        Command::Update { cmd } => match cmd {
            UpdateCmd::Status { json } => commands::cli_update_status(json),
            UpdateCmd::List { json, quiet } => commands::cli_update_list(json, quiet),
            UpdateCmd::Revert {
                version,
                keep_config,
                dry_run,
                yes,
                restart,
                json,
            } => commands::cli_update_revert(&version, keep_config, dry_run, yes, restart, json),
        },
        Command::Claude { cmd } => match cmd {
            ClaudeCmd::Ls { json } => claude::cli_ls(json),
            ClaudeCmd::Start {
                terminal,
                prompt_file,
                trust,
                wait,
                timeout,
                wait_timeout,
                json,
            } => claude::cli_start(
                &terminal,
                prompt_file.as_deref(),
                trust,
                wait.then(|| std::time::Duration::from_secs(wait_timeout)),
                timeout,
                json,
            ),
            ClaudeCmd::Show { terminal, json } => claude::cli_show(&terminal, json),
            ClaudeCmd::Find { session, json } => claude::cli_find(&session, json),
            ClaudeCmd::Subagents { terminal, json } => claude::cli_subagents(&terminal, json),
            ClaudeCmd::Log {
                terminal,
                cursor,
                agent,
                json,
            } => claude::cli_log(&terminal, cursor.as_deref(), agent.as_deref(), json),
            ClaudeCmd::Prompt {
                terminal,
                session,
                text,
            } => claude::cli_prompt(&terminal, session.as_deref(), &text),
            ClaudeCmd::Answer {
                terminal,
                dialog,
                decision,
                always,
                message,
                answers,
            } => claude::cli_answer(
                &terminal,
                &dialog,
                decision,
                always,
                message.as_deref(),
                answers.as_deref(),
            ),
            ClaudeCmd::Interrupt { terminal } => claude::cli_interrupt(&terminal),
        },
        Command::ClaudeRemote { cmd } => match cmd {
            ClaudeRemoteCmd::Hook { cmd } => match cmd {
                ClaudeHookCmd::Permission => claude_remote::cli_hook_permission(),
                ClaudeHookCmd::Event => claude_remote::cli_hook_event(),
            },
            ClaudeRemoteCmd::Channel => claude_remote::cli_channel(),
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
    std::fs::write(&path, json.as_bytes()).map_err(|e| format!("Failed to write cli.json: {e}"))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        let _ = std::fs::set_permissions(&path, perms);
    }

    Ok(())
}

/// A running Okena instance discovered from `remote.json`.
pub(crate) struct DiscoveredServer {
    pub host: String,
    pub port: u16,
    tls: bool,
    local_endpoint: Option<LocalEndpoint>,
}

impl DiscoveredServer {
    /// Whether [`Self::client_and_url`] connects over the same-user Unix
    /// socket, which the server authorizes without a bearer token.
    pub fn uses_unix_socket(&self) -> bool {
        cfg!(unix) && matches!(self.local_endpoint, Some(LocalEndpoint::UnixSocket { .. }))
    }

    pub fn client_and_url(
        &self,
        path: &str,
    ) -> Result<(reqwest::blocking::Client, String), String> {
        #[cfg(unix)]
        if let Some(LocalEndpoint::UnixSocket { path: socket_path }) = &self.local_endpoint {
            let client = reqwest::blocking::Client::builder()
                .unix_socket(socket_path.as_str())
                .build()
                .map_err(|e| format!("Cannot initialise Unix socket client: {e}"))?;
            return Ok((client, format!("http://okena.local{path}")));
        }

        let client = okena_transport::client::tls::build_blocking_reqwest_client(
            self.tls,
            None,
            okena_transport::client::tls::new_observed(),
            std::time::Duration::from_secs(10),
        )?;
        let scheme = if self.tls { "https" } else { "http" };
        Ok((
            client,
            format!("{scheme}://{}:{}{path}", self.host, self.port),
        ))
    }
}

/// Discover a running Okena instance by reading `remote.json`.
fn discover_server() -> Result<DiscoveredServer, String> {
    let path = config_dir().join("remote.json");
    let data =
        std::fs::read_to_string(&path).map_err(|_| "Okena is not running (no remote.json).")?;
    let json: serde_json::Value =
        serde_json::from_str(&data).map_err(|_| "Invalid remote.json.")?;

    let port = json
        .get("port")
        .and_then(|v| v.as_u64())
        .ok_or("Missing port in remote.json.")
        .and_then(|port| u16::try_from(port).map_err(|_| "Invalid port in remote.json."))?;
    let host = json
        .get("local_host")
        .and_then(|v| v.as_str())
        .filter(|host| !host.is_empty())
        .unwrap_or("127.0.0.1")
        .to_string();
    let local_endpoint = json
        .get("local_endpoint")
        .and_then(|value| serde_json::from_value::<LocalEndpoint>(value.clone()).ok());
    let tls = json
        .get("tls")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);

    let pid = json.get("pid").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    if pid != 0 && !is_process_alive(pid) {
        return Err("Okena is not running (stale remote.json).".to_string());
    }

    Ok(DiscoveredServer {
        host,
        port,
        tls,
        local_endpoint,
    })
}

/// Ensure we have a valid token, auto-registering if needed.
/// Returns the bearer token string.
fn ensure_token() -> Result<String, String> {
    // Try existing token
    if let Some(config) = load_cli_config() {
        // Quick validation: try an authenticated request
        if let Ok(server) = discover_server()
            && let Ok((client, url)) = server.client_and_url("/v1/tokens")
            && let Ok(resp) = client
                .get(&url)
                .header("Authorization", format!("Bearer {}", config.token))
                .timeout(std::time::Duration::from_secs(5))
                .send()
            && resp.status().is_success()
        {
            return Ok(config.token);
        }
    }

    // Token missing or invalid — register
    register::register()
}

fn api_get(path: &str, token: &str) -> Result<String, String> {
    api_request(path, token, None)
}

/// POST a JSON body to a REST route (not an `ActionRequest`).
fn api_post_json(path: &str, token: &str, body: &serde_json::Value) -> Result<String, String> {
    api_request(path, token, Some(body))
}

/// GET `path`, or POST `body` to it. A failure carries the server's plain-text
/// error body, so callers can show the daemon's reason.
fn api_request(
    path: &str,
    token: &str,
    body: Option<&serde_json::Value>,
) -> Result<String, String> {
    let server = discover_server()?;
    let (client, url) = server.client_and_url(path)?;
    let request = match body {
        Some(body) => client.post(&url).json(body),
        None => client.get(&url),
    };
    let resp = request
        .header("Authorization", format!("Bearer {}", token))
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .map_err(|e| format!("Request failed: {e}"))?;

    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Err("Token expired or revoked. Delete ~/.config/okena/cli.json and retry.".into());
    }
    if !status.is_success() {
        let message = resp.text().unwrap_or_default();
        let message = message.trim();
        return Err(if message.is_empty() {
            format!("Server returned {status}")
        } else {
            format!("Server returned {status}: {message}")
        });
    }

    resp.text().map_err(|e| format!("Failed to read body: {e}"))
}

fn api_action(token: &str, body: &str) -> Result<String, String> {
    let server = discover_server()?;
    let action: okena_core::api::ActionRequest =
        serde_json::from_str(body).map_err(|e| format!("Invalid action: {e}"))?;
    let config = RemoteConnectionConfig {
        id: okena_transport::client::LOCAL_DAEMON_CONNECTION_ID.to_string(),
        name: "Local daemon".to_string(),
        host: server.host,
        port: server.port,
        saved_token: Some(token.to_string()),
        token_obtained_at: None,
        tls: server.tls,
        pinned_cert_sha256: None,
        local_endpoint: server.local_endpoint,
    };
    match okena_transport::remote_action::RemoteActionClient::new(config, token.to_string())
        .post_action(action)?
    {
        Some(value) => {
            serde_json::to_string(&value).map_err(|e| format!("Failed to serialize response: {e}"))
        }
        None => Ok(String::new()),
    }
}

#[cfg(test)]
mod gate_tests {
    use super::cli_args;

    fn claimed(args: &[&str]) -> Option<Vec<String>> {
        let args: Vec<String> = std::iter::once("okena")
            .chain(args.iter().copied())
            .map(String::from)
            .collect();
        cli_args(&args)
    }

    #[test]
    fn the_gui_keeps_no_arguments_its_flags_and_paths() {
        for args in [
            &[][..],
            &["--profile", "work"],
            &["--profile=work"],
            &["--new-profile", "x"],
            &["--list-profiles"],
            &["--headless", "--ui-owned"],
            &["--listen", "0.0.0.0", "--remote"],
            &["--await-pid", "123"],
            &[
                "--complete-config-restore",
                "--revert-target-exe",
                "/opt/okena",
            ],
            &["-psn_0_1234567"],
            &[""],
            // A folder dropped onto the app, or `okena .` from a shell.
            &["C:\\projects\\site"],
            &["C:"],
            &["/Users/u/projects/site"],
            &["."],
            &["--profile", "work", "~/projects/site"],
        ] {
            assert_eq!(claimed(args), None, "{args:?}");
        }
    }

    #[test]
    fn everything_else_is_the_clis_after_the_gui_flags() {
        let expect = |args: &[&str], clap: &[&str]| {
            let clap: Vec<String> = clap.iter().map(|s| s.to_string()).collect();
            assert_eq!(claimed(args), Some(clap), "{args:?}");
        };
        expect(&["ls"], &["okena", "ls"]);
        expect(
            &["claude", "ls", "--json"],
            &["okena", "claude", "ls", "--json"],
        );
        expect(&["help"], &["okena", "help"]);
        expect(&["-h"], &["okena", "-h"]);
        // Profile flags select the profile (main reads them) and are not clap's.
        expect(&["--profile", "work", "ls"], &["okena", "ls"]);
        expect(&["--profile=work", "frobnicate"], &["okena", "frobnicate"]);
        // A flag the GUI does not know goes to clap, which knows `--window`
        // and rejects a typo.
        expect(
            &["--window", "main", "term", "focus", "x"],
            &["okena", "--window", "main", "term", "focus", "x"],
        );
        expect(&["--jsno", "ls"], &["okena", "--jsno", "ls"]);
        // Unknown subcommands are claimed, so clap can reject them.
        expect(&["claud", "ls"], &["okena", "claud", "ls"]);
    }

    #[test]
    fn clap_rejects_what_the_gate_claims_but_does_not_know() {
        use clap::Parser;
        for args in [&["frobnicate"][..], &["--jsno", "ls"]] {
            let clap_args = claimed(args).unwrap();
            let error = match crate::parser::Cli::try_parse_from(&clap_args) {
                Ok(_) => panic!("{args:?} must not parse"),
                Err(error) => error,
            };
            assert_eq!(error.exit_code(), 2, "{args:?}");
        }
        let window = claimed(&["--window", "main", "term", "focus", "x"]).unwrap();
        assert!(crate::parser::Cli::try_parse_from(&window).is_ok());
    }
}
