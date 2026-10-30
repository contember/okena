//! `okena claude …`: follow and steer the `claude` sessions running in panes
//! through the same client endpoints the web and mobile chat use
//! (`/v1/claude/terminals/{id}/...`, see `docs/reference/claude-remote.md`).

use crate::commands::{fetch_state, with_state_post};
use crate::parser::ClaudeDecisionArg;
use crate::{api_action, api_get, api_post_json, ensure_token, resolve};
use okena_core::api::StateResponse;
use okena_core::claude_remote::{
    ApiClaudeDecision, ApiClaudeEntry, ApiClaudePromptDelivery, ApiClaudePromptOrigin,
    ApiClaudePromptRequest, ApiClaudePromptResponse, ApiClaudeSession, ApiClaudeStartupDialog,
    ApiClaudeSubagentList, ApiClaudeTranscriptPage,
};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant};

/// How often `claude start` re-reads the session while it waits.
const START_POLL_INTERVAL: Duration = Duration::from_millis(250);
/// After a key press, the dialog needs a moment to redraw before the screen
/// says anything new; pressing again earlier could answer the next dialog.
const KEY_SETTLE: Duration = Duration::from_millis(700);

/// `okena claude ls [--json]`
pub fn cli_ls(json_mode: bool) -> i32 {
    exit_code((|| {
        let (token, state) = connect()?;
        let mut sessions = Vec::new();
        for terminal_id in connected_terminal_ids(&state) {
            // A pane can close between the state read and this request.
            match fetch_session(&token, &terminal_id) {
                Ok(session) => sessions.push(LabeledSession::new(&state, session)),
                Err(e) => eprintln!("{terminal_id}: {e}"),
            }
        }
        if json_mode {
            let entries = sessions
                .iter()
                .map(LabeledSession::to_json)
                .collect::<Result<Vec<_>, _>>()?;
            print_json(&serde_json::Value::Array(entries))
        } else {
            for session in &sessions {
                println!("{}", ls_row(session));
            }
            Ok(())
        }
    })())
}

/// `okena claude show <term> [--json]`
pub fn cli_show(terminal: &str, json_mode: bool) -> i32 {
    exit_code((|| {
        let (token, state) = connect()?;
        let (_project_id, terminal_id) = resolve::resolve_terminal(&state, terminal)?;
        let session = LabeledSession::new(&state, fetch_session(&token, &terminal_id)?);
        if json_mode {
            print_json(&session.to_json()?)
        } else {
            for line in show_lines(&session) {
                println!("{line}");
            }
            Ok(())
        }
    })())
}

/// `okena claude find <session> [--json]`
pub fn cli_find(session_id: &str, json_mode: bool) -> i32 {
    exit_code((|| {
        let (token, state) = connect()?;
        let body = api_get(
            &format!("/v1/claude/sessions/{}", encode(session_id)),
            &token,
        )?;
        let session: ApiClaudeSession =
            serde_json::from_str(&body).map_err(|e| format!("Invalid session: {e}"))?;
        if json_mode {
            print_json(&LabeledSession::new(&state, session).to_json()?)
        } else {
            println!("{}", session.terminal_id);
            Ok(())
        }
    })())
}

/// `okena claude subagents <term> [--json]`
pub fn cli_subagents(terminal: &str, json_mode: bool) -> i32 {
    exit_code((|| {
        let (token, state) = connect()?;
        let (_project_id, terminal_id) = resolve::resolve_terminal(&state, terminal)?;
        let body = api_get(&terminal_path(&terminal_id, "/subagents"), &token)?;
        let list: ApiClaudeSubagentList =
            serde_json::from_str(&body).map_err(|e| format!("Invalid subagent list: {e}"))?;
        if json_mode {
            print_json(&to_json(&list)?)
        } else {
            for subagent in &list.subagents {
                println!(
                    "{}\t{}\t{}\t{}",
                    subagent.agent_id,
                    subagent.agent_type.as_deref().unwrap_or("-"),
                    if subagent.running { "running" } else { "done" },
                    one_line(subagent.description.as_deref().unwrap_or("")),
                );
            }
            Ok(())
        }
    })())
}

/// `okena claude log <term> [--cursor <c>] [--agent <id>] [--json]`
pub fn cli_log(terminal: &str, cursor: Option<&str>, agent: Option<&str>, json_mode: bool) -> i32 {
    exit_code((|| {
        let cursor = cursor.map(LogCursor::parse).transpose()?;
        let (token, state) = connect()?;
        let (_project_id, terminal_id) = resolve::resolve_terminal(&state, terminal)?;
        let path = terminal_path(&terminal_id, &transcript_query(cursor.as_ref(), agent));
        let body = api_get(&path, &token)?;
        let page: ApiClaudeTranscriptPage =
            serde_json::from_str(&body).map_err(|e| format!("Invalid transcript page: {e}"))?;
        let next = LogCursor {
            session_id: page.session_id.clone(),
            offset: page.cursor,
        }
        .to_string();
        if json_mode {
            let mut value = to_json(&page)?;
            if let Some(object) = value.as_object_mut() {
                object.insert("next_cursor".into(), next.into());
            }
            print_json(&value)
        } else {
            for line in log_lines(&page, &next) {
                println!("{line}");
            }
            Ok(())
        }
    })())
}

