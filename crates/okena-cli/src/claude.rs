//! `okena claude …`: follow and steer the `claude` sessions running in panes
//! through the same client endpoints the web and mobile chat use
//! (`/v1/claude/terminals/{id}/...`, see `docs/reference/claude-remote.md`).

use crate::commands::{fetch_state, with_state_post};
use crate::parser::ClaudeDecisionArg;
use crate::{api_get, api_post_json, ensure_token, resolve};
use okena_core::api::StateResponse;
use okena_core::claude_remote::{
    ApiClaudeDecision, ApiClaudeEntry, ApiClaudePromptDelivery, ApiClaudePromptOrigin,
    ApiClaudePromptRequest, ApiClaudePromptResponse, ApiClaudeSession, ApiClaudeSubagentList,
    ApiClaudeTranscriptPage,
};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use std::collections::BTreeMap;

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
pub fn cli_prompt(terminal: &str, text: &[String]) -> i32 {
    exit_code((|| {
        let (token, state) = connect()?;
        let (_project_id, terminal_id) = resolve::resolve_terminal(&state, terminal)?;
        let request = ApiClaudePromptRequest {
            text: text.join(" "),
        };
        let body = api_post_json(
            &terminal_path(&terminal_id, "/prompt"),
            &token,
            &to_json(&request)?,
        )?;
        let response: ApiClaudePromptResponse =
            serde_json::from_str(&body).map_err(|e| format!("Invalid prompt response: {e}"))?;
        println!("{}", delivery_label(response.delivery));
        Ok(())
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
    fn log_lines_keep_each_entry_on_one_line() {
        let page = ApiClaudeTranscriptPage {
            session_id: Some("s1".into()),
            entries: vec![
                ApiClaudeEntry::User {
                    uuid: "u".into(),
                    text: "fix\tit\nnow".into(),
                    origin: ApiClaudePromptOrigin::Channel,
                },
                ApiClaudeEntry::ToolUse {
                    uuid: "u".into(),
                    tool_use_id: "tu1".into(),
                    name: "Bash".into(),
                    summary: "cargo test".into(),
                },
                ApiClaudeEntry::ToolResult {
                    uuid: "u".into(),
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
