//! okena's MCP server — stdio JSON-RPC, for agents okena is running.
//!
//! Transport choice: stdio rather than HTTP. Agents already know how to launch
//! a stdio MCP server from their own config, and stdio lets this reuse the
//! CLI's existing daemon auth wholesale instead of implementing MCP's
//! streamable-HTTP transport and a second authentication path.
//!
//! Identity is implicit and that is the point: the server reads
//! `$OKENA_TERMINAL_ID` from its own environment, which okena sets on every PTY
//! it spawns. An agent therefore cannot report against a session other than the
//! one it is running in — there is no session parameter to forge. A server
//! launched outside an okena terminal has no identity and refuses the tools
//! that write.
//!
//! Wire up with, in the worktree the agent runs in:
//!
//! ```json
//! { "mcpServers": { "okena": { "command": "okena", "args": ["mcp"] } } }
//! ```

use serde_json::{Value, json};
use std::io::{BufRead, Write};

/// MCP revision this server implements.
const PROTOCOL_VERSION: &str = "2024-11-05";

/// JSON-RPC error codes used here (per the spec's reserved range).
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;

/// Run the server until stdin closes.
pub fn run() -> i32 {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            // stdin closed or went away mid-read: the agent is gone.
            Err(_) => break,
        };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let request: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => {
                // Parse errors carry no id, so there is nothing to respond to;
                // log to stderr, which is not part of the protocol stream.
                eprintln!("okena mcp: could not parse request: {e}");
                continue;
            }
        };

        // A notification has no `id` and takes no response — replying to one
        // corrupts the stream.
        let Some(id) = request.get("id").cloned() else {
            continue;
        };
        let method = request.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let params = request.get("params").cloned().unwrap_or(Value::Null);

        let response = match dispatch(method, &params) {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Err(err) => json!({ "jsonrpc": "2.0", "id": id, "error": err }),
        };

        if writeln!(stdout, "{response}").is_err() || stdout.flush().is_err() {
            break;
        }
    }
    0
}

fn rpc_error(code: i64, message: impl Into<String>) -> Value {
    json!({ "code": code, "message": message.into() })
}

fn dispatch(method: &str, params: &Value) -> Result<Value, Value> {
    match method {
        "initialize" => Ok(json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "okena", "version": env!("CARGO_PKG_VERSION") },
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tool_definitions() })),
        "tools/call" => call_tool(params),
        other => Err(rpc_error(
            METHOD_NOT_FOUND,
            format!("unknown method: {other}"),
        )),
    }
}

fn tool_definitions() -> Value {
    json!([
        {
            "name": "okena_whoami",
            "description":
                "Identify the okena session this agent is running in: terminal, \
                 project, path, and the task it was started for (if any). Call \
                 this first — the other tools act on this session.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
        },
        {
            "name": "okena_list_projects",
            "description":
                "List okena's projects with their paths, branches and worktrees, \
                 including the other worktrees created for the same task.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
        },
        {
            "name": "okena_report_status",
            "description":
                "Report what this agent is currently doing. Shown on the agent's \
                 card in okena's Agents view. Replaces the previous status.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "status": { "type": "string", "description": "Short status line." }
                },
                "required": ["status"],
                "additionalProperties": false
            }
        },
        {
            "name": "okena_list_subtasks",
            "description":
                "List the sub-tasks of a task — every child, including ones \
                 assigned to somebody else. Defaults to this session's own task. \
                 Call before creating children so you do not duplicate one that \
                 already exists.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "task": {
                        "type": "string",
                        "description":
                            "Provider id of the parent. Defaults to this session's task."
                    }
                },
                "additionalProperties": false
            }
        },
        {
            "name": "okena_create_subtask",
            "description":
                "Create a sub-task under a task, in the same team as its parent. \
                 Use this to break work down: one call per child. Prefer several \
                 small children over one large one, and say in the description \
                 what \"done\" means for that child.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "title": { "type": "string", "description": "Short imperative title." },
                    "description": {
                        "type": "string",
                        "description":
                            "What the child covers and what finishing it means."
                    },
                    "kind": {
                        "type": "string",
                        "enum": ["epic", "feature", "story", "task", "defect"],
                        "description":
                            "Where it sits in the breakdown. Defaults to task. A \
                             child is normally narrower than its parent: a feature \
                             under an epic, a story under a feature."
                    },
                    "parent": {
                        "type": "string",
                        "description":
                            "Provider id of the parent. Defaults to this session's task."
                    }
                },
                "required": ["title"],
                "additionalProperties": false
            }
        },
        {
            "name": "okena_register_asset",
            "description":
                "Record something this agent produced — a pull request, branch or \
                 document — against its session. Appends; call once per asset.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "kind": {
                        "type": "string",
                        "enum": ["pull_request", "branch", "document", "other"],
                        "description": "What kind of thing was produced."
                    },
                    "title": { "type": "string", "description": "Human-readable label." },
                    "url": { "type": "string", "description": "Link to it, when there is one." },
                    "project": {
                        "type": "string",
                        "description":
                            "Repo it landed in, when the agent spans several projects."
                    }
                },
                "required": ["kind", "title"],
                "additionalProperties": false
            }
        }
    ])
}

