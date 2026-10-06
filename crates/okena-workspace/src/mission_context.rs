use crate::missions::{terminal_mission, terminal_mission_for_conversation};
use crate::state::{ProjectData, WorkspaceData};
use okena_core::api::ApiGitStatus;
use okena_core::attention::ConversationId;
use okena_core::mission::{
    ConversationAttachment, MissionBriefing, MissionContext, MissionContextConversation,
    MissionContextProject, MissionContextRequest,
};
use std::collections::{HashMap, HashSet};

const MAX_CONTEXT_MEMBERS: usize = 12;

pub fn mission_context(
    data: &WorkspaceData,
    request: &MissionContextRequest,
    git_statuses: &HashMap<String, ApiGitStatus>,
) -> Result<MissionContext, String> {
    if request.conversation.as_ref().is_some_and(|c| !c.is_valid()) {
        return Err("Invalid agent conversation identity".into());
    }
    let project = data
        .projects
        .iter()
        .filter(|p| p.connection_id.is_none())
        .find(|p| {
            p.layout
                .as_ref()
                .is_some_and(|l| l.find_terminal_path(&request.terminal_id).is_some())
        })
        .ok_or_else(|| "Terminal not found on this daemon".to_string())?;
    let mission_id = if let Some(conversation) = &request.conversation {
        terminal_mission_for_conversation(
            data,
            &project.id,
            &request.terminal_id,
            Some(conversation),
        )
    } else {
        terminal_mission(data, &project.id, &request.terminal_id)
    };
    let mission = mission_id.and_then(|id| data.missions.iter().find(|m| m.id == id));
    let briefing = mission.map(|mission| {
        let mut project_ids: HashSet<&str> = mission
            .repository_ids
            .iter()
            .chain(&mission.worktree_ids)
            .map(String::as_str)
            .collect();
        project_ids.insert(&project.id);
        let mut attachments: HashMap<ConversationId, Vec<ConversationAttachment>> = HashMap::new();
        for member_project in data.projects.iter().filter(|p| p.connection_id.is_none()) {
            if let Some(layout) = &member_project.layout {
                for terminal in layout.collect_terminal_ids() {
                    if terminal_mission(data, &member_project.id, &terminal) == Some(&mission.id) {
                        project_ids.insert(&member_project.id);
                    }
                    if let Some(session) = member_project.agent_sessions.get(&terminal) {
                        let conversation = ConversationId::from(session);
                        if mission.conversations.contains(&conversation) {
                            attachments.entry(conversation.clone()).or_default().push(
                                ConversationAttachment {
                                    project_id: member_project.id.clone(),
                                    terminal_id: terminal,
                                    conversation,
                                },
                            );
                        }
                    }
                }
            }
        }
        let mut projects: Vec<_> = data
            .projects
            .iter()
            .filter(|p| p.connection_id.is_none() && project_ids.contains(p.id.as_str()))
            .map(|p| context_project(p, git_statuses))
            .collect();
        projects.sort_by(|a, b| (a.id != project.id, &a.id).cmp(&(b.id != project.id, &b.id)));
        let omitted_projects = projects.len().saturating_sub(MAX_CONTEXT_MEMBERS);
        projects.truncate(MAX_CONTEXT_MEMBERS);
        let mut conversations = mission.conversations.clone();
        conversations.sort_by(|a, b| (&a.agent, &a.session_id).cmp(&(&b.agent, &b.session_id)));
        conversations.dedup();
        let omitted_conversations = conversations.len().saturating_sub(MAX_CONTEXT_MEMBERS);
        conversations.truncate(MAX_CONTEXT_MEMBERS);
        MissionBriefing {
            id: mission.id.clone(),
            title: mission.title.clone(),
            goal: mission.goal.clone(),
            lifecycle: mission.lifecycle,
            home_project: mission.home_project_id.as_ref().and_then(|id| {
                data.projects
                    .iter()
                    .find(|p| p.id == *id && p.connection_id.is_none())
                    .map(|p| context_project(p, git_statuses))
            }),
            projects,
            conversations: conversations
                .into_iter()
                .map(|conversation| {
                    let mut panes = attachments.remove(&conversation).unwrap_or_default();
                    panes.sort_by(|a, b| {
                        (&a.project_id, &a.terminal_id).cmp(&(&b.project_id, &b.terminal_id))
                    });
                    let omitted_attachments = panes.len().saturating_sub(MAX_CONTEXT_MEMBERS);
                    panes.truncate(MAX_CONTEXT_MEMBERS);
                    MissionContextConversation {
                        conversation,
                        attachments: panes,
                        omitted_attachments,
                    }
                })
                .collect(),
            omitted_projects,
            omitted_conversations,
        }
    });
    Ok(MissionContext {
        terminal_id: request.terminal_id.clone(),
        current_project: context_project(project, git_statuses),
        mission: briefing,
    })
}

