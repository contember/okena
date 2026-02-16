use crate::keys::SpecialKey;
use crate::theme::FolderColor;
use crate::types::SplitDirection;
use serde::{Deserialize, Serialize};

// ── API request/response types ──────────────────────────────────────────────

/// GET /health response
#[derive(Serialize, Deserialize)]
pub struct HealthResponse {
    pub status: String,
    pub version: String,
    pub uptime_secs: u64,
}

/// GET /v1/state response
#[derive(Clone, Serialize, Deserialize)]
pub struct StateResponse {
    pub state_version: u64,
    pub projects: Vec<ApiProject>,
    pub focused_project_id: Option<String>,
    pub fullscreen_terminal: Option<ApiFullscreen>,
    #[serde(default)]
    pub folders: Vec<ApiFolder>,
    #[serde(default)]
    pub project_order: Vec<String>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ApiProject {
    pub id: String,
    pub name: String,
    pub path: String,
    pub is_visible: bool,
    pub layout: Option<ApiLayoutNode>,
    pub terminal_names: std::collections::HashMap<String, String>,
    #[serde(default)]
    pub folder_color: FolderColor,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ApiFolder {
    pub id: String,
    pub name: String,
    pub project_ids: Vec<String>,
    #[serde(default)]
    pub folder_color: FolderColor,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ApiLayoutNode {
    Terminal {
        terminal_id: Option<String>,
        minimized: bool,
        detached: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        cols: Option<u16>,
        #[serde(skip_serializing_if = "Option::is_none")]
        rows: Option<u16>,
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
#[derive(Debug, Serialize, Deserialize)]
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
    CreateTerminal {
        project_id: String,
    },
    UpdateSplitSizes {
        project_id: String,
        path: Vec<usize>,
        sizes: Vec<f32>,
    },
}

/// POST /v1/pair request
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairRequest {
    pub code: String,
}

/// POST /v1/pair response
#[derive(Serialize, Deserialize)]
pub struct PairResponse {
    pub token: String,
    pub expires_in: u64,
}

/// Generic error response
#[derive(Serialize, Deserialize)]
pub struct ErrorResponse {
    pub error: String,
}

// ── Helper methods ──────────────────────────────────────────────────────────

impl ApiLayoutNode {
    /// Collect all terminal IDs from the layout tree
    pub fn collect_terminal_ids(&self) -> Vec<String> {
        let mut ids = Vec::new();
        self.collect_terminal_ids_into(&mut ids);
        ids
    }

    fn collect_terminal_ids_into(&self, ids: &mut Vec<String>) {
        match self {
            ApiLayoutNode::Terminal { terminal_id, .. } => {
                if let Some(id) = terminal_id {
                    ids.push(id.clone());
                }
            }
            ApiLayoutNode::Split { children, .. } | ApiLayoutNode::Tabs { children, .. } => {
                for child in children {
                    child.collect_terminal_ids_into(ids);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_response_round_trip() {
        let resp = StateResponse {
            state_version: 42,
            projects: vec![ApiProject {
                id: "p1".into(),
                name: "Test".into(),
                path: "/tmp".into(),
                is_visible: true,
                layout: Some(ApiLayoutNode::Split {
                    direction: SplitDirection::Horizontal,
                    sizes: vec![50.0, 50.0],
                    children: vec![
                        ApiLayoutNode::Terminal {
                            terminal_id: Some("t1".into()),
                            minimized: false,
                            detached: false,
                            cols: Some(120),
                            rows: Some(40),
                        },
                        ApiLayoutNode::Tabs {
                            active_tab: 0,
                            children: vec![ApiLayoutNode::Terminal {
                                terminal_id: Some("t2".into()),
                                minimized: true,
                                detached: true,
                                cols: None,
                                rows: None,
                            }],
                        },
                    ],
                }),
                terminal_names: [("t1".into(), "bash".into())].into_iter().collect(),
                folder_color: FolderColor::Blue,
            }],
            focused_project_id: Some("p1".into()),
            fullscreen_terminal: None,
            folders: vec![ApiFolder {
                id: "f1".into(),
                name: "Backend".into(),
                project_ids: vec!["p1".into()],
                folder_color: FolderColor::Green,
            }],
            project_order: vec!["f1".into()],
        };
        let json = serde_json::to_string(&resp).unwrap();
        let parsed: StateResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.state_version, 42);
        assert_eq!(parsed.projects.len(), 1);
        assert_eq!(parsed.projects[0].id, "p1");
        assert_eq!(parsed.projects[0].folder_color, FolderColor::Blue);
        assert!(parsed.fullscreen_terminal.is_none());
        assert_eq!(parsed.folders.len(), 1);
        assert_eq!(parsed.folders[0].name, "Backend");
        assert_eq!(parsed.folders[0].folder_color, FolderColor::Green);
        assert_eq!(parsed.project_order, vec!["f1"]);
    }

    #[test]
    fn state_response_backwards_compatible() {
        // Old JSON without folders/project_order/folder_color should deserialize with defaults
        let json = r#"{"state_version":1,"projects":[{"id":"p1","name":"Test","path":"/tmp","is_visible":true,"layout":null,"terminal_names":{}}],"focused_project_id":null,"fullscreen_terminal":null}"#;
        let parsed: StateResponse = serde_json::from_str(json).unwrap();
        assert!(parsed.folders.is_empty());
        assert!(parsed.project_order.is_empty());
        assert_eq!(parsed.projects[0].folder_color, FolderColor::Default);
    }

    #[test]
    fn action_request_round_trip() {
        let actions = vec![
            ActionRequest::SendText {
                terminal_id: "t1".into(),
                text: "hello".into(),
            },
            ActionRequest::RunCommand {
                terminal_id: "t1".into(),
                command: "ls".into(),
            },
            ActionRequest::SendSpecialKey {
                terminal_id: "t1".into(),
                key: SpecialKey::Enter,
            },
            ActionRequest::SplitTerminal {
                project_id: "p1".into(),
                path: vec![0, 1],
                direction: SplitDirection::Vertical,
            },
            ActionRequest::CloseTerminal {
                project_id: "p1".into(),
                terminal_id: "t1".into(),
            },
            ActionRequest::FocusTerminal {
                project_id: "p1".into(),
                terminal_id: "t1".into(),
            },
            ActionRequest::ReadContent {
                terminal_id: "t1".into(),
            },
            ActionRequest::Resize {
                terminal_id: "t1".into(),
                cols: 80,
                rows: 24,
            },
            ActionRequest::CreateTerminal {
                project_id: "p1".into(),
            },
            ActionRequest::UpdateSplitSizes {
                project_id: "p1".into(),
                path: vec![0],
                sizes: vec![60.0, 40.0],
            },
        ];
        for action in actions {
            let json = serde_json::to_string(&action).unwrap();
            let _parsed: ActionRequest = serde_json::from_str(&json).unwrap();
        }
    }

    #[test]
    fn api_layout_node_collect_terminal_ids() {
        let layout = ApiLayoutNode::Split {
            direction: SplitDirection::Horizontal,
            sizes: vec![50.0, 50.0],
            children: vec![
                ApiLayoutNode::Terminal {
                    terminal_id: Some("t1".into()),
                    minimized: false,
                    detached: false,
                    cols: Some(80),
                    rows: Some(24),
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
                            terminal_id: None,
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
        let ids = layout.collect_terminal_ids();
        assert_eq!(ids, vec!["t1", "t2", "t3"]);
    }
}