/// Wrap a payload in MCP's tool-result envelope.
///
/// Content is JSON rendered as text: models read it reliably and it keeps the
/// structure intact, which a prose summary would lose.
fn tool_result(value: &Value) -> Value {
    let text = serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string());
    json!({ "content": [{ "type": "text", "text": text }] })
}

/// Report a tool-level failure.
///
/// MCP distinguishes protocol errors from tool errors: a tool that fails should
/// return `isError` so the model can read the reason and adjust, rather than a
/// JSON-RPC error which it never sees.
fn tool_error(message: impl Into<String>) -> Value {
    json!({
        "content": [{ "type": "text", "text": message.into() }],
        "isError": true
    })
}

fn call_tool(params: &Value) -> Result<Value, Value> {
    let name = params
        .get("name")
        .and_then(|n| n.as_str())
        .ok_or_else(|| rpc_error(INVALID_PARAMS, "missing tool name"))?;
    let args = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));

    let outcome = match name {
        "okena_whoami" => whoami(),
        "okena_list_projects" => list_projects(),
        "okena_report_status" => report_status(&args),
        "okena_list_subtasks" => list_subtasks(&args),
        "okena_create_subtask" => create_subtask(&args),
        "okena_register_asset" => register_asset(&args),
        other => {
            return Err(rpc_error(
                METHOD_NOT_FOUND,
                format!("unknown tool: {other}"),
            ));
        }
    };

    Ok(match outcome {
        Ok(value) => tool_result(&value),
        Err(message) => tool_error(message),
    })
}

// ─── Session identity ────────────────────────────────────────────────────────

/// This agent's session: the terminal it runs in and the project that owns it.
struct Session {
    terminal_id: String,
    project: okena_core::api::ApiProject,
}

fn current_session() -> Result<Session, String> {
    let terminal_id = std::env::var("OKENA_TERMINAL_ID").map_err(|_| {
        "Not running inside an okena terminal (OKENA_TERMINAL_ID is unset), so there is \
         no session to act on."
            .to_string()
    })?;

    let token = super::ensure_token()?;
    let state = super::commands::fetch_state(&token)?;
    let project = state
        .projects
        .into_iter()
        .find(|p| {
            p.layout
                .as_ref()
                .is_some_and(|l| layout_has_terminal(l, &terminal_id))
                || p.terminal_names.contains_key(&terminal_id)
        })
        .ok_or_else(|| format!("No okena project owns terminal {terminal_id}."))?;

    Ok(Session {
        terminal_id,
        project,
    })
}

/// Whether `layout` contains `terminal_id`.
fn layout_has_terminal(layout: &okena_core::api::ApiLayoutNode, terminal_id: &str) -> bool {
    use okena_core::api::ApiLayoutNode as N;
    match layout {
        N::Terminal {
            terminal_id: id, ..
        } => id.as_deref() == Some(terminal_id),
        N::Split { children, .. } | N::Tabs { children, .. } => {
            children.iter().any(|c| layout_has_terminal(c, terminal_id))
        }
    }
}

// ─── Tools ───────────────────────────────────────────────────────────────────

fn whoami() -> Result<Value, String> {
    let session = current_session()?;
    Ok(json!({
        "terminal_id": session.terminal_id,
        "project_id": session.project.id,
        "project_name": session.project.name,
        "project_path": session.project.path,
        "branch": session.project.git_status.as_ref().and_then(|g| g.branch.clone()),
        "task": session.project.task_ref,
        "agent": session.project.agent,
    }))
}

