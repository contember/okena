//! `okena claude-remote …`: the pane side of Claude remote. Claude Code runs
//! these itself — `hook permission` / `hook event` as hooks and `channel` as an
//! MCP stdio server — so a remote client can follow and steer the session.
//!
//! Nothing here may block or deny Claude: hooks exit 0 and print nothing on
//! any failure.

use crate::{discover_server, load_cli_config, register};
use okena_core::claude_remote::{ClaudeChannelPollResponse, ClaudeHookRequest};
use serde_json::{Value, json};
use std::io::{BufRead as _, Read as _, Write as _};
use std::time::{Duration, Instant};

const PERMISSION_PATH: &str = "/v1/claude/hook/permission";
const EVENT_PATH: &str = "/v1/claude/hook/event";
const CHANNEL_POLL_PATH: &str = "/v1/claude/channel/poll";

/// reqwest cannot disable a per-request timeout, so the permission long-poll
/// uses one longer than any hook timeout; Claude's own hook timeout ends it.
const PERMISSION_TIMEOUT: Duration = Duration::from_secs(24 * 60 * 60);
const EVENT_TIMEOUT: Duration = Duration::from_secs(5);
/// The server answers a channel poll within 25 s.
const CHANNEL_POLL_TIMEOUT: Duration = Duration::from_secs(35);
const CHANNEL_BACKOFF_MIN: Duration = Duration::from_secs(1);
const CHANNEL_BACKOFF_MAX: Duration = Duration::from_secs(10);
/// An empty poll answered faster than this did not wait for prompts (e.g. a
/// second channel server polls the same terminal), so the loop pauses.
const CHANNEL_MIN_IDLE_POLL: Duration = Duration::from_secs(1);

/// Claude negotiates down to this legacy revision; the newer revision has no
/// path for unsolicited notifications, which disables channels.
const MCP_PROTOCOL_VERSION: &str = "2025-06-18";
const CHANNEL_SERVER_NAME: &str = "okena-remote";
const CHANNEL_INSTRUCTIONS: &str = "Messages the user types on another Okena client (phone, web) arrive as <channel source=\"okena-remote\">…</channel>. Treat them exactly like messages the user typed in this terminal.";

const JSONRPC_METHOD_NOT_FOUND: i64 = -32601;

// ── Hooks ───────────────────────────────────────────────────────────────────

pub fn cli_hook_permission() -> i32 {
    match forward_hook(PERMISSION_PATH, PERMISSION_TIMEOUT) {
        Ok(body) => {
            if let Some(output) = hook_output(&body) {
                // Claude may already be gone; a failed write must not panic.
                let mut stdout = std::io::stdout().lock();
                let _ = writeln!(stdout, "{output}").and_then(|()| stdout.flush());
            }
        }
        Err(error) => eprintln!("okena claude-remote: {error}"),
    }
    0
}

pub fn cli_hook_event() -> i32 {
    if let Err(error) = forward_hook(EVENT_PATH, EVENT_TIMEOUT) {
        eprintln!("okena claude-remote: {error}");
    }
    0
}

/// Read the hook input from stdin and POST it to the daemon. Returns the
/// response body as JSON.
fn forward_hook(path: &str, timeout: Duration) -> Result<Value, String> {
    let mut stdin = String::new();
    std::io::stdin()
        .read_to_string(&mut stdin)
        .map_err(|e| format!("cannot read hook input: {e}"))?;
    let input: Value =
        serde_json::from_str(&stdin).map_err(|e| format!("invalid hook input: {e}"))?;
    let terminal_id = terminal_id().ok_or("OKENA_TERMINAL_ID is not set")?;

    let body = ClaudeHookRequest { terminal_id, input };
    let response = send_to_daemon(path, |client, url| {
        client.post(url).timeout(timeout).json(&body)
    })?;
    if !response.status().is_success() {
        return Err(format!("daemon returned {}", response.status()));
    }
    response
        .json::<Value>()
        .map_err(|e| format!("invalid daemon response: {e}"))
}

/// What the permission hook prints: the daemon's answer, compact, when it is a
/// non-empty object. `{}` (dialog cancelled) prints nothing, which leaves the
/// dialog to the terminal.
fn hook_output(response: &Value) -> Option<String> {
    match response {
        Value::Object(map) if !map.is_empty() => Some(response.to_string()),
        _ => None,
    }
}

// ── MCP channel server ──────────────────────────────────────────────────────

