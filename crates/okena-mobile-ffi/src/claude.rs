//! Claude remote: follow and steer a `claude` session running in a pane over
//! the server's `/v1/claude/terminals/{terminal_id}` routes.
//!
//! Responses are parsed into the `okena-core` wire types before they are handed
//! to JS as JSON strings, so a server speaking another shape fails here rather
//! than deep inside the UI. The transcript is the exception: it is forwarded
//! as the server sent it, so entry kinds added by a newer daemon reach the RN
//! parser, which skips the ones it doesn't know.

use okena_core::claude_remote::{
    ApiClaudeDecision, ApiClaudePromptRequest, ApiClaudePromptResponse, ApiClaudeSession,
};
use okena_transport::remote_action::{JsonRequestError, get_json_async, post_json_async};

use crate::MobileFfiError;
use crate::client::manager::ConnectionManager;

/// Error message for answering a dialog the server no longer has open. The RN
/// client matches on it to drop the dialog card.
pub(crate) const DIALOG_GONE_MESSAGE: &str = "dialog already answered";

const NOT_FOUND: u16 = 404;

fn terminal_route<'a>(terminal_id: &'a str, rest: &[&'a str]) -> Vec<&'a str> {
    let mut segments = vec!["v1", "claude", "terminals", terminal_id];
    segments.extend_from_slice(rest);
    segments
}

fn transcript_query<'a>(cursor: &'a str, session_id: Option<&'a str>) -> Vec<(&'a str, &'a str)> {
    let mut query = vec![("cursor", cursor)];
    if let Some(session_id) = session_id {
        query.push(("session_id", session_id));
    }
    query
}

fn parse_decision(decision_json: &str) -> Result<ApiClaudeDecision, MobileFfiError> {
    serde_json::from_str(decision_json).map_err(|error| MobileFfiError::Action {
        message: format!("Invalid dialog decision: {error}"),
    })
}

/// Maps a failed request to the message the app shows. The server's plain-text
/// reason (e.g. the 409 for an undeliverable prompt) is passed through as is.
fn request_error(error: JsonRequestError, not_found_message: Option<&str>) -> MobileFfiError {
    let message = match (error, not_found_message) {
        (
            JsonRequestError::Status {
                status: NOT_FOUND, ..
            },
            Some(message),
        ) => message.to_string(),
        (JsonRequestError::Status { status, body }, _) if body.trim().is_empty() => {
            format!("Server returned {status}")
        }
        (JsonRequestError::Status { body, .. }, _) => body.trim().to_string(),
        (JsonRequestError::Transport(message), _) => message,
    };
    MobileFfiError::Action { message }
}

fn encode_error(error: serde_json::Error) -> MobileFfiError {
    MobileFfiError::Action {
        message: format!("Cannot encode response: {error}"),
    }
}

pub(crate) async fn get_session(
    conn_id: &str,
    terminal_id: &str,
) -> Result<String, MobileFfiError> {
    let (config, token) = ConnectionManager::get().http_auth(conn_id)?;
    let session: ApiClaudeSession =
        get_json_async(&config, &token, &terminal_route(terminal_id, &[]), &[])
            .await
            .map_err(|error| request_error(error, None))?;
    serde_json::to_string(&session).map_err(encode_error)
}

pub(crate) async fn get_transcript(
    conn_id: &str,
    terminal_id: &str,
    cursor: u64,
    session_id: Option<&str>,
) -> Result<String, MobileFfiError> {
    let (config, token) = ConnectionManager::get().http_auth(conn_id)?;
    let cursor = cursor.to_string();
    let page: serde_json::Value = get_json_async(
        &config,
        &token,
        &terminal_route(terminal_id, &["transcript"]),
        &transcript_query(&cursor, session_id),
    )
    .await
    .map_err(|error| request_error(error, None))?;
    serde_json::to_string(&page).map_err(encode_error)
}

