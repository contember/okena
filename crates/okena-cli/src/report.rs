//! Report-back: `okena spawn` starts a command or an agent in a new terminal
//! that reports back to yours, `okena report` reports from inside a child,
//! `okena wait` blocks until children report, `okena children` lists them, and
//! `okena link` / `unlink` attach an existing terminal. The daemon side is
//! `/v1/report/...` (`okena-remote-server/src/report/`).

use std::path::Path;
use std::time::{Duration, Instant};

use okena_core::api::StateResponse;
use okena_core::report::{
    ApiReportChildrenResponse, ApiReportEvent, ApiReportWaitResponse, ReportEventKind,
};
use okena_core::shell::{ShellType, shell_quote};

use crate::claude::{pane_shell, reads_posix_quoting};
use crate::commands::fetch_state;
use crate::{api_action, api_get, api_post_json, api_post_json_blocking, ensure_token, resolve};

/// What `spawn` appends to the command; the daemon skips the line showing it
/// when it quotes the screen (`okena_remote_server::report::EXIT_REPORT_MARK`).
const EXIT_REPORT_ARGS: &str = "report --exit-code";
/// One wait request blocks at most this long on the daemon; the CLI asks again.
const WAIT_POLL: Duration = Duration::from_secs(25);

pub struct SpawnOptions<'a> {
    pub name: Option<&'a str>,
    pub project: Option<&'a str>,
    pub parent: Option<&'a str>,
    pub keep: bool,
    pub claude: bool,
    pub prompt: Option<String>,
    pub prompt_file: Option<&'a Path>,
    pub trust: bool,
    pub wait: bool,
    pub timeout_secs: u64,
    pub json: bool,
    pub command: &'a [String],
}

/// `okena spawn`: a new terminal in the parent's project (or `--project`),
/// linked to the parent, running `command` (or `claude` with `--claude`).
pub fn cli_spawn(options: SpawnOptions<'_>) -> i32 {
    let result = (|| {
        let token = ensure_token()?;
        let state = fetch_state(&token)?;
        let parent = parent_terminal(&state, options.parent)?;
        let project_id = match options.project {
            Some(project) => resolve::resolve_project(&state, project)?.id.clone(),
            None => resolve::project_of_terminal(&state, &parent)
                .ok_or_else(|| format!("no project holds terminal {parent}"))?
                .id
                .clone(),
        };
        let prompt = match (options.prompt.clone(), options.prompt_file) {
            (Some(_), Some(_)) => return Err("pass --prompt or --prompt-file, not both".into()),
            (Some(prompt), None) => Some(prompt),
            (None, Some(path)) => Some(
                std::fs::read_to_string(path)
                    .map_err(|e| format!("Cannot read {}: {e}", path.display()))?,
            ),
            (None, None) => None,
        };
        if !options.claude {
            if options.command.is_empty() {
                return Err("nothing to run: pass `-- <command…>` or --claude".into());
            }
            if prompt.is_some() || options.trust {
                return Err("--prompt, --prompt-file and --trust go with --claude".into());
            }
            let shell = pane_shell(&state, &project_id, "", &default_shell(&token)?);
            if !reads_posix_quoting(&shell) {
                return Err(
                    "spawn reports a command's exit through a POSIX shell (sh, bash, zsh); \
                     this project's shell is not one"
                        .into(),
                );
            }
        }

        let created = api_action(
            &token,
            &serde_json::json!({ "action": "create_terminal", "project_id": project_id })
                .to_string(),
        )?;
        let child = serde_json::from_str::<serde_json::Value>(&created)
            .ok()
            .and_then(|value| value["terminal_ids"].get(0)?.as_str().map(str::to_string))
            .ok_or_else(|| format!("create_terminal returned no terminal: {created}"))?;

        let name = options
            .name
            .map(str::to_string)
            .or_else(|| default_name(options.claude, options.command));
        if let Some(name) = &name {
            api_action(
                &token,
                &serde_json::json!({
                    "action": "rename_terminal",
                    "project_id": project_id,
                    "terminal_id": child,
                    "name": name,
                })
                .to_string(),
            )?;
        }
        link(&token, &child, &parent, !options.keep)?;
        Ok::<_, String>((token, parent, child, prompt))
    })();
    let (token, parent, child, prompt) = match result {
        Ok(spawned) => spawned,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };

    if options.claude {
        // `claude start` prints the terminal itself.
        return crate::claude::start(
            &child,
            prompt,
            options.trust,
            None,
            60,
            options.json,
            options.command,
        );
    }

    let command = spawned_command(&options.command.join(" "), &okena_bin());
    if let Err(e) = api_action(
        &token,
        &serde_json::json!({ "action": "run_command", "terminal_id": child, "command": command })
            .to_string(),
    ) {
        eprintln!("{e}");
        return 1;
    }
    if !options.wait {
        if options.json {
            println!("{}", serde_json::json!({ "terminal_id": child }));
        } else {
            println!("{child}");
        }
        return 0;
    }
    if !options.json {
        println!("{child}");
    }
    wait_and_print(&token, &parent, &[child], false, options.timeout_secs, options.json, true)
}