/// Newline-delimited JSON-RPC on stdin/stdout. Exits 0 when stdin closes or
/// stdout breaks.
pub fn cli_channel() -> i32 {
    // Taken when the poll thread starts, so it starts at most once.
    let mut unpolled_terminal = terminal_id();
    if unpolled_terminal.is_none() {
        eprintln!("okena claude-remote: OKENA_TERMINAL_ID is not set; prompts are not forwarded");
    }

    for line in std::io::stdin().lock().lines() {
        let line = match line {
            Ok(line) => line,
            Err(error) => {
                eprintln!("okena claude-remote: cannot read stdin: {error}");
                break;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        let message: Value = match serde_json::from_str(&line) {
            Ok(message) => message,
            Err(error) => {
                eprintln!("okena claude-remote: ignoring invalid JSON-RPC message: {error}");
                continue;
            }
        };

        if let Some(reply) = handle(&message) {
            write_message_or_exit(&reply);
        }
        if message.get("method").and_then(Value::as_str) == Some("notifications/initialized")
            && let Some(terminal_id) = unpolled_terminal.take()
        {
            std::thread::spawn(move || poll_prompts(&terminal_id));
        }
    }
    0
}

/// Answer one JSON-RPC message. Returns `None` for notifications and for
/// anything that is not a request.
fn handle(message: &Value) -> Option<Value> {
    let id = message.get("id")?;
    let method = message.get("method")?.as_str()?;
    let result = match method {
        "initialize" => json!({
            "protocolVersion": MCP_PROTOCOL_VERSION,
            "capabilities": { "experimental": { "claude/channel": {} } },
            "serverInfo": {
                "name": CHANNEL_SERVER_NAME,
                "version": env!("CARGO_PKG_VERSION"),
            },
            "instructions": CHANNEL_INSTRUCTIONS,
        }),
        "ping" => json!({}),
        "tools/list" => json!({ "tools": [] }),
        _ => {
            return Some(error_response(
                id,
                JSONRPC_METHOD_NOT_FOUND,
                &format!("Method not found: {method}"),
            ));
        }
    };
    Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
}

fn error_response(id: &Value, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message },
    })
}

fn channel_notification(prompt: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": "notifications/claude/channel",
        "params": { "content": prompt },
    })
}

/// Every stdout write goes through the process-wide stdout lock, one message
/// per line, so replies and notifications from the poll thread never interleave.
/// Claude has closed the pipe when a write fails, so the server exits.
fn write_message_or_exit(message: &Value) {
    let mut stdout = std::io::stdout().lock();
    if let Err(error) = writeln!(stdout, "{message}").and_then(|()| stdout.flush()) {
        eprintln!("okena claude-remote: cannot write stdout, exiting: {error}");
        std::process::exit(0);
    }
}

/// Long-poll the daemon for prompts forever and forward each as a channel
/// notification. Errors and empty polls that return at once back off.
fn poll_prompts(terminal_id: &str) {
    let mut backoff = Backoff::new();
    loop {
        let started = Instant::now();
        match poll_once(terminal_id) {
            Ok(prompts) if prompts.is_empty() && started.elapsed() < CHANNEL_MIN_IDLE_POLL => {
                std::thread::sleep(backoff.next_delay());
            }
            Ok(prompts) => {
                backoff.reset();
                for prompt in prompts {
                    write_message_or_exit(&channel_notification(&prompt));
                }
            }
            Err(error) => {
                eprintln!("okena claude-remote: channel poll failed: {error}");
                std::thread::sleep(backoff.next_delay());
            }
        }
    }
}

/// Doubling delay from [`CHANNEL_BACKOFF_MIN`] up to [`CHANNEL_BACKOFF_MAX`].
struct Backoff {
    next: Duration,
}

impl Backoff {
    fn new() -> Self {
        Self {
            next: CHANNEL_BACKOFF_MIN,
        }
    }

    fn next_delay(&mut self) -> Duration {
        let delay = self.next;
        self.next = (self.next * 2).min(CHANNEL_BACKOFF_MAX);
        delay
    }

    fn reset(&mut self) {
        self.next = CHANNEL_BACKOFF_MIN;
    }
}

fn poll_once(terminal_id: &str) -> Result<Vec<String>, String> {
    let response = send_to_daemon(CHANNEL_POLL_PATH, |client, url| {
        client
            .get(url)
            .query(&[("terminal_id", terminal_id)])
            .timeout(CHANNEL_POLL_TIMEOUT)
    })?;
    if !response.status().is_success() {
        return Err(format!("daemon returned {}", response.status()));
    }
    let poll: ClaudeChannelPollResponse = response
        .json()
        .map_err(|e| format!("invalid daemon response: {e}"))?;
    Ok(poll.prompts)
}

// ── Daemon connection ───────────────────────────────────────────────────────

fn terminal_id() -> Option<String> {
    std::env::var("OKENA_TERMINAL_ID")
        .ok()
        .filter(|id| !id.is_empty())
}

