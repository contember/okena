use crate::remote::bridge::SpecialKey;
use crate::workspace::state::SplitDirection;
use serde::{Deserialize, Serialize};

// ── API request/response types ──────────────────────────────────────────────

/// GET /health response
#[derive(Serialize)]
pub struct HealthResponse {
    pub status: &'static str,
    pub version: &'static str,
    pub uptime_secs: u64,
}

/// GET /v1/state response
#[derive(Clone, Serialize, Deserialize)]
pub struct StateResponse {
    pub state_version: u64,
    pub projects: Vec<ApiProject>,
    pub focused_project_id: Option<String>,
    pub fullscreen_terminal: Option<ApiFullscreen>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ApiProject {
    pub id: String,
    pub name: String,
    pub path: String,
    pub is_visible: bool,
    pub layout: Option<ApiLayoutNode>,
    pub terminal_names: std::collections::HashMap<String, String>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ApiLayoutNode {
    Terminal {
        terminal_id: Option<String>,
        minimized: bool,
        detached: bool,
    },
    Split {
        direction: SplitDirection,
        sizes: Vec<f32>,
        children: Vec<ApiLayoutNode>,
    },
    Tabs {
        children: Vec<ApiLayoutNode>,
        active_tab: usize,
    },
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ApiFullscreen {
    pub project_id: String,
    pub terminal_id: String,
}

/// POST /v1/actions request body (tagged enum)
#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum ActionRequest {
    SendText {
        terminal_id: String,
        text: String,
    },
    RunCommand {
        terminal_id: String,
        command: String,
    },
    SendSpecialKey {
        terminal_id: String,
        key: SpecialKey,
    },
    SplitTerminal {
        project_id: String,
        path: Vec<usize>,
        direction: SplitDirection,
    },
    CloseTerminal {
        project_id: String,
        terminal_id: String,
    },
    FocusTerminal {
        project_id: String,
        terminal_id: String,
    },
    ReadContent {
        terminal_id: String,
    },
    Resize {
        terminal_id: String,
        cols: u16,
        rows: u16,
    },
}

/// POST /v1/pair request
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairRequest {
    pub code: String,
}

/// POST /v1/pair response
#[derive(Serialize)]
pub struct PairResponse {
    pub token: String,
    pub expires_in: u64,
}

/// Generic error response
#[derive(Serialize)]
#[allow(dead_code)]
pub struct ErrorResponse {
    pub error: String,
}

// ── WebSocket message types ─────────────────────────────────────────────────

/// Inbound WebSocket messages (from client)
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum WsInbound {
    Auth {
        token: String,
    },
    Subscribe {
        terminal_ids: Vec<String>,
    },
    Unsubscribe {
        terminal_ids: Vec<String>,
    },
    SendText {
        terminal_id: String,
        text: String,
    },
    SendSpecialKey {
        terminal_id: String,
        key: SpecialKey,
    },
    Resize {
        terminal_id: String,
        cols: u16,
        rows: u16,
    },
    Ping,
}

/// Outbound WebSocket JSON messages (to client)
#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WsOutbound {
    AuthOk,
    AuthFailed {
        error: String,
    },
    Subscribed {
        mappings: std::collections::HashMap<String, u32>,
    },
    StateChanged {
        state_version: u64,
    },
    Dropped {
        count: u64,
    },
    Pong,
    Error {
        error: String,
    },
}

// ── Conversion helpers ──────────────────────────────────────────────────────

impl ApiLayoutNode {
    pub fn to_layout_node(&self) -> crate::workspace::state::LayoutNode {
        match self {
            ApiLayoutNode::Terminal { terminal_id, minimized, detached } => {
                crate::workspace::state::LayoutNode::Terminal {
                    terminal_id: terminal_id.clone(),
                    minimized: *minimized,
                    detached: *detached,
                    shell_type: Default::default(),
                    zoom_level: 1.0,
                }
            }
            ApiLayoutNode::Split { direction, sizes, children } => {
                crate::workspace::state::LayoutNode::Split {
                    direction: *direction,
                    sizes: sizes.clone(),
                    children: children.iter().map(|c| c.to_layout_node()).collect(),
                }
            }
            ApiLayoutNode::Tabs { children, active_tab } => {
                crate::workspace::state::LayoutNode::Tabs {
                    children: children.iter().map(|c| c.to_layout_node()).collect(),
                    active_tab: *active_tab,
                }
            }
        }
    }

    /// Convert to LayoutNode, prefixing all terminal IDs with the given prefix.
    /// Used for remote projects where terminals are registered with prefixed IDs.
    pub fn to_layout_node_prefixed(&self, prefix: &str) -> crate::workspace::state::LayoutNode {
        match self {
            ApiLayoutNode::Terminal { terminal_id, minimized, detached } => {
                crate::workspace::state::LayoutNode::Terminal {
                    terminal_id: terminal_id.as_ref().map(|id| format!("{}:{}", prefix, id)),
                    minimized: *minimized,
                    detached: *detached,
                    shell_type: Default::default(),
                    zoom_level: 1.0,
                }
            }
            ApiLayoutNode::Split { direction, sizes, children } => {
                crate::workspace::state::LayoutNode::Split {
                    direction: *direction,
                    sizes: sizes.clone(),
                    children: children.iter().map(|c| c.to_layout_node_prefixed(prefix)).collect(),
                }
            }
            ApiLayoutNode::Tabs { children, active_tab } => {
                crate::workspace::state::LayoutNode::Tabs {
                    children: children.iter().map(|c| c.to_layout_node_prefixed(prefix)).collect(),
                    active_tab: *active_tab,
                }
            }
        }
    }

    pub fn from_layout(node: &crate::workspace::state::LayoutNode) -> Self {
        match node {
            crate::workspace::state::LayoutNode::Terminal {
                terminal_id,
                minimized,
                detached,
                ..
            } => ApiLayoutNode::Terminal {
                terminal_id: terminal_id.clone(),
                minimized: *minimized,
                detached: *detached,
            },
            crate::workspace::state::LayoutNode::Split {
                direction,
                sizes,
                children,
            } => ApiLayoutNode::Split {
                direction: *direction,
                sizes: sizes.clone(),
                children: children.iter().map(ApiLayoutNode::from_layout).collect(),
            },
            crate::workspace::state::LayoutNode::Tabs {
                children,
                active_tab,
            } => ApiLayoutNode::Tabs {
                children: children.iter().map(ApiLayoutNode::from_layout).collect(),
                active_tab: *active_tab,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::state::LayoutNode;

    #[test]
    fn prefixed_terminal_id() {
        let api = ApiLayoutNode::Terminal {
            terminal_id: Some("abc-123".into()),
            minimized: false,
            detached: false,
        };
        let node = api.to_layout_node_prefixed("remote:conn1");
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
        let node = api.to_layout_node_prefixed("remote:x");
        match node {
            LayoutNode::Terminal { terminal_id, minimized, .. } => {
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
        let node = api.to_layout_node_prefixed("remote:c1");
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
        let node = api.to_layout_node();
        match node {
            LayoutNode::Terminal { terminal_id, .. } => {
                assert_eq!(terminal_id.unwrap(), "raw-id");
            }
            _ => panic!("expected Terminal"),
        }
    }
}