/// `okena claude prompt <term> <text…>`
pub fn cli_prompt(terminal: &str, session: Option<&str>, text: &[String]) -> i32 {
    exit_code((|| {
        let (token, state) = connect()?;
        let (_project_id, terminal_id) = resolve::resolve_terminal(&state, terminal)?;
        let delivery = send_prompt(&token, &terminal_id, text.join(" "), session)?;
        println!("{}", delivery_label(delivery));
        Ok(())
    })())
}

/// `okena claude start <term> [--prompt-file f] [--trust] [--wait] [--timeout s] [--json]`:
/// run `claude` in the pane, accept the start-up dialogs, wait until it
/// connects and send the first prompt.
pub fn cli_start(
    terminal: &str,
    prompt_file: Option<&Path>,
    trust: bool,
    wait: Option<Duration>,
    timeout_secs: u64,
    json_mode: bool,
) -> i32 {
    exit_code((|| {
        let prompt = prompt_file
            .map(|path| {
                std::fs::read_to_string(path)
                    .map_err(|e| format!("Cannot read {}: {e}", path.display()))
            })
            .transpose()?;
        let (token, state) = connect()?;
        let (_project_id, terminal_id) = resolve::resolve_terminal(&state, terminal)?;
        let wait_for_channel = remote_settings(&token)?;
        let body = api_get(
            &terminal_path(&terminal_id, "?probe_foreground=true"),
            &token,
        )?;
        let session: ApiClaudeSession =
            serde_json::from_str(&body).map_err(|e| format!("Invalid session: {e}"))?;
        if launch(&session).map_err(|e| format!("{terminal_id}: {e}"))? == Launch::Type {
            let run = serde_json::json!({
                "action": "run_command",
                "terminal_id": terminal_id,
                "command": "claude",
            });
            api_action(&token, &run.to_string())?;
        }
        let session = wait_until_connected(
            &token,
            &terminal_id,
            wait_for_channel,
            trust,
            Duration::from_secs(timeout_secs),
        )?;

        let mut started = StartResult {
            terminal_id: terminal_id.clone(),
            session_id: session.session_id,
            delivery: None,
            state: None,
        };
        if let Some(prompt) = prompt {
            let before = fetch_page(&token, &terminal_id, None).ok();
            started.delivery = Some(send_prompt(
                &token,
                &terminal_id,
                prompt,
                started.session_id.as_deref(),
            )?);
            if let Some(wait) = wait {
                started.state = Some(wait_for_turn(&token, &terminal_id, before, wait)?);
            }
        }
        if started.session_id.is_none() {
            started.session_id = fetch_session(&token, &terminal_id)?.session_id;
        }
        if json_mode {
            print_json(&started.to_json())?;
        } else {
            for line in started.lines() {
                println!("{line}");
            }
        }
        match started.state {
            Some(TurnState::Disconnected) => Err(format!("claude in {terminal_id} disconnected")),
            Some(TurnState::TimedOut) => {
                Err(format!("the turn in {terminal_id} did not end in time"))
            }
            _ => Ok(()),
        }
    })())
}

/// `okena claude answer <term> <dialog> allow|deny|answer [...]`
pub fn cli_answer(
    terminal: &str,
    dialog: &str,
    decision: ClaudeDecisionArg,
    always: bool,
    message: Option<&str>,
    answers: Option<&str>,
) -> i32 {
    exit_code((|| {
        let decision = build_decision(decision, always, message, answers)?;
        let (token, state) = connect()?;
        let (_project_id, terminal_id) = resolve::resolve_terminal(&state, terminal)?;
        let path = terminal_path(&terminal_id, &format!("/dialogs/{}", encode(dialog)));
        api_post_json(&path, &token, &to_json(&decision)?)?;
        Ok(())
    })())
}

/// `okena claude interrupt <term>` — Escape, like the chat's Interrupt button.
pub fn cli_interrupt(terminal: &str) -> i32 {
    with_state_post(|state| {
        let (_project_id, terminal_id) = resolve::resolve_terminal(state, terminal)?;
        Ok(serde_json::json!({
            "action": "send_special_key",
            "terminal_id": terminal_id,
            "key": "Escape",
        }))
    })
}

// ── Plumbing ────────────────────────────────────────────────────────────────

