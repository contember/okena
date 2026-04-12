#[allow(unused_imports)]
pub use okena_core::api::{
    ActionRequest, ApiFolder, ApiFullscreen, ApiGitStatus, ApiLayoutNode, ApiProject,
    ApiServiceInfo, ErrorResponse, HealthResponse, PairRequest, PairResponse, StateResponse,
};
#[allow(unused_imports)]
pub use okena_core::ws::{
    WsInbound, WsOutbound, build_binary_frame, build_pty_frame, parse_binary_frame,
    parse_pty_frame, FRAME_TYPE_INPUT, FRAME_TYPE_PTY, FRAME_TYPE_SNAPSHOT, PROTO_VERSION,
    PROTO_VERSION_2, build_input_frame_v2, parse_input_frame_v2, build_pty_frame_v2,
    parse_pty_frame_v2, parse_binary_frame_any,
};

// LayoutNode conversion helpers (from_api, from_api_prefixed, to_api) are now
// defined in the okena-workspace crate (state.rs impl LayoutNode).

#[cfg(test)]
mod tests {
    use crate::workspace::state::LayoutNode;
    use okena_core::api::ApiLayoutNode;
    use okena_core::types::SplitDirection;

    #[test]
    fn prefixed_terminal_id() {
        let api = ApiLayoutNode::Terminal {
            terminal_id: Some("abc-123".into()),
            minimized: false,
            detached: false,
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
                },
                ApiLayoutNode::Tabs {
                    active_tab: 0,
                    children: vec![
                        ApiLayoutNode::Terminal {
                            terminal_id: Some("t2".into()),
                            minimized: false,
                            detached: false,
                        },
                        ApiLayoutNode::Terminal {
                            terminal_id: Some("t3".into()),
                            minimized: false,
                            detached: true,
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
