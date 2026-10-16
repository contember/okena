//! What a `PermissionRequest` hook prints for a remote client's decision.

use okena_core::claude_remote::ApiClaudeDecision;
use serde_json::{Map, Value, json};

const DEFAULT_DENY_MESSAGE: &str = "Denied from Okena remote";

/// Hook output that leaves the dialog to the terminal.
pub(super) fn cancelled_output() -> Value {
    json!({})
}

/// Hook output answering the dialog for `tool_input` with `decision`.
pub(super) fn decided_output(
    decision: &ApiClaudeDecision,
    tool_input: &Value,
    permission_suggestions: &[Value],
) -> Value {
    json!({
        "hookSpecificOutput": {
            "hookEventName": "PermissionRequest",
            "decision": hook_decision(decision, tool_input, permission_suggestions),
        }
    })
}

fn hook_decision(
    decision: &ApiClaudeDecision,
    tool_input: &Value,
    permission_suggestions: &[Value],
) -> Value {
    match decision {
        ApiClaudeDecision::Allow { always: false } => json!({"behavior": "allow"}),
        ApiClaudeDecision::Allow { always: true } => json!({
            "behavior": "allow",
            "updatedPermissions": session_scoped(permission_suggestions),
        }),
        ApiClaudeDecision::Deny { message } => json!({
            "behavior": "deny",
            "message": message.as_deref().unwrap_or(DEFAULT_DENY_MESSAGE),
        }),
        ApiClaudeDecision::Answer { answers } => {
            // Claude validates `updatedInput` against the tool schema, so the
            // original `questions` must stay next to the answers.
            let mut updated_input = match tool_input {
                Value::Object(fields) => fields.clone(),
                _ => Map::new(),
            };
            updated_input.insert("answers".to_string(), json!(answers));
            json!({"behavior": "allow", "updatedInput": updated_input})
        }
    }
}

/// "Always allow" applies Claude's own suggestions, but only for this session:
/// a phone tap should not edit the user's settings files.
fn session_scoped(permission_suggestions: &[Value]) -> Vec<Value> {
    permission_suggestions
        .iter()
        .map(|suggestion| match suggestion {
            Value::Object(fields) => {
                let mut fields = fields.clone();
                fields.insert("destination".to_string(), json!("session"));
                Value::Object(fields)
            }
            other => other.clone(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn decision_of(output: Value) -> Value {
        assert_eq!(
            output["hookSpecificOutput"]["hookEventName"],
            json!("PermissionRequest")
        );
        output["hookSpecificOutput"]["decision"].clone()
    }

    #[test]
    fn allow_once() {
        let output = decided_output(
            &ApiClaudeDecision::Allow { always: false },
            &json!({"command": "ls"}),
            &[json!({"type": "addRules", "destination": "localSettings"})],
        );
        assert_eq!(
            output,
            json!({"hookSpecificOutput": {
                "hookEventName": "PermissionRequest",
                "decision": {"behavior": "allow"}
            }})
        );
    }

    #[test]
    fn allow_always_scopes_every_suggestion_to_the_session() {
        let suggestions = [
            json!({"type": "addRules", "rules": [{"toolName": "Bash"}], "behavior": "allow", "destination": "localSettings"}),
            json!({"type": "setMode", "mode": "acceptEdits", "destination": "userSettings"}),
        ];
        let decision = decision_of(decided_output(
            &ApiClaudeDecision::Allow { always: true },
            &json!({}),
            &suggestions,
        ));
        assert_eq!(
            decision,
            json!({
                "behavior": "allow",
                "updatedPermissions": [
                    {"type": "addRules", "rules": [{"toolName": "Bash"}], "behavior": "allow", "destination": "session"},
                    {"type": "setMode", "mode": "acceptEdits", "destination": "session"},
                ]
            })
        );
    }

    #[test]
    fn deny_with_and_without_message() {
        let with_message = decision_of(decided_output(
            &ApiClaudeDecision::Deny {
                message: Some("not now".into()),
            },
            &json!({}),
            &[],
        ));
        assert_eq!(
            with_message,
            json!({"behavior": "deny", "message": "not now"})
        );

        let without_message = decision_of(decided_output(
            &ApiClaudeDecision::Deny { message: None },
            &json!({}),
            &[],
        ));
        assert_eq!(
            without_message,
            json!({"behavior": "deny", "message": "Denied from Okena remote"})
        );
    }

    #[test]
    fn answer_keeps_the_original_questions() {
        let tool_input = json!({"questions": [{"question": "Which color?", "options": [{"label": "Red"}, {"label": "Blue"}]}]});
        let answers = BTreeMap::from([("Which color?".to_string(), "Blue".to_string())]);
        let decision = decision_of(decided_output(
            &ApiClaudeDecision::Answer { answers },
            &tool_input,
            &[],
        ));
        assert_eq!(
            decision,
            json!({
                "behavior": "allow",
                "updatedInput": {
                    "questions": [{"question": "Which color?", "options": [{"label": "Red"}, {"label": "Blue"}]}],
                    "answers": {"Which color?": "Blue"}
                }
            })
        );
    }

    #[test]
    fn cancelled_is_an_empty_object() {
        assert_eq!(cancelled_output(), json!({}));
    }
}