fn exit_code(result: Result<(), String>) -> i32 {
    match result {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

fn connect() -> Result<(String, StateResponse), String> {
    let token = ensure_token()?;
    let state = fetch_state(&token)?;
    Ok((token, state))
}

fn fetch_session(token: &str, terminal_id: &str) -> Result<ApiClaudeSession, String> {
    let body = api_get(&terminal_path(terminal_id, ""), token)?;
    serde_json::from_str(&body).map_err(|e| format!("Invalid session: {e}"))
}

fn send_prompt(
    token: &str,
    terminal_id: &str,
    text: String,
    expect_session_id: Option<&str>,
) -> Result<ApiClaudePromptDelivery, String> {
    let request = ApiClaudePromptRequest {
        text,
        expect_session_id: expect_session_id.map(str::to_string),
    };
    let body = api_post_json(
        &terminal_path(terminal_id, "/prompt"),
        token,
        &to_json(&request)?,
    )?;
    let response: ApiClaudePromptResponse =
        serde_json::from_str(&body).map_err(|e| format!("Invalid prompt response: {e}"))?;
    Ok(response.delivery)
}

fn fetch_page(
    token: &str,
    terminal_id: &str,
    cursor: Option<&LogCursor>,
) -> Result<ApiClaudeTranscriptPage, String> {
    let body = api_get(
        &terminal_path(terminal_id, &transcript_query(cursor, None)),
        token,
    )?;
    serde_json::from_str(&body).map_err(|e| format!("Invalid transcript page: {e}"))
}

/// The settings `claude start` depends on.
#[derive(serde::Deserialize)]
struct RemoteSettings {
    claude_remote: bool,
    claude_remote_channel: bool,
}

/// Whether panes connect `claude` through the channel (else hooks only).
/// Fails when Claude remote is off: `claude` would never connect.
fn remote_settings(token: &str) -> Result<bool, String> {
    let body = api_action(
        token,
        &serde_json::json!({"action": "get_settings"}).to_string(),
    )?;
    let settings: RemoteSettings =
        serde_json::from_str(&body).map_err(|e| format!("Invalid settings: {e}"))?;
    if !settings.claude_remote {
        return Err(
            "Claude Remote is off; turn it on (okena settings set claude_remote true) and open a new terminal"
                .into(),
        );
    }
    Ok(settings.claude_remote_channel)
}

/// Whether `claude start` types `claude` or resumes one waiting on a
/// start-up dialog (typing into the dialog would answer it).
#[derive(Debug, PartialEq)]
enum Launch {
    Type,
    Resume,
}

fn launch(session: &ApiClaudeSession) -> Result<Launch, String> {
    if session.hooks_attached || session.channel_attached {
        return Err("a connected claude already runs here".into());
    }
    if session.startup_dialog.is_some() {
        return Ok(Launch::Resume);
    }
    match &session.foreground_command {
        Some(command) => Err(format!(
            "`{command}` runs in the foreground; claude start needs a shell prompt"
        )),
        None => Ok(Launch::Type),
    }
}

/// What one poll of the starting session asks for.
#[derive(Debug, PartialEq)]
enum StartStep {
    Connected,
    Wait,
    Answer(ApiClaudeStartupDialog),
    /// The trust dialog is open and `--trust` was not given.
    NeedsTrust,
}

fn start_step(session: &ApiClaudeSession, wait_for_channel: bool, trust: bool) -> StartStep {
    let connected = if wait_for_channel {
        session.channel_attached
    } else {
        session.hooks_attached
    };
    match session.startup_dialog {
        _ if connected => StartStep::Connected,
        None => StartStep::Wait,
        Some(ApiClaudeStartupDialog::Trust) if !trust => StartStep::NeedsTrust,
        Some(dialog) => StartStep::Answer(dialog),
    }
}

/// Poll the session until `claude` connects, answering the start-up dialogs on
/// the way: the channel warning always, the folder trust only with `trust`.
fn wait_until_connected(
    token: &str,
    terminal_id: &str,
    wait_for_channel: bool,
    trust: bool,
    timeout: Duration,
) -> Result<ApiClaudeSession, String> {
    let deadline = Instant::now() + timeout;
    let mut last_key: Option<Instant> = None;
    loop {
        if Instant::now() > deadline {
            return Err(format!(
                "claude did not connect within {}s; check the terminal (okena read {terminal_id})",
                timeout.as_secs()
            ));
        }
        std::thread::sleep(START_POLL_INTERVAL);
        let session = fetch_session(token, terminal_id)?;
        let dialog = match start_step(&session, wait_for_channel, trust) {
            StartStep::Connected => return Ok(session),
            StartStep::Wait => continue,
            StartStep::NeedsTrust => {
                return Err(format!(
                    "claude asks whether to trust this folder; pass --trust to accept it, or answer it in the terminal ({terminal_id})"
                ));
            }
            StartStep::Answer(dialog) => dialog,
        };
        if last_key.is_some_and(|at| at.elapsed() < KEY_SETTLE) {
            continue;
        }
        let screen = crate::commands::fetch_terminal_content(token, terminal_id)?;
        let Some(key) = startup_dialog_key(dialog, &screen) else {
            continue;
        };
        let body = serde_json::json!({
            "action": "send_special_key",
            "terminal_id": terminal_id,
            "key": key,
        });
        api_action(token, &body.to_string())?;
        last_key = Some(Instant::now());
    }
}

/// The key that moves `dialog` towards accepting: Enter on the confirming
/// option, else an arrow towards it. `None` when `screen` (read after the
/// session) no longer shows that dialog, so a key would hit the next one.
fn startup_dialog_key(dialog: ApiClaudeStartupDialog, screen: &str) -> Option<&'static str> {
    if ApiClaudeStartupDialog::detect(screen) != Some(dialog) {
        return None;
    }
    if dialog.confirm_selected(screen) {
        return Some("Enter");
    }
    let lines: Vec<&str> = screen.lines().collect();
    let selected = lines
        .iter()
        .position(|line| line.trim_start().starts_with('❯'));
    let confirm = lines
        .iter()
        .position(|line| line.contains(dialog.confirm_label()));
    match (selected, confirm) {
        (Some(selected), Some(confirm)) if confirm < selected => Some("ArrowUp"),
        _ => Some("ArrowDown"),
    }
}

/// Wait until the turn the first prompt started ends, a dialog needs an
/// answer, `claude` disconnects or `timeout` passes. `before` is the transcript
/// as it was before the prompt.
fn wait_for_turn(
    token: &str,
    terminal_id: &str,
    before: Option<ApiClaudeTranscriptPage>,
    timeout: Duration,
) -> Result<TurnState, String> {
    let deadline = Instant::now() + timeout;
    let mut cursor = before.map(|page| LogCursor {
        session_id: page.session_id,
        offset: page.cursor,
    });
    let mut progress = TurnProgress::default();
    loop {
        if Instant::now() > deadline {
            return Ok(TurnState::TimedOut);
        }
        std::thread::sleep(START_POLL_INTERVAL);
        if !progress.reply_seen
            && let Ok(page) = fetch_page(token, terminal_id, cursor.as_ref())
        {
            progress.read(&page.entries);
            cursor = Some(LogCursor {
                session_id: page.session_id,
                offset: page.cursor,
            });
        }
        let session = fetch_session(token, terminal_id)?;
        if let Some(state) = progress.observe(&session) {
            return Ok(state);
        }
    }
}

/// How far the prompt's turn has come. The hooks that mark the pane busy run
/// async, so the transcript can show the prompt while the pane still reads
/// idle: the turn has ended once it was seen busy, or a reply follows the
/// prompt, and the pane is idle again.
#[derive(Default)]
struct TurnProgress {
    prompt_seen: bool,
    reply_seen: bool,
    busy_seen: bool,
}

impl TurnProgress {
    fn read(&mut self, entries: &[ApiClaudeEntry]) {
        for entry in entries {
            match entry {
                ApiClaudeEntry::User { .. } => self.prompt_seen = true,
                ApiClaudeEntry::Assistant { .. } if self.prompt_seen => self.reply_seen = true,
                _ => {}
            }
        }
    }

    /// Where the turn stands after a session read; `None` while it runs.
    fn observe(&mut self, session: &ApiClaudeSession) -> Option<TurnState> {
        if !session.hooks_attached && !session.channel_attached {
            return Some(TurnState::Disconnected);
        }
        if !session.dialogs.is_empty() {
            return Some(TurnState::Dialog);
        }
        self.busy_seen |= session.busy == Some(true);
        let ran = self.reply_seen || self.busy_seen;
        (ran && session.busy == Some(false) && session.queued_prompts == 0)
            .then_some(TurnState::Idle)
    }
}

fn terminal_path(terminal_id: &str, rest: &str) -> String {
    format!("/v1/claude/terminals/{}{rest}", encode(terminal_id))
}

fn encode(value: &str) -> String {
    utf8_percent_encode(value, NON_ALPHANUMERIC).to_string()
}

fn transcript_query(cursor: Option<&LogCursor>, agent: Option<&str>) -> String {
    let mut params = Vec::new();
    if let Some(cursor) = cursor {
        params.push(format!("cursor={}", cursor.offset));
        if let Some(session_id) = &cursor.session_id {
            params.push(format!("session_id={}", encode(session_id)));
        }
    }
    if let Some(agent) = agent {
        params.push(format!("agent_id={}", encode(agent)));
    }
    if params.is_empty() {
        "/transcript".to_string()
    } else {
        format!("/transcript?{}", params.join("&"))
    }
}

fn to_json<T: serde::Serialize>(value: &T) -> Result<serde_json::Value, String> {
    serde_json::to_value(value).map_err(|e| format!("Failed to encode JSON: {e}"))
}

fn print_json(value: &serde_json::Value) -> Result<(), String> {
    let out =
        serde_json::to_string_pretty(value).map_err(|e| format!("Failed to encode JSON: {e}"))?;
    println!("{out}");
    Ok(())
}

// ── Pure parts ──────────────────────────────────────────────────────────────

/// Terminals the chat is offered for: `claude` connected, or a dialog open.
fn connected_terminal_ids(state: &StateResponse) -> Vec<String> {
    let mut ids: Vec<String> = state
        .claude_sessions
        .iter()
        .chain(state.claude_dialogs.keys())
        .cloned()
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

/// A session with the project and terminal names it is shown under.
struct LabeledSession {
    project_id: Option<String>,
    project_name: Option<String>,
    terminal_name: Option<String>,
    session: ApiClaudeSession,
}

impl LabeledSession {
    fn new(state: &StateResponse, session: ApiClaudeSession) -> Self {
        let project = state.projects.iter().find(|project| {
            resolve::project_terminals(project)
                .iter()
                .any(|t| t.terminal_id == session.terminal_id)
        });
        Self {
            project_id: project.map(|p| p.id.clone()),
            project_name: project.map(|p| p.name.clone()),
            // Only a real name; `project_terminals` falls back to the id.
            terminal_name: project
                .and_then(|p| p.terminal_names.get(&session.terminal_id).cloned()),
            session,
        }
    }

    fn to_json(&self) -> Result<serde_json::Value, String> {
        let mut value = to_json(&self.session)?;
        if let Some(object) = value.as_object_mut() {
            object.insert("project_id".into(), self.project_id.clone().into());
            object.insert("project_name".into(), self.project_name.clone().into());
            object.insert("terminal_name".into(), self.terminal_name.clone().into());
        }
        Ok(value)
    }
}

fn busy_label(busy: Option<bool>) -> &'static str {
    match busy {
        Some(true) => "busy",
        Some(false) => "idle",
        None => "-",
    }
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

fn delivery_label(delivery: ApiClaudePromptDelivery) -> &'static str {
    match delivery {
        ApiClaudePromptDelivery::Channel => "channel",
        ApiClaudePromptDelivery::Pty => "pty",
        ApiClaudePromptDelivery::Queued => "queued",
    }
}

/// Where `claude start --wait` stopped waiting.
#[derive(Clone, Copy, Debug, PartialEq)]
enum TurnState {
    Idle,
    /// A dialog waits for an answer (`claude show`, `claude answer`).
    Dialog,
    /// `claude` left the pane during the turn.
    Disconnected,
    /// `--wait-timeout` passed first.
    TimedOut,
}

impl TurnState {
    fn label(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Dialog => "dialog",
            Self::Disconnected => "disconnected",
            Self::TimedOut => "timeout",
        }
    }
}