/// `okena report`: report from inside a child terminal to its parent.
pub fn cli_report(
    text: &[String],
    ask: bool,
    exit_code: Option<i32>,
    codex_notify: bool,
) -> i32 {
    let Ok(terminal_id) = std::env::var("OKENA_TERMINAL_ID") else {
        if codex_notify {
            return 0;
        }
        eprintln!("okena report runs inside an Okena terminal (OKENA_TERMINAL_ID is not set)");
        return 1;
    };
    let (event, text) = if codex_notify {
        // Codex runs its `notify` program with the event JSON as the last
        // argument. Never fail: codex would show the error.
        match text.last().and_then(|json| codex_turn_message(json)) {
            Some(message) => (ReportEventKind::Done, message),
            None => return 0,
        }
    } else if exit_code.is_some() {
        (ReportEventKind::Exit, String::new())
    } else {
        let text = text.join(" ");
        if text.trim().is_empty() {
            eprintln!("nothing to report: pass the text");
            return 1;
        }
        (if ask { ReportEventKind::Ask } else { ReportEventKind::Report }, text)
    };
    let body = serde_json::json!({
        "terminal_id": terminal_id,
        "event": event,
        "exit_code": exit_code.filter(|_| event == ReportEventKind::Exit),
        "text": text,
    });
    let result = ensure_token().and_then(|token| api_post_json("/v1/report", &token, &body));
    match result {
        Ok(_) => 0,
        Err(_) if codex_notify => 0,
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

/// `okena wait`: block until a child of yours reports (each one, with `--all`).
pub fn cli_wait(
    children: &[String],
    all: bool,
    parent: Option<&str>,
    timeout_secs: u64,
    json: bool,
) -> i32 {
    let resolved = (|| {
        let token = ensure_token()?;
        let state = fetch_state(&token)?;
        let parent = parent_terminal(&state, parent)?;
        let children = children
            .iter()
            .map(|child| resolve::resolve_terminal(&state, child).map(|(_, id)| id))
            .collect::<Result<Vec<_>, _>>()?;
        Ok::<_, String>((token, parent, children))
    })();
    match resolved {
        Ok((token, parent, children)) => {
            wait_and_print(&token, &parent, &children, all, timeout_secs, json, false)
        }
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

/// Wait, print the reports, and exit 0; 1 on a timeout. `exit_status`: exit
/// with an `exit` report's code (`spawn --wait`).
fn wait_and_print(
    token: &str,
    parent: &str,
    children: &[String],
    all: bool,
    timeout_secs: u64,
    json: bool,
    exit_status: bool,
) -> i32 {
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let poll = left.min(WAIT_POLL);
        let body = serde_json::json!({
            "parent_terminal_id": parent,
            "children": children,
            "all": all,
            "timeout_ms": poll.as_millis() as u64,
        });
        let response = api_post_json_blocking(
            "/v1/report/wait",
            token,
            &body,
            poll + Duration::from_secs(10),
        )
        .and_then(|raw| {
            serde_json::from_str::<ApiReportWaitResponse>(&raw)
                .map_err(|e| format!("Invalid wait response: {e}"))
        });
        let response = match response {
            Ok(response) => response,
            Err(e) => {
                eprintln!("{e}");
                return 1;
            }
        };
        if !response.events.is_empty() {
            print_events(&response.events, json);
            if exit_status
                && let Some(code) = response
                    .events
                    .iter()
                    .find(|event| event.event == ReportEventKind::Exit)
                    .and_then(|event| event.exit_code)
            {
                return code.clamp(0, 255);
            }
            return 0;
        }
        if Instant::now() >= deadline {
            eprintln!("timeout; still waiting for: {}", response.waiting.join(" "));
            return 1;
        }
    }
}

fn print_events(events: &[ApiReportEvent], json: bool) {
    if json {
        println!("{}", serde_json::json!({ "events": events }));
    } else {
        for event in events {
            println!("{}", event.message);
        }
    }
}

/// `okena children`: your children (all links with `--all`).
pub fn cli_children(all: bool, parent: Option<&str>, json: bool) -> i32 {
    let result = (|| {
        let token = ensure_token()?;
        let path = if all {
            "/v1/report/children".to_string()
        } else {
            let state = fetch_state(&token)?;
            let parent = parent_terminal(&state, parent)?;
            format!("/v1/report/children?parent={parent}")
        };
        let raw = api_get(&path, &token)?;
        let response: ApiReportChildrenResponse =
            serde_json::from_str(&raw).map_err(|e| format!("Invalid children: {e}"))?;
        if json {
            println!("{raw}");
            return Ok(());
        }
        for child in response.children {
            let (state, delivery) = match (&child.last_event, child.running) {
                (_, true) => ("running".to_string(), "-".to_string()),
                (Some(event), false) => (
                    match event.exit_code {
                        Some(code) => format!("{} {code}", event.event.as_str()),
                        None => event.event.as_str().to_string(),
                    },
                    event.delivery.map_or("-".to_string(), |delivery| {
                        serde_json::to_value(delivery)
                            .ok()
                            .and_then(|value| value.as_str().map(str::to_string))
                            .unwrap_or_default()
                    }),
                ),
                (None, false) => ("idle".to_string(), "-".to_string()),
            };
            println!(
                "{}\t{}\t{state}\t{delivery}\t{}",
                child.terminal_id,
                child.name.as_deref().unwrap_or("-"),
                child.parent_terminal_id
            );
        }
        Ok::<_, String>(())
    })();
    exit_code(result)
}

/// `okena link <child>`: an existing terminal reports to yours (`--parent`).
pub fn cli_link(child: &str, parent: Option<&str>, owned: bool) -> i32 {
    exit_code((|| {
        let token = ensure_token()?;
        let state = fetch_state(&token)?;
        let parent = parent_terminal(&state, parent)?;
        let (_, child) = resolve::resolve_terminal(&state, child)?;
        link(&token, &child, &parent, owned)
    })())
}

pub fn cli_unlink(child: &str) -> i32 {
    exit_code((|| {
        let token = ensure_token()?;
        let state = fetch_state(&token)?;
        let (_, child) = resolve::resolve_terminal(&state, child)?;
        api_post_json(
            "/v1/report/unlink",
            &token,
            &serde_json::json!({ "child_terminal_id": child }),
        )
        .map(drop)
    })())
}

fn link(token: &str, child: &str, parent: &str, owned: bool) -> Result<(), String> {
    api_post_json(
        "/v1/report/link",
        token,
        &serde_json::json!({
            "child_terminal_id": child,
            "parent_terminal_id": parent,
            "owned": owned,
        }),
    )
    .map(drop)
}

/// `--parent`, else the terminal this runs in.
fn parent_terminal(state: &StateResponse, parent: Option<&str>) -> Result<String, String> {
    match parent {
        Some(parent) => resolve::resolve_terminal(state, parent).map(|(_, id)| id),
        None => std::env::var("OKENA_TERMINAL_ID").map_err(|_| {
            "not inside an Okena terminal (OKENA_TERMINAL_ID is not set); pass --parent".to_string()
        }),
    }
}

fn default_shell(token: &str) -> Result<ShellType, String> {
    #[derive(serde::Deserialize)]
    struct Settings {
        #[serde(default)]
        default_shell: ShellType,
    }
    let body = api_action(
        token,
        &serde_json::json!({ "action": "get_settings" }).to_string(),
    )?;
    serde_json::from_str::<Settings>(&body)
        .map(|settings| settings.default_shell)
        .map_err(|e| format!("Invalid settings: {e}"))
}

/// The child's name when `--name` is not given: the program it runs.
fn default_name(claude: bool, command: &[String]) -> Option<String> {
    if claude {
        return Some("claude".to_string());
    }
    let program = command.first()?.split_whitespace().next()?;
    Path::new(program)
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_string)
}

/// The okena binary the child calls to report its exit.
fn okena_bin() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.to_str().map(str::to_string))
        .unwrap_or_else(|| "okena".to_string())
}

