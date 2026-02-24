use crate::api::ApiGitStatus;
use crate::keys::SpecialKey;
use serde::{Deserialize, Serialize};

/// Inbound WebSocket messages (from client)
#[derive(Serialize, Deserialize)]
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
    SubscribeApps {
        app_ids: Vec<String>,
    },
    UnsubscribeApps {
        app_ids: Vec<String>,
    },
    AppAction {
        app_id: String,
        action: serde_json::Value,
    },
    Ping,
}

/// Outbound WebSocket JSON messages (to client)
#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WsOutbound {
    AuthOk,
    AuthFailed {
        error: String,
    },
    Subscribed {
        mappings: std::collections::HashMap<String, u32>,
        /// Terminal sizes (cols, rows) keyed by terminal_id.
        /// Clients should pre-resize grids to these dimensions before snapshots arrive.
        #[serde(default)]
        sizes: std::collections::HashMap<String, (u16, u16)>,
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
    GitStatusChanged {
        projects: std::collections::HashMap<String, ApiGitStatus>,
    },
    AppStateChanged {
        app_id: String,
        app_kind: String,
        state: serde_json::Value,
    },
}

// ── Binary frame protocol ──────────────────────────────────────────────────

pub const PROTO_VERSION: u8 = 1;
pub const FRAME_TYPE_PTY: u8 = 1; // server → client: live PTY output
pub const FRAME_TYPE_SNAPSHOT: u8 = 2; // server → client: full screen redraw
pub const FRAME_TYPE_INPUT: u8 = 3; // client → server: terminal input

/// Parse a generic binary frame.
/// Format: [proto_version=1][frame_type][stream_id:u32 BE][payload...]
/// Returns (frame_type, stream_id, payload) or None if invalid.
pub fn parse_binary_frame(data: &[u8]) -> Option<(u8, u32, &[u8])> {
    if data.len() < 6 || data[0] != PROTO_VERSION {
        return None;
    }
    let frame_type = data[1];
    let stream_id = u32::from_be_bytes([data[2], data[3], data[4], data[5]]);
    Some((frame_type, stream_id, &data[6..]))
}

/// Build a generic binary frame.
pub fn build_binary_frame(frame_type: u8, stream_id: u32, data: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(6 + data.len());
    frame.push(PROTO_VERSION);
    frame.push(frame_type);
    frame.extend_from_slice(&stream_id.to_be_bytes());
    frame.extend_from_slice(data);
    frame
}

/// Parse a binary PTY output frame (frame_type must be 1).
/// Format: [proto_version=1][frame_type=1][stream_id:u32 BE][pty_data...]
pub fn parse_pty_frame(data: &[u8]) -> Option<(u32, &[u8])> {
    let (frame_type, stream_id, payload) = parse_binary_frame(data)?;
    if frame_type != FRAME_TYPE_PTY {
        return None;
    }
    Some((stream_id, payload))
}

