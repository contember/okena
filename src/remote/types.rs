#[allow(unused_imports)]
pub use okena_core::api::{
    ActionRequest, ApiFolder, ApiFullscreen, ApiLayoutNode, ApiProject, ErrorResponse,
    HealthResponse, PairRequest, PairResponse, StateResponse,
};
#[allow(unused_imports)]
pub use okena_core::ws::{
    WsInbound, WsOutbound, build_binary_frame, build_pty_frame, parse_binary_frame,
    parse_pty_frame, FRAME_TYPE_INPUT, FRAME_TYPE_PTY, FRAME_TYPE_SNAPSHOT, PROTO_VERSION,
};

use crate::workspace::state::LayoutNode;
use std::collections::HashMap;

// ── Conversion helpers ──────────────────────────────────────────────────────

impl LayoutNode {
    #[allow(dead_code)]
    pub fn from_api(api: &ApiLayoutNode) -> Self {
        match api {
            ApiLayoutNode::Terminal {
                terminal_id,
                minimized,
                detached,
                ..
            } => LayoutNode::Terminal {
                terminal_id: terminal_id.clone(),
                minimized: *minimized,
                detached: *detached,
                shell_type: Default::default(),
                zoom_level: 1.0,
            },
            ApiLayoutNode::Split {
                direction,
                sizes,
                children,
            } => LayoutNode::Split {
                direction: *direction,
                sizes: sizes.clone(),
                children: children.iter().map(LayoutNode::from_api).collect(),
            },
            ApiLayoutNode::Tabs {
                children,
                active_tab,
            } => LayoutNode::Tabs {
                children: children.iter().map(LayoutNode::from_api).collect(),
                active_tab: *active_tab,
            },
        }
    }

    /// Convert from API, prefixing all terminal IDs with the given prefix.
    /// Used for remote projects where terminals are registered with prefixed IDs.
    pub fn from_api_prefixed(api: &ApiLayoutNode, prefix: &str) -> Self {
        match api {
            ApiLayoutNode::Terminal {
                terminal_id,
                minimized,
                detached,
                ..
            } => LayoutNode::Terminal {
                terminal_id: terminal_id.as_ref().map(|id| format!("{}:{}", prefix, id)),
                minimized: *minimized,
                detached: *detached,
                shell_type: Default::default(),
                zoom_level: 1.0,
            },
            ApiLayoutNode::Split {
                direction,
                sizes,
                children,
            } => LayoutNode::Split {
                direction: *direction,
                sizes: sizes.clone(),
                children: children
                    .iter()
                    .map(|c| LayoutNode::from_api_prefixed(c, prefix))
                    .collect(),
            },
            ApiLayoutNode::Tabs {
                children,
                active_tab,
            } => LayoutNode::Tabs {
                children: children
                    .iter()
                    .map(|c| LayoutNode::from_api_prefixed(c, prefix))
                    .collect(),
                active_tab: *active_tab,
            },
        }
    }

    pub fn to_api(&self) -> ApiLayoutNode {
        self.to_api_with_sizes(&HashMap::new())
    }

    /// Convert to API, populating terminal `cols`/`rows` from the given size map.
    pub fn to_api_with_sizes(
        &self,
        sizes: &HashMap<String, (u16, u16)>,
    ) -> ApiLayoutNode {
        match self {
            LayoutNode::Terminal {
                terminal_id,
                minimized,
                detached,
                ..
            } => {
                let (cols, rows) = terminal_id
                    .as_ref()
                    .and_then(|id| sizes.get(id))
                    .map(|&(c, r)| (Some(c), Some(r)))
                    .unwrap_or((None, None));
                ApiLayoutNode::Terminal {
                    terminal_id: terminal_id.clone(),
                    minimized: *minimized,
                    detached: *detached,
                    cols,
                    rows,
                }
            }
            LayoutNode::Split {
                direction,
                sizes: split_sizes,
                children,
            } => ApiLayoutNode::Split {
                direction: *direction,
                sizes: split_sizes.clone(),
                children: children
                    .iter()
                    .map(|c| c.to_api_with_sizes(sizes))
                    .collect(),
            },
            LayoutNode::Tabs {
                children,
                active_tab,
            } => ApiLayoutNode::Tabs {
                children: children
                    .iter()
                    .map(|c| c.to_api_with_sizes(sizes))
                    .collect(),
                active_tab: *active_tab,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use okena_core::types::SplitDirection;

    #[test]
    fn prefixed_terminal_id() {
        let api = ApiLayoutNode::Terminal {
            terminal_id: Some("abc-123".into()),
            minimized: false,
            detached: false,
            cols: None,
            rows: None,
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
            cols: None,
            rows: None,
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
                    cols: None,
                    rows: None,
                },
                ApiLayoutNode::Tabs {
                    active_tab: 0,
                    children: vec![
                        ApiLayoutNode::Terminal {
                            terminal_id: Some("t2".into()),
                            minimized: false,
                            detached: false,
                            cols: None,
                            rows: None,
                        },
                        ApiLayoutNode::Terminal {
                            terminal_id: Some("t3".into()),
                            minimized: false,
                            detached: true,
                            cols: None,
                            rows: None,
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
            cols: None,
            rows: None,
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