pub(crate) async fn send_prompt(
    conn_id: &str,
    terminal_id: &str,
    text: String,
) -> Result<String, MobileFfiError> {
    let (config, token) = ConnectionManager::get().http_auth(conn_id)?;
    let response: ApiClaudePromptResponse = post_json_async(
        &config,
        &token,
        &terminal_route(terminal_id, &["prompt"]),
        &ApiClaudePromptRequest { text },
    )
    .await
    .map_err(|error| request_error(error, None))?;
    serde_json::to_string(&response).map_err(encode_error)
}

pub(crate) async fn answer_dialog(
    conn_id: &str,
    terminal_id: &str,
    dialog_id: &str,
    decision_json: &str,
) -> Result<(), MobileFfiError> {
    let decision = parse_decision(decision_json)?;
    let (config, token) = ConnectionManager::get().http_auth(conn_id)?;
    let _: serde_json::Value = post_json_async(
        &config,
        &token,
        &terminal_route(terminal_id, &["dialogs", dialog_id]),
        &decision,
    )
    .await
    .map_err(|error| request_error(error, Some(DIALOG_GONE_MESSAGE)))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn message(error: MobileFfiError) -> String {
        match error {
            MobileFfiError::Action { message } => message,
        }
    }

    #[test]
    fn routes_live_under_the_terminal() {
        assert_eq!(
            terminal_route("t1", &["dialogs", "d1"]),
            ["v1", "claude", "terminals", "t1", "dialogs", "d1"]
        );
        assert_eq!(
            terminal_route("t1", &[]),
            ["v1", "claude", "terminals", "t1"]
        );
    }

    #[test]
    fn transcript_query_sends_the_session_only_when_known() {
        assert_eq!(transcript_query("0", None), [("cursor", "0")]);
        assert_eq!(
            transcript_query("42", Some("s1")),
            [("cursor", "42"), ("session_id", "s1")]
        );
    }

    #[test]
    fn decisions_parse_every_wire_shape() {
        assert_eq!(
            parse_decision(r#"{"decision":"allow"}"#).ok(),
            Some(ApiClaudeDecision::Allow { always: false })
        );
        assert_eq!(
            parse_decision(r#"{"decision":"deny","message":"no"}"#).ok(),
            Some(ApiClaudeDecision::Deny {
                message: Some("no".to_string())
            })
        );
        assert_eq!(
            parse_decision(r#"{"decision":"answer","answers":{"Which?":"A, B"}}"#).ok(),
            Some(ApiClaudeDecision::Answer {
                answers: BTreeMap::from([("Which?".to_string(), "A, B".to_string())])
            })
        );
    }

    #[test]
    fn malformed_decisions_are_rejected_before_sending() {
        for json in [
            r#"{"decision":"maybe"}"#,
            r#"{"decision":"allow","extra":true}"#,
            r#"{"decision":"answer"}"#,
            "not json",
        ] {
            let error = message(parse_decision(json).unwrap_err());
            assert!(
                error.starts_with("Invalid dialog decision: "),
                "{json}: {error}"
            );
        }
    }

    #[test]
    fn dialog_not_found_reads_as_already_answered() {
        let error = JsonRequestError::Status {
            status: 404,
            body: String::new(),
        };
        assert_eq!(
            message(request_error(error, Some(DIALOG_GONE_MESSAGE))),
            DIALOG_GONE_MESSAGE
        );
    }

    #[test]
    fn server_reasons_pass_through() {
        let conflict = JsonRequestError::Status {
            status: 409,
            body: "Claude is busy\n".to_string(),
        };
        assert_eq!(
            message(request_error(conflict, Some(DIALOG_GONE_MESSAGE))),
            "Claude is busy"
        );

        let not_found = JsonRequestError::Status {
            status: 404,
            body: "no transcript".to_string(),
        };
        assert_eq!(message(request_error(not_found, None)), "no transcript");
    }

    #[test]
    fn empty_error_bodies_name_the_status() {
        let error = JsonRequestError::Status {
            status: 400,
            body: " ".to_string(),
        };
        assert_eq!(message(request_error(error, None)), "Server returned 400");
    }

    #[test]
    fn transport_failures_keep_their_message() {
        let error = JsonRequestError::Transport("HTTP request failed: refused".to_string());
        assert_eq!(
            message(request_error(error, None)),
            "HTTP request failed: refused"
        );
    }
}
