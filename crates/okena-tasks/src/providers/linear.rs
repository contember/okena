//! Linear provider — GraphQL over `okena_transport::http`.
//!
//! Why GraphQL rather than Linear's MCP server: the harness needs typed rows it
//! can sort, diff against the previous poll, and render in a table. MCP is an
//! agent-facing protocol whose tool output is shaped for a model to read, with
//! no stable schema across server versions. The MCP server still has a role —
//! the *agent* spawned in a task's worktree talks to it — but it is handed the
//! same OAuth token this provider holds rather than being the harness's own
//! data path.

use crate::provider::{AuthStatus, Credential, TaskError, TaskProvider};
use okena_core::tasks::{Task, TaskId, TaskKind, TaskState};
use okena_transport::http::{self, HttpError, HttpRequest};
use std::time::Duration;

const PROVIDER_ID: &str = "linear";
const API_URL: &str = "https://api.linear.app/graphql";
/// Client-side rate floor. Well below the real poll cadence — it exists only to
/// catch a runaway caller, never a legitimate refresh.
const MIN_INTERVAL: Duration = Duration::from_secs(5);
const TIMEOUT: Duration = Duration::from_secs(20);
/// Linear caps page size at 250; the assigned-work queue is far smaller.
const PAGE_SIZE: u32 = 100;

/// Assigned, still-open issues, most recently updated first.
///
/// The `state.type` filter runs server-side so a large finished backlog is
/// never transferred. Linear's state types are a closed set:
/// `triage | backlog | unstarted | started | completed | canceled`.
const QUERY_ASSIGNED: &str = r#"
query AssignedIssues($first: Int!) {
  viewer {
    id
    name
    assignedIssues(
      first: $first
      filter: { state: { type: { nin: ["completed", "canceled"] } } }
      orderBy: updatedAt
    ) {
      nodes {
        id
        identifier
        title
        description
        url
        branchName
        updatedAt
        state { name type }
        parent { id identifier }
        labels(first: 20) { nodes { name } }
      }
    }
  }
}
"#;

/// The workflow states available to the issue's own team.
///
/// A state cannot be set by category — `issueUpdate` takes a concrete state id,
/// and those are per-team — so a state change is always resolve-then-mutate.
const QUERY_ISSUE_STATES: &str = r#"
query IssueStates($id: String!) {
  issue(id: $id) {
    id
    team {
      states(first: 100) {
        nodes { id name type position }
      }
    }
  }
}
"#;

const MUTATION_SET_STATE: &str = r#"
mutation SetState($id: String!, $stateId: String!) {
  issueUpdate(id: $id, input: { stateId: $stateId }) {
    success
  }
}
"#;

pub struct LinearProvider {
    credential: Option<Credential>,
    /// Cached display name of the authenticated account, filled by the first
    /// successful `list_assigned` (the same query returns `viewer`).
    account: std::sync::RwLock<Option<String>>,
}

impl LinearProvider {
    pub fn new(credential: Option<Credential>) -> Self {
        Self {
            credential,
            account: std::sync::RwLock::new(None),
        }
    }

    fn credential(&self) -> Result<&Credential, TaskError> {
        self.credential.as_ref().ok_or(TaskError::NotAuthenticated {
            provider: PROVIDER_ID,
        })
    }

    /// Apply Linear's auth header.
    ///
    /// Linear takes a personal API key *raw* in `Authorization` but an OAuth
    /// access token as `Bearer <token>`. Sending a personal key with a `Bearer`
    /// prefix is rejected, which is the whole reason [`Credential`] keeps the
    /// two kinds apart rather than collapsing them into one string.
    fn authorize(req: HttpRequest, cred: &Credential) -> HttpRequest {
        match cred {
            Credential::ApiKey(key) => req.header("Authorization", key.clone()),
            Credential::OAuth { access_token, .. } => req.bearer(access_token),
        }
    }

