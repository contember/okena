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

/// Parse a binary PTY output frame.
/// Format: [proto_version=1][frame_type=1][stream_id:u32 BE][pty_data...]
pub fn parse_pty_frame(data: &[u8]) -> Option<(u32, &[u8])> {
    if data.len() < 6 || data[0] != 1 || data[1] != 1 {
        return None;
    }
    let stream_id = u32::from_be_bytes([data[2], data[3], data[4], data[5]]);
    Some((stream_id, &data[6..]))
}

/// Build a binary PTY output frame.
pub fn build_pty_frame(stream_id: u32, data: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(6 + data.len());
    frame.push(1u8); // proto_version
    frame.push(1u8); // frame_type: pty
    frame.extend_from_slice(&stream_id.to_be_bytes());
    frame.extend_from_slice(data);
    frame
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
            },
            WsOutbound::StateChanged { state_version: 5 },
            WsOutbound::Dropped { count: 3 },
            WsOutbound::Pong,
            WsOutbound::Error {
                error: "oops".into(),
            },
        ];
        for msg in messages {
            let json = serde_json::to_string(&msg).unwrap();
            let _parsed: WsOutbound = serde_json::from_str(&json).unwrap();
        }
    }

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
    fn parse_pty_frame_wrong_frame_type() {
        let mut data = build_pty_frame(1, b"x");
        data[1] = 2; // wrong frame type
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