/// Build a binary PTY output frame.
pub fn build_pty_frame(stream_id: u32, data: &[u8]) -> Vec<u8> {
    build_binary_frame(FRAME_TYPE_PTY, stream_id, data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ws_inbound_round_trip() {
        let messages = vec![
            WsInbound::Auth {
                token: "tok123".into(),
            },
            WsInbound::Subscribe {
                terminal_ids: vec!["t1".into(), "t2".into()],
            },
            WsInbound::Unsubscribe {
                terminal_ids: vec!["t1".into()],
            },
            WsInbound::SendText {
                terminal_id: "t1".into(),
                text: "hello".into(),
            },
            WsInbound::SendSpecialKey {
                terminal_id: "t1".into(),
                key: SpecialKey::Enter,
            },
            WsInbound::Resize {
                terminal_id: "t1".into(),
                cols: 80,
                rows: 24,
            },
            WsInbound::SubscribeApps {
                app_ids: vec!["app-1".into(), "app-2".into()],
            },
            WsInbound::UnsubscribeApps {
                app_ids: vec!["app-1".into()],
            },
            WsInbound::AppAction {
                app_id: "app-1".into(),
                action: serde_json::json!({"kind": "click"}),
            },
            WsInbound::Ping,
        ];
        for msg in messages {
            let json = serde_json::to_string(&msg).unwrap();
            let _parsed: WsInbound = serde_json::from_str(&json).unwrap();
        }
    }

    #[test]
    fn ws_outbound_round_trip() {
        let messages = vec![
            WsOutbound::AuthOk,
            WsOutbound::AuthFailed {
                error: "bad token".into(),
            },
            WsOutbound::Subscribed {
                mappings: [("t1".into(), 1)].into_iter().collect(),
                sizes: [("t1".into(), (120, 40))].into_iter().collect(),
            },
            WsOutbound::StateChanged { state_version: 5 },
            WsOutbound::Dropped { count: 3 },
            WsOutbound::Pong,
            WsOutbound::Error {
                error: "oops".into(),
            },
            WsOutbound::GitStatusChanged {
                projects: [(
                    "p1".into(),
                    ApiGitStatus {
                        branch: Some("main".into()),
                        lines_added: 10,
                        lines_removed: 3,
                    },
                )]
                .into_iter()
                .collect(),
            },
            WsOutbound::AppStateChanged {
                app_id: "app-1".into(),
                app_kind: "kruh".into(),
                state: serde_json::json!({"step": 3}),
            },
        ];
        for msg in messages {
            let json = serde_json::to_string(&msg).unwrap();
            let _parsed: WsOutbound = serde_json::from_str(&json).unwrap();
        }
    }

    // ── Generic binary frame tests ─────────────────────────────────────

    #[test]
    fn binary_frame_round_trip_all_types() {
        for frame_type in [FRAME_TYPE_PTY, FRAME_TYPE_SNAPSHOT, FRAME_TYPE_INPUT] {
            let data = b"hello world";
            let frame = build_binary_frame(frame_type, 42, data);
            let (ft, sid, payload) = parse_binary_frame(&frame).unwrap();
            assert_eq!(ft, frame_type);
            assert_eq!(sid, 42);
            assert_eq!(payload, data);
        }
    }

    #[test]
    fn binary_frame_empty_payload() {
        let frame = build_binary_frame(FRAME_TYPE_PTY, 1, b"");
        let (ft, sid, payload) = parse_binary_frame(&frame).unwrap();
        assert_eq!(ft, FRAME_TYPE_PTY);
        assert_eq!(sid, 1);
        assert!(payload.is_empty());
    }

    #[test]
    fn binary_frame_too_short() {
        assert!(parse_binary_frame(&[1, 1, 0, 0, 0]).is_none());
        assert!(parse_binary_frame(&[]).is_none());
    }

    #[test]
    fn binary_frame_wrong_proto() {
        let mut frame = build_binary_frame(FRAME_TYPE_PTY, 1, b"x");
        frame[0] = 2; // wrong proto version
        assert!(parse_binary_frame(&frame).is_none());
    }

    #[test]
    fn binary_frame_various_stream_ids() {
        for stream_id in [0, 1, 255, 65535, u32::MAX] {
            let payload = format!("data for {}", stream_id);
            let frame = build_binary_frame(FRAME_TYPE_SNAPSHOT, stream_id, payload.as_bytes());
            let (ft, parsed_id, parsed_data) = parse_binary_frame(&frame).unwrap();
            assert_eq!(ft, FRAME_TYPE_SNAPSHOT);
            assert_eq!(parsed_id, stream_id);
            assert_eq!(parsed_data, payload.as_bytes());
        }
    }

    // ── PTY frame wrapper tests ────────────────────────────────────────

    #[test]
    fn parse_pty_frame_valid() {
        let data = build_pty_frame(42, b"hello");
        let (stream_id, payload) = parse_pty_frame(&data).unwrap();
        assert_eq!(stream_id, 42);
        assert_eq!(payload, b"hello");
    }

    #[test]
    fn parse_pty_frame_too_short() {
        assert!(parse_pty_frame(&[1, 1, 0, 0, 0]).is_none());
        assert!(parse_pty_frame(&[]).is_none());
    }

    #[test]
    fn parse_pty_frame_wrong_proto() {
        let mut data = build_pty_frame(1, b"x");
        data[0] = 2; // wrong proto version
        assert!(parse_pty_frame(&data).is_none());
    }

    #[test]
    fn parse_pty_frame_rejects_snapshot_type() {
        let data = build_binary_frame(FRAME_TYPE_SNAPSHOT, 1, b"x");
        assert!(parse_pty_frame(&data).is_none());
    }

    #[test]
    fn parse_pty_frame_rejects_input_type() {
        let data = build_binary_frame(FRAME_TYPE_INPUT, 1, b"x");
        assert!(parse_pty_frame(&data).is_none());
    }

    #[test]
    fn build_then_parse_pty_frame() {
        for stream_id in [0, 1, 255, 65535, u32::MAX] {
            let payload = format!("data for {}", stream_id);
            let frame = build_pty_frame(stream_id, payload.as_bytes());
            let (parsed_id, parsed_data) = parse_pty_frame(&frame).unwrap();
            assert_eq!(parsed_id, stream_id);
            assert_eq!(parsed_data, payload.as_bytes());
        }
    }
}