    /// Issue a GraphQL request and return the `data` object.
    fn graphql(
        &self,
        label: &'static str,
        query: &str,
        variables: serde_json::Value,
    ) -> Result<serde_json::Value, TaskError> {
        let cred = self.credential()?;
        let req = Self::authorize(
            HttpRequest::post(API_URL)
                .json(&serde_json::json!({ "query": query, "variables": variables }))
                .label(label)
                .min_interval(MIN_INTERVAL)
                .timeout(TIMEOUT),
            cred,
        );

        let resp = http::send(req).map_err(|e| match e {
            // 401/403 mean the credential is bad — surfaced distinctly so the
            // UI prompts for re-auth instead of silently retrying forever.
            HttpError::Status(401) | HttpError::Status(403) => TaskError::Unauthorized {
                provider: PROVIDER_ID,
            },
            other => TaskError::Transport {
                provider: PROVIDER_ID,
                message: other.to_string(),
            },
        })?;

        if resp.status() == 401 || resp.status() == 403 {
            return Err(TaskError::Unauthorized {
                provider: PROVIDER_ID,
            });
        }
        if !resp.is_success() {
            return Err(TaskError::Transport {
                provider: PROVIDER_ID,
                message: format!("HTTP {}", resp.status()),
            });
        }

        let body: serde_json::Value = resp.json().map_err(|e| TaskError::Protocol {
            provider: PROVIDER_ID,
            message: e.to_string(),
        })?;

        // GraphQL reports application errors inside a 200 response.
        if let Some(errors) = body.get("errors").and_then(|e| e.as_array())
            && !errors.is_empty()
        {
            let joined = errors
                .iter()
                .filter_map(|e| e.get("message").and_then(|m| m.as_str()))
                .collect::<Vec<_>>()
                .join("; ");
            let message = if joined.is_empty() {
                "GraphQL error".to_string()
            } else {
                joined
            };
            // Linear reports an invalid/expired token as a GraphQL error rather
            // than an HTTP status on some endpoints.
            if message.to_ascii_lowercase().contains("authentication") {
                return Err(TaskError::Unauthorized {
                    provider: PROVIDER_ID,
                });
            }
            return Err(TaskError::Protocol {
                provider: PROVIDER_ID,
                message,
            });
        }

        body.get("data")
            .cloned()
            .ok_or_else(|| TaskError::Protocol {
                provider: PROVIDER_ID,
                message: "response had no `data`".into(),
            })
    }
}

/// Map a Linear workflow-state type onto a normalized category.
///
/// Linear has no distinct "in review" type — review columns are `started` — so
/// [`TaskState::InReview`] is never produced here. It exists for providers that
/// model review as its own category (Jira does).
fn map_state(type_: &str) -> TaskState {
    match type_ {
        "backlog" | "triage" => TaskState::Backlog,
        "unstarted" => TaskState::Todo,
        "started" => TaskState::InProgress,
        "completed" => TaskState::Done,
        "canceled" => TaskState::Canceled,
        _ => TaskState::Unknown,
    }
}

/// The Linear state type to move to for a normalized category.
fn target_state_type(state: TaskState) -> &'static str {
    match state {
        TaskState::Backlog => "backlog",
        TaskState::Todo => "unstarted",
        // Linear folds review into `started`; see `map_state`.
        TaskState::InProgress | TaskState::InReview => "started",
        TaskState::Done => "completed",
        TaskState::Canceled => "canceled",
        TaskState::Unknown => "unstarted",
    }
}

