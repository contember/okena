#[allow(unused_imports)]
pub use okena_core::api::{
    ActionRequest, ApiFolder, ApiFullscreen, ApiGitStatus, ApiLayoutNode, ApiProject,
    ApiServiceInfo, ApiSystemStats, ApiWindow, ApiWindowBounds, ErrorResponse, HealthResponse,
    PairRequest, PairResponse, StateResponse,
};
#[allow(unused_imports)]
pub use okena_core::ws::{
    FRAME_TYPE_INPUT, FRAME_TYPE_PTY, FRAME_TYPE_SNAPSHOT, PROTO_VERSION, WsInbound, WsOutbound,
    build_binary_frame, build_pty_frame, parse_binary_frame, parse_pty_frame,
};

// LayoutNode conversion helpers (from_api, from_api_prefixed, to_api) are now
// defined in the okena-workspace crate (state.rs impl LayoutNode).

#[cfg(test)]
mod tests {
    use okena_core::api::ApiLayoutNode;
    use okena_core::types::SplitDirection;
    use okena_workspace::state::LayoutNode;

    #[test]
    fn work_overview_preserves_old_snapshot_compatibility() {
        use okena_core::api::StateResponse;
        use okena_core::mission::WorkOverview;

        let old_json = serde_json::json!({
            "state_version": 1, "projects": [],
            "focused_project_id": null, "fullscreen_terminal": null
        });
        let mut current: StateResponse = serde_json::from_value(old_json).expect("old snapshot");
        assert!(current.work_overview.is_none());
        current.work_overview = Some(WorkOverview::default());

        // The pre-overview schema accepts additional top-level fields.
        #[derive(serde::Deserialize)]
        struct LegacyState {
            state_version: u64,
            projects: Vec<okena_core::api::ApiProject>,
            focused_project_id: Option<String>,
            fullscreen_terminal: Option<okena_core::api::ApiFullscreen>,
            project_order: Vec<String>,
            folders: Vec<okena_core::api::ApiFolder>,
            windows: Vec<okena_core::api::ApiWindow>,
            hooks: Vec<okena_core::api::ApiHookExecution>,
        }
        let json = serde_json::to_value(&current).expect("snapshot");
        let legacy: LegacyState = serde_json::from_value(json.clone()).expect("old client");
        assert_eq!(legacy.state_version, 1);
        assert!(legacy.projects.is_empty());
        assert!(legacy.focused_project_id.is_none());
        assert!(legacy.fullscreen_terminal.is_none());
        assert!(legacy.project_order.is_empty());
        assert!(legacy.folders.is_empty());
        assert!(legacy.windows.is_empty());
        assert!(legacy.hooks.is_empty());
        let decoded: StateResponse = serde_json::from_value(json).expect("new client");
        assert_eq!(decoded.work_overview, Some(WorkOverview::default()));
    }

    #[test]
    fn conversation_overview_omits_private_transcript_path() {
        use okena_core::agent_session::AgentSession;
        use okena_core::attention::ConversationId;
        use okena_core::mission::{ConversationAttachment, WorkOverview};

        let session = AgentSession {
            agent: "codex".into(),
            session_id: "11111111-2222-3333-4444-555555555555".into(),
            transcript_path: Some("/private/transcripts/session.jsonl".into()),
        };
        let overview = WorkOverview {
            conversations: vec![ConversationAttachment {
                project_id: "p".into(),
                terminal_id: "t".into(),
                conversation: ConversationId::from(&session),
            }],
            ..WorkOverview::default()
        };
        let json = serde_json::to_value(overview).expect("public overview");
        assert_eq!(
            json["conversations"][0]["conversation"],
            serde_json::json!({
                "agent": "codex",
                "session_id": "11111111-2222-3333-4444-555555555555"
            })
        );
        let serialized = json.to_string();
        assert!(!serialized.contains("transcript_path"));
        assert!(!serialized.contains("/private/transcripts"));
    }

    #[test]
    fn prefixed_terminal_id() {
        let api = ApiLayoutNode::Terminal {
            terminal_id: Some("abc-123".into()),
            minimized: false,
            detached: false,
            shell_type: Default::default(),
            cols: None,
            rows: None,
            show_name_when_inactive: false,
        };
        let node = LayoutNode::from_api_prefixed(&api, "remote:conn1");
        match node {
            LayoutNode::Terminal { terminal_id, .. } => {
                assert_eq!(terminal_id.unwrap(), "remote:conn1:abc-123");
            }
            _ => panic!("expected Terminal"),
        }
    }

    #[test]
    fn prefixed_none_terminal_id_stays_none() {
        let api = ApiLayoutNode::Terminal {
            terminal_id: None,
            minimized: true,
            detached: false,
            shell_type: Default::default(),
            cols: None,
            rows: None,
            show_name_when_inactive: false,
        };
        let node = LayoutNode::from_api_prefixed(&api, "remote:x");
        match node {
            LayoutNode::Terminal {
                terminal_id,
                minimized,
                ..
            } => {
                assert!(terminal_id.is_none());
                assert!(minimized);
            }
            _ => panic!("expected Terminal"),
        }
    }

    #[test]
    fn prefixed_nested_split_prefixes_all_children() {
        let api = ApiLayoutNode::Split {
            direction: SplitDirection::Horizontal,
            sizes: vec![50.0, 50.0],
            children: vec![
                ApiLayoutNode::Terminal {
                    terminal_id: Some("t1".into()),
                    minimized: false,
                    detached: false,
                    shell_type: Default::default(),
                    cols: None,
                    rows: None,
                    show_name_when_inactive: false,
                },
                ApiLayoutNode::Tabs {
                    active_tab: 0,
                    children: vec![
                        ApiLayoutNode::Terminal {
                            terminal_id: Some("t2".into()),
                            minimized: false,
                            detached: false,
                            shell_type: Default::default(),
                            cols: None,
                            rows: None,
                            show_name_when_inactive: false,
                        },
                        ApiLayoutNode::Terminal {
                            terminal_id: Some("t3".into()),
                            minimized: false,
                            detached: true,
                            shell_type: Default::default(),
                            cols: None,
                            rows: None,
                            show_name_when_inactive: false,
                        },
                    ],
                },
            ],
        };
        let node = LayoutNode::from_api_prefixed(&api, "remote:c1");
        let ids = node.collect_terminal_ids();
        assert_eq!(ids, vec!["remote:c1:t1", "remote:c1:t2", "remote:c1:t3"]);
    }

    #[test]
    fn unprefixed_preserves_raw_ids() {
        let api = ApiLayoutNode::Terminal {
            terminal_id: Some("raw-id".into()),
            minimized: false,
            detached: false,
            shell_type: Default::default(),
            cols: None,
            rows: None,
            show_name_when_inactive: false,
        };
        let node = LayoutNode::from_api(&api);
        match node {
            LayoutNode::Terminal { terminal_id, .. } => {
                assert_eq!(terminal_id.unwrap(), "raw-id");
            }
            _ => panic!("expected Terminal"),
        }
    }
}