/// What `claude start` reports.
struct StartResult {
    terminal_id: String,
    session_id: Option<String>,
    delivery: Option<ApiClaudePromptDelivery>,
    state: Option<TurnState>,
}

impl StartResult {
    fn lines(&self) -> Vec<String> {
        let mut lines = vec![
            format!("terminal\t{}", self.terminal_id),
            format!("session_id\t{}", self.session_id.as_deref().unwrap_or("-")),
            format!("delivery\t{}", self.delivery.map_or("-", delivery_label)),
        ];
        if let Some(state) = self.state {
            lines.push(format!("state\t{}", state.label()));
        }
        lines
    }

    fn to_json(&self) -> serde_json::Value {
        let mut value = serde_json::json!({
            "terminal": self.terminal_id,
            "session_id": self.session_id,
            "delivery": self.delivery.map(delivery_label),
        });
        if let (Some(state), Some(object)) = (self.state, value.as_object_mut()) {
            object.insert("state".into(), state.label().into());
        }
        value
    }
}

fn startup_dialog_label(dialog: Option<ApiClaudeStartupDialog>) -> &'static str {
    match dialog {
        Some(ApiClaudeStartupDialog::Trust) => "trust",
        Some(ApiClaudeStartupDialog::Channels) => "channels",
        None => "-",
    }
}