/// The line `spawn` types: the command, then a report of its exit code. A
/// `codex` command also reports each finished turn through codex's `notify`.
fn spawned_command(command: &str, okena: &str) -> String {
    let okena = shell_quote(okena);
    let command = match command.strip_prefix("codex") {
        Some(rest) if (rest.is_empty() || rest.starts_with(' ')) && !rest.contains("notify") => {
            let notify = format!(
                "notify=[{},\"report\",\"--codex-notify\"]",
                serde_json::Value::String(okena_bin_unquoted(&okena))
            );
            format!("codex -c {}{rest}", shell_quote(&notify))
        }
        _ => command.to_string(),
    };
    format!("{command}; {okena} {EXIT_REPORT_ARGS} $?")
}

/// Undo [`shell_quote`] for the notify array, which holds the bare path.
fn okena_bin_unquoted(quoted: &str) -> String {
    quoted
        .strip_prefix('\'')
        .and_then(|rest| rest.strip_suffix('\''))
        .map_or_else(|| quoted.to_string(), |inner| inner.replace("'\\''", "'"))
}

/// The last message of a finished codex turn, from the JSON codex hands its
/// `notify` program. `None` for any other notification.
fn codex_turn_message(json: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    if value["type"] != "agent-turn-complete" {
        return None;
    }
    Some(
        value["last-assistant-message"]
            .as_str()
            .unwrap_or("(turn complete)")
            .to_string(),
    )
}

