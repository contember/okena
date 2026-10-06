//! Mission wire contracts. Projects remain the owners of terminals and layouts.

use crate::api::{CiCheckSummary, PrInfo};
use crate::attention::ConversationId;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MissionLifecycle {
    #[default]
    Active,
    Done,
    Archived,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mission {
    pub id: String,
    pub title: String,
    pub goal: Option<String>,
    pub home_project_id: Option<String>,
    pub created_at: u64,
    pub lifecycle: MissionLifecycle,
    #[serde(default)]
    pub repository_ids: Vec<String>,
    #[serde(default)]
    pub worktree_ids: Vec<String>,
    #[serde(default)]
    pub conversations: Vec<ConversationId>,
    #[serde(default)]
    pub pull_requests: Vec<MissionPullRequest>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullRequestIdentity {
    pub host: String,
    pub repository: String,
    pub number: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MissionPullRequest {
    pub identity: PullRequestIdentity,
    pub info: PrInfo,
    pub ci: Option<CiCheckSummary>,
    pub observed_at: u64,
    pub available: bool,
    pub source_project_ids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MissionMember {
    Repository {
        project_id: String,
    },
    Worktree {
        project_id: String,
    },
    Conversation {
        conversation: ConversationId,
    },
    Terminal {
        project_id: String,
        terminal_id: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum MissionCommand {
    Create {
        title: String,
        goal: Option<String>,
        home_project_id: Option<String>,
        member: Option<MissionMember>,
    },
    Edit {
        mission_id: String,
        title: String,
        goal: Option<String>,
        home_project_id: Option<String>,
    },
    SetLifecycle {
        mission_id: String,
        lifecycle: MissionLifecycle,
    },
    Attach {
        mission_id: String,
        member: MissionMember,
    },
    Detach {
        mission_id: String,
        member: MissionMember,
    },
    Move {
        mission_id: String,
        member: MissionMember,
    },
}

pub fn validate_text(title: &str, goal: Option<&str>) -> Result<(), String> {
    if title.trim().is_empty() || title.len() > 256 || title.chars().any(char::is_control) {
        return Err("Mission title must contain 1–256 bytes without control characters".into());
    }
    if goal.is_some_and(|g| g.len() > 4096 || g.contains('\0')) {
        return Err("Mission goal must contain at most 4096 bytes without NUL".into());
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationAttachment {
    pub project_id: String,
    pub terminal_id: String,
    pub conversation: ConversationId,
}

/// Presence is the capability signal; absence means an older daemon.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkOverview {
    pub missions: Vec<Mission>,
    pub attention: Vec<crate::attention::AttentionEpisode>,
    pub lost_transitions: u64,
    pub conversations: Vec<ConversationAttachment>,
    #[serde(default)]
    pub conversation_history: Vec<ConversationId>,
    pub terminal_bindings: Vec<TerminalMissionBinding>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalMissionBinding {
    pub project_id: String,
    pub terminal_id: String,
    pub mission_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MissionContextRequest {
    pub terminal_id: String,
    #[serde(default)]
    pub conversation: Option<ConversationId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MissionContext {
    pub terminal_id: String,
    pub current_project: MissionContextProject,
    pub mission: Option<MissionBriefing>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MissionContextProject {
    pub id: String,
    pub name: String,
    pub path: String,
    pub branch: Option<String>,
    pub is_worktree: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MissionBriefing {
    pub id: String,
    pub title: String,
    pub goal: Option<String>,
    pub lifecycle: MissionLifecycle,
    pub home_project: Option<MissionContextProject>,
    pub projects: Vec<MissionContextProject>,
    pub conversations: Vec<MissionContextConversation>,
    pub omitted_projects: usize,
    pub omitted_conversations: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MissionContextConversation {
    pub conversation: ConversationId,
    /// Last-reported pane attachments, not proof that an agent process is running.
    pub attachments: Vec<ConversationAttachment>,
    pub omitted_attachments: usize,
}