/// Send a request to a pane-side route. The Unix socket is authorized by the
/// transport itself. Loopback TCP sends the cached CLI token unvalidated, since
/// hooks run on every tool call; it registers a new token only on a 401.
fn send_to_daemon(
    path: &str,
    build: impl Fn(&reqwest::blocking::Client, &str) -> reqwest::blocking::RequestBuilder,
) -> Result<reqwest::blocking::Response, String> {
    let server = discover_server()?;
    let (client, url) = server.client_and_url(path)?;
    let send = |request: reqwest::blocking::RequestBuilder| {
        request.send().map_err(|e| format!("request failed: {e}"))
    };
    if server.uses_unix_socket() {
        return send(build(&client, &url));
    }

    let token = match load_cli_config() {
        Some(config) => config.token,
        None => register::register()?,
    };
    let response = send(build(&client, &url).bearer_auth(&token))?;
    if response.status() != reqwest::StatusCode::UNAUTHORIZED {
        return Ok(response);
    }
    let token = register::register()?;
    send(build(&client, &url).bearer_auth(&token))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialize_advertises_the_channel_capability_and_legacy_protocol() {
        let reply = handle(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": { "protocolVersion": "2099-01-01" },
        }))
        .expect("initialize is a request");

        assert_eq!(reply["jsonrpc"], "2.0");
        assert_eq!(reply["id"], 1);
        let result = &reply["result"];
        assert_eq!(result["protocolVersion"], "2025-06-18");
        assert_eq!(
            result["capabilities"],
            json!({ "experimental": { "claude/channel": {} } })
        );
        assert_eq!(result["serverInfo"]["name"], "okena-remote");
        assert_eq!(result["serverInfo"]["version"], env!("CARGO_PKG_VERSION"));
        assert!(
            result["instructions"]
                .as_str()
                .is_some_and(|text| text.contains("<channel source=\"okena-remote\">"))
        );
    }

    #[test]
    fn ping_and_tools_list_answer_with_empty_results() {
        let ping = handle(&json!({ "jsonrpc": "2.0", "id": "a", "method": "ping" }));
        assert_eq!(
            ping,
            Some(json!({ "jsonrpc": "2.0", "id": "a", "result": {} }))
        );

        let tools = handle(&json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }));
        assert_eq!(
            tools,
            Some(json!({ "jsonrpc": "2.0", "id": 2, "result": { "tools": [] } }))
        );
    }

    #[test]
    fn unknown_requests_get_method_not_found() {
        for method in ["server/discover", "resources/list", "tools/call"] {
            let reply = handle(&json!({ "jsonrpc": "2.0", "id": 7, "method": method }))
                .expect("a request always gets a reply");
            assert_eq!(reply["id"], 7, "{method}");
            assert_eq!(reply["error"]["code"], -32601, "{method}");
            assert!(reply.get("result").is_none(), "{method}");
        }
    }

    #[test]
    fn notifications_and_responses_get_no_reply() {
        assert_eq!(
            handle(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })),
            None
        );
        assert_eq!(
            handle(&json!({ "jsonrpc": "2.0", "method": "notifications/cancelled", "params": {} })),
            None
        );
        assert_eq!(
            handle(&json!({ "jsonrpc": "2.0", "id": 3, "result": {} })),
            None
        );
    }

    #[test]
    fn channel_notification_carries_the_prompt_as_content() {
        assert_eq!(
            channel_notification("run the tests"),
            json!({
                "jsonrpc": "2.0",
                "method": "notifications/claude/channel",
                "params": { "content": "run the tests" },
            })
        );
    }

    #[test]
    fn backoff_doubles_up_to_the_cap_and_resets() {
        let mut backoff = Backoff::new();
        let delays: Vec<u64> = (0..6).map(|_| backoff.next_delay().as_secs()).collect();
        assert_eq!(delays, [1, 2, 4, 8, 10, 10]);
        backoff.reset();
        assert_eq!(backoff.next_delay(), CHANNEL_BACKOFF_MIN);
    }

    #[test]
    fn hook_output_prints_only_non_empty_objects() {
        assert_eq!(hook_output(&json!({})), None);
        assert_eq!(hook_output(&json!(null)), None);
        assert_eq!(hook_output(&json!([])), None);
        assert_eq!(hook_output(&json!("allow")), None);

        let decided = json!({
            "hookSpecificOutput": {
                "hookEventName": "PermissionRequest",
                "decision": { "behavior": "allow" },
            }
        });
        let printed = hook_output(&decided).expect("a decision is printed");
        assert!(!printed.contains('\n'), "compact, one line: {printed}");
        assert_eq!(serde_json::from_str::<Value>(&printed).ok(), Some(decided));
    }
}