fn exit_code(result: Result<(), String>) -> i32 {
    match result {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_types_the_command_then_reports_its_exit() {
        assert_eq!(
            spawned_command("cargo test -p foo", "/opt/okena/okena"),
            "cargo test -p foo; /opt/okena/okena report --exit-code $?"
        );
        assert_eq!(
            spawned_command("make", "/Applications/My Okena/okena"),
            "make; '/Applications/My Okena/okena' report --exit-code $?"
        );
    }

    #[test]
    fn a_codex_child_also_reports_its_turns() {
        assert_eq!(
            spawned_command("codex \"review the diff\"", "/bin/okena"),
            "codex -c 'notify=[\"/bin/okena\",\"report\",\"--codex-notify\"]' \"review the diff\"; \
             /bin/okena report --exit-code $?"
        );
        // A path with a quote survives both quotings.
        let command = spawned_command("codex", "/tmp/it's/okena");
        assert!(command.starts_with("codex -c 'notify=[\"/tmp/it'\\''s/okena\""), "{command}");
        // Its own notify, or another program that starts with codex, is left alone.
        assert_eq!(
            spawned_command("codex -c notify=[]", "/bin/okena"),
            "codex -c notify=[]; /bin/okena report --exit-code $?"
        );
        assert_eq!(
            spawned_command("codexx", "/bin/okena"),
            "codexx; /bin/okena report --exit-code $?"
        );
    }

    #[test]
    fn codex_notify_reports_finished_turns_only() {
        assert_eq!(
            codex_turn_message(
                r#"{"type":"agent-turn-complete","turn-id":"1","last-assistant-message":"Done: 3 files"}"#
            )
            .as_deref(),
            Some("Done: 3 files")
        );
        assert_eq!(codex_turn_message(r#"{"type":"other"}"#), None);
        assert_eq!(codex_turn_message("not json"), None);
    }

    #[test]
    fn a_child_is_named_after_its_program() {
        assert_eq!(
            default_name(false, &["/usr/bin/cargo test".to_string()]).as_deref(),
            Some("cargo")
        );
        assert_eq!(
            default_name(false, &["codex".to_string(), "x".to_string()]).as_deref(),
            Some("codex")
        );
        assert_eq!(default_name(true, &[]).as_deref(), Some("claude"));
        assert_eq!(default_name(false, &[]), None);
    }
}