/// `terminal_id  project  name  session_id  busy|idle|-  dialogs  queued  subagents`
fn ls_row(labeled: &LabeledSession) -> String {
    let s = &labeled.session;
    format!(
        "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
        s.terminal_id,
        labeled.project_name.as_deref().unwrap_or("-"),
        labeled.terminal_name.as_deref().unwrap_or("-"),
        s.session_id.as_deref().unwrap_or("-"),
        busy_label(s.busy),
        s.dialogs.len(),
        s.queued_prompts,
        s.running_subagents.len(),
    )
}

/// `key<TAB>value` lines; one `subagent` line per running subagent and one
/// `dialog` line (id, tool, compact `tool_input` JSON) per open dialog.
fn show_lines(labeled: &LabeledSession) -> Vec<String> {
    let s = &labeled.session;
    let mut lines = vec![
        format!("terminal\t{}", s.terminal_id),
        format!(
            "project\t{}",
            labeled.project_name.as_deref().unwrap_or("-")
        ),
        format!("name\t{}", labeled.terminal_name.as_deref().unwrap_or("-")),
        format!("session\t{}", s.session_id.as_deref().unwrap_or("-")),
        format!("state\t{}", busy_label(s.busy)),
        format!("hooks\t{}", yes_no(s.hooks_attached)),
        format!("channel\t{}", yes_no(s.channel_attached)),
        format!("transcript\t{}", yes_no(s.has_transcript)),
        format!("queued\t{}", s.queued_prompts),
        format!("startup_dialog\t{}", startup_dialog_label(s.startup_dialog)),
    ];
    for subagent in &s.running_subagents {
        lines.push(format!(
            "subagent\t{}\t{}",
            subagent.agent_id, subagent.agent_type
        ));
    }
    for dialog in &s.dialogs {
        lines.push(format!(
            "dialog\t{}\t{}\t{}",
            dialog.id, dialog.tool_name, dialog.tool_input
        ));
    }
    lines
}

/// `reset` (when the page replaces what the caller had), one line per entry,
/// then `cursor<TAB><next>`.
fn log_lines(page: &ApiClaudeTranscriptPage, next_cursor: &str) -> Vec<String> {
    let mut lines = Vec::new();
    if page.reset {
        lines.push(format!(
            "reset\t{}",
            page.session_id.as_deref().unwrap_or("-")
        ));
    }
    lines.extend(page.entries.iter().map(entry_line));
    lines.push(format!("cursor\t{next_cursor}"));
    lines
}

fn entry_line(entry: &ApiClaudeEntry) -> String {
    match entry {
        ApiClaudeEntry::User { text, origin, .. } => {
            let origin = match origin {
                ApiClaudePromptOrigin::Terminal => "terminal",
                ApiClaudePromptOrigin::Channel => "channel",
            };
            format!("user\t{origin}\t{}", one_line(text))
        }
        ApiClaudeEntry::Assistant { text, .. } => format!("assistant\t{}", one_line(text)),
        ApiClaudeEntry::ToolUse {
            tool_use_id,
            name,
            summary,
            ..
        } => format!("tool_use\t{tool_use_id}\t{name}\t{}", one_line(summary)),
        ApiClaudeEntry::ToolResult {
            tool_use_id,
            text,
            is_error,
            ..
        } => format!(
            "tool_result\t{tool_use_id}\t{}\t{}",
            if *is_error { "error" } else { "ok" },
            one_line(text)
        ),
    }
}

/// Escape a value so a record stays on one tab-separated line.
fn one_line(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out
}