/// Parse one issue node. Returns `None` for a node missing the fields the
/// harness cannot work without, so one malformed row can't fail the whole poll.
fn parse_issue(node: &serde_json::Value) -> Option<Task> {
    let str_at = |k: &str| node.get(k).and_then(|v| v.as_str());
    let id = str_at("id")?;
    let identifier = str_at("identifier")?;
    let state = node.get("state");
    let labels: Vec<String> = node
        .get("labels")
        .and_then(|l| l.get("nodes"))
        .and_then(|n| n.as_array())
        .map(|nodes| {
            nodes
                .iter()
                .filter_map(|l| l.get("name").and_then(|v| v.as_str()).map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let parent = node.get("parent").and_then(|p| {
        let id = p.get("id").and_then(|v| v.as_str())?;
        let key = p.get("identifier").and_then(|v| v.as_str()).unwrap_or(id);
        Some((id.to_string(), key.to_string()))
    });
    let state_type = state
        .and_then(|s| s.get("type"))
        .and_then(|v| v.as_str())
        .unwrap_or("");

    Some(Task {
        id: TaskId::new(PROVIDER_ID, id),
        display_key: identifier.to_string(),
        title: str_at("title").unwrap_or_default().to_string(),
        description: str_at("description").map(str::to_string),
        state: map_state(state_type),
        state_name: state
            .and_then(|s| s.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        url: str_at("url").unwrap_or_default().to_string(),
        // Linear supplies its own branch name; using it keeps Linear's
        // branch-to-issue automation working.
        branch_name: str_at("branchName").unwrap_or_default().to_string(),
        updated_at: str_at("updatedAt").unwrap_or_default().to_string(),
        // Linear has no issue-type field, so the breakdown level is derived
        // from labels. A sub-issue with no telling label falls back to Story
        // rather than Task: it sits under something, which is what a story is.
        kind: TaskKind::from_labels(labels.iter().map(String::as_str)).unwrap_or({
            if parent.is_some() {
                TaskKind::Story
            } else {
                TaskKind::Task
            }
        }),
        parent_id: parent.as_ref().map(|(id, _)| id.clone()),
        parent_key: parent.as_ref().map(|(_, key)| key.clone()),
        labels,
    })
}

impl TaskProvider for LinearProvider {
    fn id(&self) -> &'static str {
        PROVIDER_ID
    }

    fn display_name(&self) -> &'static str {
        "Linear"
    }

    fn auth_status(&self) -> AuthStatus {
        match &self.credential {
            None => AuthStatus::Disconnected,
            Some(c) => {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                if c.is_expired(now, 60) {
                    AuthStatus::Expired
                } else {
                    AuthStatus::Connected {
                        account: self.account.read().ok().and_then(|a| a.clone()),
                    }
                }
            }
        }
    }

    fn list_assigned(&self) -> Result<Vec<Task>, TaskError> {
        let data = self.graphql(
            "linear.assigned",
            QUERY_ASSIGNED,
            serde_json::json!({ "first": PAGE_SIZE }),
        )?;

        let viewer = data.get("viewer").ok_or_else(|| TaskError::Protocol {
            provider: PROVIDER_ID,
            message: "response had no `viewer`".into(),
        })?;

        if let Some(name) = viewer.get("name").and_then(|v| v.as_str())
            && let Ok(mut slot) = self.account.write()
        {
            *slot = Some(name.to_string());
        }

        let nodes = viewer
            .get("assignedIssues")
            .and_then(|a| a.get("nodes"))
            .and_then(|n| n.as_array())
            .ok_or_else(|| TaskError::Protocol {
                provider: PROVIDER_ID,
                message: "response had no `assignedIssues.nodes`".into(),
            })?;

        let total = nodes.len();
        let tasks: Vec<Task> = nodes.iter().filter_map(parse_issue).collect();
        if tasks.len() != total {
            log::warn!(
                "[tasks] linear: skipped {} malformed issue node(s)",
                total - tasks.len()
            );
        }
        Ok(tasks)
    }

    fn set_state(&self, id: &TaskId, state: TaskState) -> Result<(), TaskError> {
        if id.provider != PROVIDER_ID {
            return Err(TaskError::Protocol {
                provider: PROVIDER_ID,
                message: format!("task belongs to provider `{}`", id.provider),
            });
        }

        let want = target_state_type(state);
        let data = self.graphql(
            "linear.issue_states",
            QUERY_ISSUE_STATES,
            serde_json::json!({ "id": id.external_id }),
        )?;

        let states = data
            .get("issue")
            .and_then(|i| i.get("team"))
            .and_then(|t| t.get("states"))
            .and_then(|s| s.get("nodes"))
            .and_then(|n| n.as_array())
            .ok_or_else(|| TaskError::Protocol {
                provider: PROVIDER_ID,
                message: "could not read the issue's team workflow states".into(),
            })?;

        // A team may define several states of the same type ("In Progress",
        // "In Review" are both `started`). Lowest `position` is the team's
        // leftmost, which is the canonical entry point for that category.
        let chosen = states
            .iter()
            .filter(|s| s.get("type").and_then(|v| v.as_str()) == Some(want))
            .min_by(|a, b| {
                let pos = |v: &serde_json::Value| {
                    v.get("position")
                        .and_then(|p| p.as_f64())
                        .unwrap_or(f64::MAX)
                };
                pos(a).total_cmp(&pos(b))
            })
            .and_then(|s| s.get("id").and_then(|v| v.as_str()))
            .ok_or_else(|| TaskError::Protocol {
                provider: PROVIDER_ID,
                message: format!("the issue's team has no `{want}` workflow state"),
            })?;

        let data = self.graphql(
            "linear.set_state",
            MUTATION_SET_STATE,
            serde_json::json!({ "id": id.external_id, "stateId": chosen }),
        )?;

        let ok = data
            .get("issueUpdate")
            .and_then(|u| u.get("success"))
            .and_then(|s| s.as_bool())
            .unwrap_or(false);
        if ok {
            Ok(())
        } else {
            Err(TaskError::Protocol {
                provider: PROVIDER_ID,
                message: "issueUpdate reported failure".into(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_linear_state_types() {
        assert_eq!(map_state("backlog"), TaskState::Backlog);
        assert_eq!(map_state("triage"), TaskState::Backlog);
        assert_eq!(map_state("unstarted"), TaskState::Todo);
        assert_eq!(map_state("started"), TaskState::InProgress);
        assert_eq!(map_state("completed"), TaskState::Done);
        assert_eq!(map_state("canceled"), TaskState::Canceled);
        // An unrecognized type must not masquerade as a real category.
        assert_eq!(map_state("something-new"), TaskState::Unknown);
    }

    #[test]
    fn closed_categories_are_closed() {
        assert!(TaskState::Done.is_closed());
        assert!(TaskState::Canceled.is_closed());
        assert!(!TaskState::InProgress.is_closed());
    }

    #[test]
    fn parses_a_full_issue_node() {
        let node = serde_json::json!({
            "id": "uuid-1",
            "identifier": "LIN-42",
            "title": "Fix the thing",
            "description": "details",
            "url": "https://linear.app/x/issue/LIN-42",
            "branchName": "nima/lin-42-fix-the-thing",
            "updatedAt": "2026-08-26T10:00:00.000Z",
            "state": { "name": "In Progress", "type": "started" }
        });
        let t = parse_issue(&node).expect("should parse");
        assert_eq!(t.id, TaskId::new("linear", "uuid-1"));
        assert_eq!(t.display_key, "LIN-42");
        assert_eq!(t.state, TaskState::InProgress);
        assert_eq!(t.state_name, "In Progress");
        assert_eq!(t.branch_name, "nima/lin-42-fix-the-thing");
    }

    #[test]
    fn rejects_node_without_identity() {
        // No `id` — unusable, must be skipped rather than defaulted.
        let node = serde_json::json!({ "identifier": "LIN-1", "title": "x" });
        assert!(parse_issue(&node).is_none());
    }

    #[test]
    fn tolerates_missing_optional_fields() {
        let node = serde_json::json!({ "id": "u", "identifier": "LIN-2" });
        let t = parse_issue(&node).expect("identity present, should parse");
        assert_eq!(t.title, "");
        assert_eq!(t.description, None);
        assert_eq!(t.state, TaskState::Unknown);
    }

    #[test]
    fn uses_linear_branch_name_when_present() {
        let p = LinearProvider::new(None);
        let t = parse_issue(&serde_json::json!({
            "id": "u", "identifier": "LIN-3", "title": "Some title",
            "branchName": "nima/lin-3-some-title"
        }))
        .unwrap();
        assert_eq!(p.branch_name(&t), "nima/lin-3-some-title");
    }

    #[test]
    fn falls_back_to_slug_when_provider_gives_no_branch() {
        let p = LinearProvider::new(None);
        let t = parse_issue(&serde_json::json!({
            "id": "u", "identifier": "LIN-4", "title": "Some Title"
        }))
        .unwrap();
        assert_eq!(p.branch_name(&t), "lin-4-some-title");
    }

    #[test]
    fn api_key_goes_raw_and_oauth_gets_bearer() {
        // Linear rejects a personal API key sent with a `Bearer` prefix, so the
        // two credential kinds must produce different headers.
        let raw = LinearProvider::authorize(
            HttpRequest::post(API_URL),
            &Credential::ApiKey("lin_api_xyz".into()),
        );
        assert_eq!(raw.header_value("Authorization"), Some("lin_api_xyz"));

        let oauth = LinearProvider::authorize(
            HttpRequest::post(API_URL),
            &Credential::OAuth {
                access_token: "tok".into(),
                refresh_token: None,
                expires_at: None,
            },
        );
        assert_eq!(oauth.header_value("Authorization"), Some("Bearer tok"));
    }

    #[test]
    fn unauthenticated_provider_reports_disconnected() {
        let p = LinearProvider::new(None);
        assert_eq!(p.auth_status(), AuthStatus::Disconnected);
        assert!(matches!(
            p.list_assigned(),
            Err(TaskError::NotAuthenticated { .. })
        ));
    }

    #[test]
    fn set_state_rejects_foreign_provider_task() {
        let p = LinearProvider::new(Some(Credential::ApiKey("k".into())));
        let foreign = TaskId::new("jira", "ABC-1");
        assert!(matches!(
            p.set_state(&foreign, TaskState::Done),
            Err(TaskError::Protocol { .. })
        ));
    }
}