fn context_project(
    project: &ProjectData,
    git_statuses: &HashMap<String, ApiGitStatus>,
) -> MissionContextProject {
    MissionContextProject {
        id: project.id.clone(),
        name: project.name.clone(),
        path: project.path.clone(),
        branch: git_statuses.get(&project.id).and_then(|s| s.branch.clone()),
        is_worktree: project.worktree_info.is_some(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::missions::apply_command;
    use okena_core::agent_session::AgentSession;
    use okena_core::mission::{MissionCommand, MissionMember};

    fn fixture() -> WorkspaceData {
        serde_json::from_value(serde_json::json!({
            "projects": [
                {"id":"p", "name":"Repo", "path":"/repo", "layout":{"type":"terminal", "terminal_id":"t"}},
                {"id":"w", "name":"Worktree", "path":"/worktree", "layout":{"type":"terminal", "terminal_id":"tw"}, "worktree_info":{"parent_project_id":"p"}}
            ], "project_order":["p", "w"]
        })).unwrap()
    }

    fn conversation(n: u64) -> ConversationId {
        ConversationId {
            agent: "claude-code".into(),
            session_id: format!("00000000-0000-0000-0000-{n:012x}"),
        }
    }

    fn record(data: &mut WorkspaceData, conversation: &ConversationId) -> AgentSession {
        let session = AgentSession {
            agent: conversation.agent.clone(),
            session_id: conversation.session_id.clone(),
            transcript_path: Some("/private/transcript.jsonl".into()),
        };
        data.agent_session_history.record(session.clone());
        session
    }

    fn create(data: &mut WorkspaceData, member: MissionMember) -> String {
        apply_command(
            data,
            MissionCommand::Create {
                title: "Export".into(),
                goal: Some("Add CSV export".into()),
                home_project_id: Some("p".into()),
                member: Some(member),
            },
        )
        .unwrap()
    }

    fn query(
        data: &WorkspaceData,
        terminal: &str,
        current: Option<ConversationId>,
    ) -> MissionContext {
        mission_context(
            data,
            &MissionContextRequest {
                terminal_id: terminal.into(),
                conversation: current,
            },
            &HashMap::new(),
        )
        .unwrap()
    }

    #[test]
    fn repository_membership_does_not_assign_its_terminals() {
        let mut data = fixture();
        create(
            &mut data,
            MissionMember::Repository {
                project_id: "p".into(),
            },
        );
        assert!(query(&data, "t", Some(conversation(1))).mission.is_none());
    }

    #[test]
    fn worktree_defaults_apply_before_the_first_session_report_without_mutation() {
        let mut data = fixture();
        let id = create(
            &mut data,
            MissionMember::Worktree {
                project_id: "w".into(),
            },
        );
        let before = serde_json::to_value(&data).unwrap();
        let result = query(&data, "tw", Some(conversation(1)));
        let mission = result.mission.unwrap();
        assert_eq!(mission.id, id);
        assert_eq!(mission.goal.as_deref(), Some("Add CSV export"));
        assert_eq!(mission.projects[0].path, "/worktree");
        assert!(mission.conversations.is_empty());
        assert_eq!(before, serde_json::to_value(&data).unwrap());
    }

    #[test]
    fn new_session_does_not_inherit_the_previous_conversations_assignment() {
        let mut data = fixture();
        let old = conversation(1);
        let session = record(&mut data, &old);
        data.projects[0].agent_sessions.insert("t".into(), session);
        create(
            &mut data,
            MissionMember::Conversation {
                conversation: old.clone(),
            },
        );
        assert!(query(&data, "t", Some(old)).mission.is_some());
        assert!(query(&data, "t", Some(conversation(2))).mission.is_none());
        assert!(query(&data, "t", None).mission.is_some());
    }

    #[test]
    fn resumed_assignment_wins_over_worktree_default_and_respects_detach() {
        let mut data = fixture();
        let current = conversation(1);
        record(&mut data, &current);
        let original = create(
            &mut data,
            MissionMember::Conversation {
                conversation: current.clone(),
            },
        );
        let default = create(
            &mut data,
            MissionMember::Worktree {
                project_id: "w".into(),
            },
        );
        assert_eq!(
            query(&data, "tw", Some(current.clone()))
                .mission
                .unwrap()
                .id,
            original
        );
        assert_eq!(
            query(&data, "tw", Some(conversation(2)))
                .mission
                .unwrap()
                .id,
            default
        );
        apply_command(
            &mut data,
            MissionCommand::Detach {
                mission_id: original,
                member: MissionMember::Conversation {
                    conversation: current.clone(),
                },
            },
        )
        .unwrap();
        assert!(query(&data, "tw", Some(current)).mission.is_none());
        apply_command(
            &mut data,
            MissionCommand::Detach {
                mission_id: default,
                member: MissionMember::Terminal {
                    project_id: "w".into(),
                    terminal_id: "tw".into(),
                },
            },
        )
        .unwrap();
        assert!(query(&data, "tw", Some(conversation(2))).mission.is_none());
    }

    #[test]
    fn context_contains_other_checkouts_and_retained_history_without_transcripts() {
        let mut data = fixture();
        let id = create(
            &mut data,
            MissionMember::Worktree {
                project_id: "w".into(),
            },
        );
        apply_command(
            &mut data,
            MissionCommand::Attach {
                mission_id: id.clone(),
                member: MissionMember::Terminal {
                    project_id: "p".into(),
                    terminal_id: "t".into(),
                },
            },
        )
        .unwrap();
        let other = ConversationId {
            agent: "codex".into(),
            session_id: conversation(1).session_id,
        };
        record(&mut data, &other);
        apply_command(
            &mut data,
            MissionCommand::Attach {
                mission_id: id,
                member: MissionMember::Conversation {
                    conversation: other,
                },
            },
        )
        .unwrap();
        let result = query(&data, "tw", Some(conversation(2)));
        let mission = result.mission.as_ref().unwrap();
        assert_eq!(
            mission
                .projects
                .iter()
                .map(|p| p.id.as_str())
                .collect::<Vec<_>>(),
            ["w", "p"]
        );
        assert_eq!(mission.conversations.len(), 1);
        assert!(mission.conversations[0].attachments.is_empty());
        assert!(!serde_json::to_string(&result).unwrap().contains("/private"));
    }

    #[test]
    fn context_uses_observed_branch_not_deprecated_worktree_metadata() {
        let mut data = fixture();
        create(
            &mut data,
            MissionMember::Worktree {
                project_id: "w".into(),
            },
        );
        let git = ApiGitStatus {
            branch: Some("feat/export".into()),
            ..Default::default()
        };
        let result = mission_context(
            &data,
            &MissionContextRequest {
                terminal_id: "tw".into(),
                conversation: None,
            },
            &HashMap::from([("w".into(), git)]),
        )
        .unwrap();
        assert_eq!(
            result.current_project.branch.as_deref(),
            Some("feat/export")
        );
    }

    #[test]
    fn invalid_identity_and_foreign_or_missing_terminals_are_rejected() {
        let mut data = fixture();
        for request in [
            MissionContextRequest {
                terminal_id: "missing".into(),
                conversation: None,
            },
            MissionContextRequest {
                terminal_id: "t".into(),
                conversation: Some(ConversationId {
                    agent: "claude-code".into(),
                    session_id: "../bad".into(),
                }),
            },
        ] {
            assert!(mission_context(&data, &request, &HashMap::new()).is_err());
        }
        data.projects[0].connection_id = Some("other".into());
        assert!(
            mission_context(
                &data,
                &MissionContextRequest {
                    terminal_id: "t".into(),
                    conversation: None
                },
                &HashMap::new()
            )
            .is_err()
        );
    }

    #[test]
    fn large_missions_keep_the_current_checkout_and_report_omissions() {
        let mut data = fixture();
        let id = create(
            &mut data,
            MissionMember::Worktree {
                project_id: "w".into(),
            },
        );
        for n in 0..20 {
            let mut project = data.projects[0].clone();
            project.id = format!("extra-{n}");
            project.layout = None;
            data.projects.push(project.clone());
            apply_command(
                &mut data,
                MissionCommand::Attach {
                    mission_id: id.clone(),
                    member: MissionMember::Repository {
                        project_id: project.id,
                    },
                },
            )
            .unwrap();
            let current = conversation(n);
            record(&mut data, &current);
            apply_command(
                &mut data,
                MissionCommand::Attach {
                    mission_id: id.clone(),
                    member: MissionMember::Conversation {
                        conversation: current,
                    },
                },
            )
            .unwrap();
        }
        let result = query(&data, "tw", Some(conversation(99))).mission.unwrap();
        assert_eq!(result.projects.len(), MAX_CONTEXT_MEMBERS);
        assert_eq!(result.projects[0].id, "w");
        assert_eq!(result.omitted_projects, 9);
        assert_eq!(result.conversations.len(), MAX_CONTEXT_MEMBERS);
        assert_eq!(result.omitted_conversations, 8);
    }
}