/// The transcript cursor handed to callers. The daemon honors an offset only
/// together with the session it came from, so both travel as one token:
/// `<session_id>:<offset>`, or a bare offset when the session is unknown.
#[derive(Debug, PartialEq)]
struct LogCursor {
    session_id: Option<String>,
    offset: u64,
}

impl LogCursor {
    fn parse(token: &str) -> Result<Self, String> {
        let (session_id, offset) = match token.rsplit_once(':') {
            Some((session_id, offset)) if !session_id.is_empty() => {
                (Some(session_id.to_string()), offset)
            }
            Some(_) => return Err(format!("Invalid cursor: {token}")),
            None => (None, token),
        };
        let offset = offset
            .parse()
            .map_err(|_| format!("Invalid cursor: {token}"))?;
        Ok(Self { session_id, offset })
    }
}

impl std::fmt::Display for LogCursor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.session_id {
            Some(session_id) => write!(f, "{session_id}:{}", self.offset),
            None => write!(f, "{}", self.offset),
        }
    }
}

fn build_decision(
    decision: ClaudeDecisionArg,
    always: bool,
    message: Option<&str>,
    answers: Option<&str>,
) -> Result<ApiClaudeDecision, String> {
    if always && decision != ClaudeDecisionArg::Allow {
        return Err("--always only applies to allow".into());
    }
    if message.is_some() && decision != ClaudeDecisionArg::Deny {
        return Err("--message only applies to deny".into());
    }
    if answers.is_some() && decision != ClaudeDecisionArg::Answer {
        return Err("--answers only applies to answer".into());
    }
    Ok(match decision {
        ClaudeDecisionArg::Allow => ApiClaudeDecision::Allow { always },
        ClaudeDecisionArg::Deny => ApiClaudeDecision::Deny {
            message: message.map(str::to_string),
        },
        ClaudeDecisionArg::Answer => {
            let answers = answers.ok_or("answer needs --answers '{\"<question>\":\"<label>\"}'")?;
            let answers: BTreeMap<String, String> = serde_json::from_str(answers).map_err(|e| {
                format!("--answers must be a JSON object of question -> answer strings: {e}")
            })?;
            ApiClaudeDecision::Answer { answers }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use okena_core::api::{ApiLayoutNode, ApiProject};
    use okena_core::claude_remote::{ApiClaudeDialog, ApiClaudeRunningSubagent};
    use okena_core::theme::FolderColor;
    use std::collections::HashMap;

    fn session(terminal_id: &str) -> ApiClaudeSession {
        ApiClaudeSession {
            terminal_id: terminal_id.into(),
            session_id: Some("s1".into()),
            has_transcript: true,
            hooks_attached: true,
            channel_attached: false,
            busy: Some(true),
            queued_prompts: 2,
            dialogs: vec![ApiClaudeDialog {
                id: "d-1".into(),
                tool_name: "Bash".into(),
                tool_input: serde_json::json!({"command": "ls"}),
                permission_suggestions: vec![],
                created_at_ms: 0,
            }],
            running_subagents: vec![ApiClaudeRunningSubagent {
                agent_id: "a1".into(),
                agent_type: "Explore".into(),
            }],
            pr_links: vec![],
            startup_dialog: None,
            foreground_command: None,
        }
    }

    fn state_with_terminal(terminal_id: &str, name: &str) -> StateResponse {
        let project = ApiProject {
            id: "p1".into(),
            name: "okena".into(),
            path: "/tmp/okena".into(),
            show_in_overview: true,
            layout: Some(ApiLayoutNode::Terminal {
                terminal_id: Some(terminal_id.into()),
                minimized: false,
                detached: false,
                shell_type: Default::default(),
                cols: None,
                rows: None,
            }),
            terminal_names: HashMap::from([(terminal_id.into(), name.into())]),
            status: None,
            terminal_agent_status: HashMap::new(),
            git_status: None,
            folder_color: FolderColor::Default,
            services: vec![],
            worktree_info: None,
            worktree_ids: vec![],
            pinned: false,
            last_activity_at: None,
            default_shell: None,
            hook_terminals: vec![],
            hooks: Default::default(),
            is_creating: false,
            is_closing: false,
            creating_progress: None,
        };
        let mut state: StateResponse = serde_json::from_value(serde_json::json!({
            "state_version": 0,
            "projects": [],
            "focused_project_id": null,
            "fullscreen_terminal": null,
        }))
        .expect("minimal state parses");
        state.projects.push(project);
        state
    }

    #[test]
    fn connected_terminals_join_sessions_and_dialogs() {
        let mut state = state_with_terminal("t1", "claude");
        state.claude_sessions = vec!["t2".into(), "t1".into()];
        state.claude_dialogs.insert("t1".into(), 1);
        state.claude_dialogs.insert("t3".into(), 1);
        assert_eq!(connected_terminal_ids(&state), ["t1", "t2", "t3"]);
    }

    #[test]
    fn ls_row_names_the_terminal_and_counts() {
        let state = state_with_terminal("t1", "claude");
        let row = ls_row(&LabeledSession::new(&state, session("t1")));
        assert_eq!(row, "t1\tokena\tclaude\ts1\tbusy\t1\t2\t1");

        let mut unknown = session("gone");
        unknown.session_id = None;
        unknown.busy = None;
        let row = ls_row(&LabeledSession::new(&state, unknown));
        assert_eq!(row, "gone\t-\t-\t-\t-\t1\t2\t1");

        let mut unnamed = state_with_terminal("t1", "claude");
        unnamed.projects[0].terminal_names.clear();
        let row = ls_row(&LabeledSession::new(&unnamed, session("t1")));
        assert_eq!(row, "t1\tokena\t-\ts1\tbusy\t1\t2\t1");
    }

    #[test]
    fn json_adds_names_to_the_session() {
        let state = state_with_terminal("t1", "claude");
        let value = LabeledSession::new(&state, session("t1"))
            .to_json()
            .expect("encodes");
        assert_eq!(value["project_name"], "okena");
        assert_eq!(value["terminal_name"], "claude");
        assert_eq!(value["dialogs"][0]["tool_input"]["command"], "ls");
    }

    #[test]
    fn show_lists_subagents_and_dialogs_with_their_input() {
        let state = state_with_terminal("t1", "claude");
        let lines = show_lines(&LabeledSession::new(&state, session("t1")));
        assert!(lines.contains(&"state\tbusy".to_string()));
        assert!(lines.contains(&"channel\tno".to_string()));
        assert!(lines.contains(&"subagent\ta1\tExplore".to_string()));
        assert_eq!(
            lines.last().map(String::as_str),
            Some(r#"dialog	d-1	Bash	{"command":"ls"}"#)
        );
    }

    #[test]
    fn show_names_the_startup_dialog() {
        let state = state_with_terminal("t1", "claude");
        let mut waiting = session("t1");
        waiting.startup_dialog = Some(ApiClaudeStartupDialog::Trust);
        let lines = show_lines(&LabeledSession::new(&state, waiting));
        assert!(lines.contains(&"startup_dialog\ttrust".to_string()));
        let lines = show_lines(&LabeledSession::new(&state, session("t1")));
        assert!(lines.contains(&"startup_dialog\t-".to_string()));
    }

    const TRUST_SCREEN: &str = " Accessing workspace:\n ❯ No, exit\n   Yes, I trust this folder";
    const CHANNELS_ON_EXIT: &str = "  WARNING: Loading development channels\n    1. I am using this for local development\n  ❯ 2. Exit";

    #[test]
    fn startup_dialog_key_moves_to_the_confirming_option_first() {
        use ApiClaudeStartupDialog::{Channels, Trust};
        assert_eq!(startup_dialog_key(Trust, TRUST_SCREEN), Some("ArrowDown"));
        let selected = TRUST_SCREEN
            .replace("❯ No", "  No")
            .replace("  Yes", "❯ Yes");
        assert_eq!(startup_dialog_key(Trust, &selected), Some("Enter"));
        assert_eq!(
            startup_dialog_key(Channels, CHANNELS_ON_EXIT),
            Some("ArrowUp")
        );
    }

    #[test]
    fn startup_dialog_key_skips_a_screen_showing_another_dialog() {
        use ApiClaudeStartupDialog::{Channels, Trust};
        assert_eq!(startup_dialog_key(Trust, CHANNELS_ON_EXIT), None);
        assert_eq!(startup_dialog_key(Channels, "$ "), None);
    }

    fn unconnected(foreground: Option<&str>) -> ApiClaudeSession {
        let mut session = session("t1");
        session.hooks_attached = false;
        session.foreground_command = foreground.map(str::to_string);
        session
    }

    #[test]
    fn launch_needs_a_shell_prompt_or_a_waiting_dialog() {
        assert_eq!(launch(&unconnected(None)), Ok(Launch::Type));

        let mut waiting = unconnected(Some("claude"));
        waiting.startup_dialog = Some(ApiClaudeStartupDialog::Trust);
        assert_eq!(launch(&waiting), Ok(Launch::Resume));

        assert!(launch(&unconnected(Some("vim"))).is_err());
        // Claude that runs without Okena's hooks would take `claude` as a prompt.
        assert!(launch(&unconnected(Some("claude"))).is_err());
        assert!(launch(&session("t1")).is_err());
    }

    #[test]
    fn start_steps_follow_the_dialogs_and_the_connection() {
        use ApiClaudeStartupDialog::{Channels, Trust};
        let mut session = unconnected(Some("claude"));
        assert_eq!(start_step(&session, true, false), StartStep::Wait);

        session.startup_dialog = Some(Trust);
        assert_eq!(start_step(&session, true, false), StartStep::NeedsTrust);
        assert_eq!(start_step(&session, true, true), StartStep::Answer(Trust));

        session.startup_dialog = Some(Channels);
        assert_eq!(
            start_step(&session, true, false),
            StartStep::Answer(Channels)
        );

        session.startup_dialog = None;
        session.hooks_attached = true;
        assert_eq!(start_step(&session, true, false), StartStep::Wait);
        assert_eq!(start_step(&session, false, false), StartStep::Connected);
        session.channel_attached = true;
        assert_eq!(start_step(&session, true, false), StartStep::Connected);
    }

    fn turn_session(busy: Option<bool>) -> ApiClaudeSession {
        let mut session = session("t1");
        session.dialogs.clear();
        session.queued_prompts = 0;
        session.busy = busy;
        session
    }

    #[test]
    fn a_turn_ends_once_it_ran_and_the_pane_is_idle() {
        let user = ApiClaudeEntry::User {
            uuid: "u".into(),
            timestamp: None,
            text: "go".into(),
            origin: ApiClaudePromptOrigin::Channel,
        };
        let assistant = ApiClaudeEntry::Assistant {
            uuid: "a".into(),
            timestamp: None,
            text: "done".into(),
        };
        let idle = turn_session(Some(false));

        // The prompt is in the transcript, the hooks have not marked it busy yet.
        let mut progress = TurnProgress::default();
        progress.read(std::slice::from_ref(&assistant));
        progress.read(std::slice::from_ref(&user));
        assert_eq!(progress.observe(&idle), None);
        progress.read(&[assistant]);
        assert_eq!(progress.observe(&idle), Some(TurnState::Idle));

        // A turn without a reply ends once it was seen busy.
        let mut progress = TurnProgress::default();
        assert_eq!(progress.observe(&turn_session(Some(true))), None);
        assert_eq!(progress.observe(&idle), Some(TurnState::Idle));
    }

    #[test]
    fn a_turn_stops_waiting_for_a_dialog_or_a_lost_claude() {
        let mut progress = TurnProgress::default();
        assert_eq!(progress.observe(&session("t1")), Some(TurnState::Dialog));
        let mut gone = turn_session(None);
        gone.hooks_attached = false;
        assert_eq!(progress.observe(&gone), Some(TurnState::Disconnected));
    }

    #[test]
    fn start_result_prints_every_field() {
        let mut started = StartResult {
            terminal_id: "t1".into(),
            session_id: Some("s1".into()),
            delivery: Some(ApiClaudePromptDelivery::Channel),
            state: Some(TurnState::Dialog),
        };
        assert_eq!(
            started.lines(),
            [
                "terminal\tt1",
                "session_id\ts1",
                "delivery\tchannel",
                "state\tdialog"
            ]
        );
        assert_eq!(
            started.to_json(),
            serde_json::json!({
                "terminal": "t1",
                "session_id": "s1",
                "delivery": "channel",
                "state": "dialog",
            })
        );

        started.delivery = None;
        started.state = None;
        assert_eq!(started.lines()[2], "delivery\t-");
        assert_eq!(started.to_json()["delivery"], serde_json::Value::Null);
        assert!(started.to_json().get("state").is_none());
    }

    #[test]
    fn log_lines_keep_each_entry_on_one_line() {
        let page = ApiClaudeTranscriptPage {
            session_id: Some("s1".into()),
            entries: vec![
                ApiClaudeEntry::User {
                    uuid: "u".into(),
                    timestamp: None,
                    text: "fix\tit\nnow".into(),
                    origin: ApiClaudePromptOrigin::Channel,
                },
                ApiClaudeEntry::ToolUse {
                    uuid: "u".into(),
                    timestamp: None,
                    tool_use_id: "tu1".into(),
                    name: "Bash".into(),
                    summary: "cargo test".into(),
                },
                ApiClaudeEntry::ToolResult {
                    uuid: "u".into(),
                    timestamp: None,
                    tool_use_id: "tu1".into(),
                    text: r"C:\dir".into(),
                    is_error: true,
                },
            ],
            cursor: 42,
            reset: true,
        };
        assert_eq!(
            log_lines(&page, "s1:42"),
            [
                "reset\ts1",
                r"user	channel	fix\tit\nnow",
                "tool_use\ttu1\tBash\tcargo test",
                r"tool_result	tu1	error	C:\\dir",
                "cursor\ts1:42",
            ]
        );
    }

    #[test]
    fn cursor_round_trips_with_its_session() {
        let cursor = LogCursor {
            session_id: Some("0b6c-uuid".into()),
            offset: 1234,
        };
        assert_eq!(cursor.to_string(), "0b6c-uuid:1234");
        assert_eq!(LogCursor::parse("0b6c-uuid:1234"), Ok(cursor));
        assert_eq!(
            LogCursor::parse("77"),
            Ok(LogCursor {
                session_id: None,
                offset: 77
            })
        );
        assert!(LogCursor::parse("s1:").is_err());
        assert!(LogCursor::parse(":5").is_err());
        assert!(LogCursor::parse("s1:x").is_err());
    }

    #[test]
    fn transcript_query_sends_the_session_with_the_cursor() {
        let cursor = LogCursor {
            session_id: Some("s 1".into()),
            offset: 9,
        };
        assert_eq!(transcript_query(None, None), "/transcript");
        assert_eq!(
            transcript_query(Some(&cursor), Some("a&b")),
            "/transcript?cursor=9&session_id=s%201&agent_id=a%26b"
        );
    }

    #[test]
    fn decisions_reject_flags_of_another_decision() {
        use ClaudeDecisionArg::{Allow, Answer, Deny};
        assert_eq!(
            build_decision(Allow, true, None, None),
            Ok(ApiClaudeDecision::Allow { always: true })
        );
        assert_eq!(
            build_decision(Deny, false, Some("no"), None),
            Ok(ApiClaudeDecision::Deny {
                message: Some("no".into())
            })
        );
        assert_eq!(
            build_decision(Answer, false, None, Some(r#"{"Which?":"A, B"}"#)),
            Ok(ApiClaudeDecision::Answer {
                answers: BTreeMap::from([("Which?".into(), "A, B".into())])
            })
        );
        assert!(build_decision(Deny, true, None, None).is_err());
        assert!(build_decision(Allow, false, Some("x"), None).is_err());
        assert!(build_decision(Allow, false, None, Some("{}")).is_err());
        assert!(build_decision(Answer, false, None, None).is_err());
        assert!(build_decision(Answer, false, None, Some(r#"{"q":1}"#)).is_err());
    }
}