fn list_projects() -> Result<Value, String> {
    let token = super::ensure_token()?;
    let state = super::commands::fetch_state(&token)?;
    let projects: Vec<Value> = state
        .projects
        .iter()
        .map(|p| {
            json!({
                "id": p.id,
                "name": p.name,
                "path": p.path,
                "branch": p.git_status.as_ref().and_then(|g| g.branch.clone()),
                "is_worktree": p.worktree_info.is_some(),
                "worktree_ids": p.worktree_ids,
                "task": p.task_ref,
            })
        })
        .collect();
    Ok(json!({ "projects": projects }))
}

fn report_status(args: &Value) -> Result<Value, String> {
    let status = args
        .get("status")
        .and_then(|s| s.as_str())
        .ok_or("`status` is required")?
        .to_string();
    let session = current_session()?;
    let token = super::ensure_token()?;

    super::api_action(
        &token,
        &json!({
            "action": "agent_report_status",
            "project_id": session.project.id,
            "status": status,
        })
        .to_string(),
    )?;
    Ok(json!({ "ok": true, "project_id": session.project.id }))
}

/// The task this session was started for, or an explicit one.
///
/// Defaulting to the session's own task is what makes the breakdown tools
/// usable without the agent having to discover an id first — it is already
/// working on exactly one task.
fn target_task(args: &Value) -> Result<(String, String), String> {
    if let Some(explicit) = args
        .get("parent")
        .or_else(|| args.get("task"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        // An explicit id still needs a provider; this session's is the only
        // one okena has a credential for.
        let session = current_session()?;
        let provider = session
            .project
            .task_ref
            .as_ref()
            .map(|t| t.id.provider.clone())
            .unwrap_or_else(|| "linear".to_string());
        return Ok((provider, explicit.to_string()));
    }
    let session = current_session()?;
    let task = session.project.task_ref.as_ref().ok_or(
        "this session is not linked to a task — pass `parent` to say which task to work under",
    )?;
    Ok((task.id.provider.clone(), task.id.external_id.clone()))
}

fn list_subtasks(args: &Value) -> Result<Value, String> {
    let (provider, task) = target_task(args)?;
    let token = super::ensure_token()?;
    let response = super::api_action(
        &token,
        &json!({
            "action": "task_children",
            "provider": provider,
            "task_external_id": task,
        })
        .to_string(),
    )?;
    Ok(json!({ "parent": task, "children": response }))
}

fn create_subtask(args: &Value) -> Result<Value, String> {
    let title = args
        .get("title")
        .and_then(|s| s.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or("`title` is required")?
        .to_string();
    let (provider, parent) = target_task(args)?;
    let token = super::ensure_token()?;

    let response = super::api_action(
        &token,
        &json!({
            "action": "task_create",
            "provider": provider,
            "title": title,
            "description": args.get("description").and_then(|d| d.as_str()).unwrap_or(""),
            "kind": args.get("kind").and_then(|k| k.as_str()).unwrap_or("task"),
            "parent_external_id": parent,
        })
        .to_string(),
    )?;
    Ok(json!({ "created": response, "parent": parent }))
}

fn register_asset(args: &Value) -> Result<Value, String> {
    let kind = args
        .get("kind")
        .and_then(|k| k.as_str())
        .ok_or("`kind` is required")?
        .to_string();
    let title = args
        .get("title")
        .and_then(|t| t.as_str())
        .ok_or("`title` is required")?
        .to_string();
    let session = current_session()?;
    let token = super::ensure_token()?;

    let mut body = json!({
        "action": "agent_register_asset",
        "project_id": session.project.id,
        "kind": kind,
        "title": title,
    });
    for key in ["url", "project"] {
        if let Some(v) = args
            .get(key)
            .and_then(|v| v.as_str())
            .filter(|v| !v.is_empty())
        {
            body[key] = json!(v);
        }
    }

    let response = super::api_action(&token, &body.to_string())?;
    Ok(json!({ "ok": true, "response": response }))
}

#[cfg(test)]
mod tests {
    use super::{
        INVALID_PARAMS, METHOD_NOT_FOUND, PROTOCOL_VERSION, dispatch, layout_has_terminal,
        tool_definitions, tool_error, tool_result,
    };
    use okena_core::api::ApiLayoutNode;
    use okena_core::shell::ShellType;
    use serde_json::json;

    fn terminal(id: &str) -> ApiLayoutNode {
        ApiLayoutNode::Terminal {
            terminal_id: Some(id.to_string()),
            minimized: false,
            detached: false,
            shell_type: ShellType::Default,
            cols: None,
            rows: None,
        }
    }

    #[test]
    fn initialize_reports_protocol_and_tools_capability() {
        let r = dispatch("initialize", &json!({})).expect("initialize must succeed");
        assert_eq!(r["protocolVersion"], PROTOCOL_VERSION);
        assert!(r["capabilities"]["tools"].is_object());
        assert_eq!(r["serverInfo"]["name"], "okena");
    }

    #[test]
    fn unknown_method_is_a_protocol_error() {
        let e = dispatch("nope", &json!({})).expect_err("must reject");
        assert_eq!(e["code"], METHOD_NOT_FOUND);
    }

    #[test]
    fn tools_list_advertises_every_tool_with_a_schema() {
        let r = dispatch("tools/list", &json!({})).expect("tools/list must succeed");
        let tools = r["tools"].as_array().expect("array");

        // The exact set, not a count: a bare number says nothing about which
        // tool went missing, and adding one should be a deliberate edit here.
        let mut names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
        names.sort_unstable();
        assert_eq!(
            names,
            [
                "okena_create_subtask",
                "okena_list_projects",
                "okena_list_subtasks",
                "okena_register_asset",
                "okena_report_status",
                "okena_whoami",
            ]
        );
        for t in tools {
            assert!(t["name"].as_str().is_some_and(|n| n.starts_with("okena_")));
            assert!(
                t["description"].as_str().is_some_and(|d| !d.is_empty()),
                "every tool needs a description — it is how the model chooses"
            );
            assert_eq!(t["inputSchema"]["type"], "object");
        }
    }

    #[test]
    fn tool_call_without_a_name_is_invalid_params() {
        let e = dispatch("tools/call", &json!({})).expect_err("must reject");
        assert_eq!(e["code"], INVALID_PARAMS);
    }

    #[test]
    fn unknown_tool_is_rejected() {
        let e = dispatch("tools/call", &json!({ "name": "okena_nope" })).expect_err("must reject");
        assert_eq!(e["code"], METHOD_NOT_FOUND);
    }

    #[test]
    fn tool_failures_are_tool_errors_not_protocol_errors() {
        // A model never sees a JSON-RPC error, so a failing tool must come back
        // as a normal result carrying isError.
        let r = dispatch("tools/call", &json!({ "name": "okena_report_status" }))
            .expect("tool call itself succeeds");
        assert_eq!(r["isError"], true);
        assert!(
            r["content"][0]["text"]
                .as_str()
                .is_some_and(|t| t.contains("status")),
            "the reason must be readable by the model"
        );
    }

    #[test]
    fn results_render_json_as_text_content() {
        let r = tool_result(&json!({ "a": 1 }));
        let text = r["content"][0]["text"].as_str().expect("text");
        assert!(text.contains("\"a\""));
        assert!(r.get("isError").is_none());
    }

    #[test]
    fn tool_error_marks_itself() {
        let e = tool_error("boom");
        assert_eq!(e["isError"], true);
        assert_eq!(e["content"][0]["text"], "boom");
    }

    #[test]
    fn finds_a_terminal_nested_in_splits_and_tabs() {
        let layout = ApiLayoutNode::Split {
            direction: okena_core::types::SplitDirection::Horizontal,
            sizes: vec![0.5, 0.5],
            children: vec![
                terminal("a"),
                ApiLayoutNode::Tabs {
                    children: vec![terminal("b"), terminal("target")],
                    active_tab: 0,
                },
            ],
        };
        assert!(layout_has_terminal(&layout, "target"));
        assert!(layout_has_terminal(&layout, "a"));
        assert!(!layout_has_terminal(&layout, "missing"));
    }

    #[test]
    fn every_advertised_tool_is_dispatchable() {
        // A tool advertised but not handled would fail only at call time, in an
        // agent's session, with a confusing "unknown tool".
        for t in tool_definitions().as_array().expect("array") {
            let name = t["name"].as_str().expect("name");
            let r = dispatch("tools/call", &json!({ "name": name }));
            assert!(
                r.is_ok(),
                "advertised tool {name} was rejected as unknown by dispatch"
            );
        }
    }
}
