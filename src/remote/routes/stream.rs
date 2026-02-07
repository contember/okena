use crate::remote::bridge::{BridgeMessage, RemoteCommand};
use crate::remote::routes::AppState;
use crate::remote::types::{WsInbound, WsOutbound};
use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Query, State, WebSocketUpgrade};
use axum::response::IntoResponse;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::Ordering;
use tokio::sync::broadcast;

#[derive(serde::Deserialize)]
pub struct WsQuery {
    pub token: Option<String>,
}

pub async fn ws_handler(
    State(state): State<AppState>,
    Query(query): Query<WsQuery>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_ws(socket, state, query.token))
}

async fn handle_ws(mut socket: WebSocket, state: AppState, query_token: Option<String>) {
    // ── Auth phase ──────────────────────────────────────────────────────
    let authenticated = if let Some(token) = query_token {
        state.auth_store.validate_token(&token)
    } else {
        // Wait for first-message auth (2 second timeout)
        match tokio::time::timeout(std::time::Duration::from_secs(2), socket.recv()).await {
            Ok(Some(Ok(Message::Text(text)))) => {
                if let Ok(msg) = serde_json::from_str::<WsInbound>(&text) {
                    match msg {
                        WsInbound::Auth { token } => state.auth_store.validate_token(&token),
                        _ => false,
                    }
                } else {
                    false
                }
            }
            _ => false,
        }
    };

    if !authenticated {
        let msg = serde_json::to_string(&WsOutbound::AuthFailed {
            error: "authentication required".into(),
        })
        .expect("BUG: WsOutbound must serialize");
        let _ = socket.send(Message::Text(msg.into())).await;
        return;
    }

    // Send auth success
    let msg = serde_json::to_string(&WsOutbound::AuthOk).expect("BUG: WsOutbound must serialize");
    if socket.send(Message::Text(msg.into())).await.is_err() {
        return;
    }

    // ── Main loop ───────────────────────────────────────────────────────
    let mut pty_rx = state.broadcaster.subscribe();
    let mut subscribed_ids: HashSet<String> = HashSet::new();
    let mut stream_id_map: HashMap<String, u32> = HashMap::new();
    let mut next_stream_id: u32 = 1;

    // Track state_version for push notifications
    let mut last_version = state.state_version.load(Ordering::Relaxed);

    loop {
        tokio::select! {
            // Incoming messages from client
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        let parsed = serde_json::from_str::<WsInbound>(&text);
                        match parsed {
                            Ok(WsInbound::Subscribe { terminal_ids }) => {
                                for id in &terminal_ids {
                                    if !stream_id_map.contains_key(id) {
                                        stream_id_map.insert(id.clone(), next_stream_id);
                                        next_stream_id += 1;
                                    }
                                    subscribed_ids.insert(id.clone());
                                }
                                let mappings: HashMap<String, u32> = terminal_ids
                                    .iter()
                                    .filter_map(|id| {
                                        stream_id_map.get(id).map(|sid| (id.clone(), *sid))
                                    })
                                    .collect();
                                let resp = serde_json::to_string(&WsOutbound::Subscribed { mappings }).expect("BUG: WsOutbound must serialize");
                                if socket.send(Message::Text(resp.into())).await.is_err() {
                                    break;
                                }
                            }
                            Ok(WsInbound::Unsubscribe { terminal_ids }) => {
                                for id in &terminal_ids {
                                    subscribed_ids.remove(id);
                                }
                            }
                            Ok(WsInbound::SendText { terminal_id, text }) => {
                                let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
                                if state.bridge_tx.send(BridgeMessage {
                                    command: RemoteCommand::SendText { terminal_id, text },
                                    reply: reply_tx,
                                }).await.is_ok() {
                                    // Await reply so the sender doesn't error on a dropped receiver
                                    let _ = reply_rx.await;
                                }
                            }
                            Ok(WsInbound::SendSpecialKey { terminal_id, key }) => {
                                let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
                                if state.bridge_tx.send(BridgeMessage {
                                    command: RemoteCommand::SendSpecialKey { terminal_id, key },
                                    reply: reply_tx,
                                }).await.is_ok() {
                                    let _ = reply_rx.await;
                                }
                            }
                            Ok(WsInbound::Ping) => {
                                let resp = serde_json::to_string(&WsOutbound::Pong).expect("BUG: WsOutbound must serialize");
                                if socket.send(Message::Text(resp.into())).await.is_err() {
                                    break;
                                }
                            }
                            Ok(WsInbound::Auth { .. }) => {
                                // Already authenticated, ignore
                            }
                            Err(_) => {
                                let resp = serde_json::to_string(&WsOutbound::Error {
                                    error: "invalid message".into(),
                                }).expect("BUG: WsOutbound must serialize");
                                let _ = socket.send(Message::Text(resp.into())).await;
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {} // Ignore binary, ping, pong
                }
            }

            // PTY output broadcast
            result = pty_rx.recv() => {
                match result {
                    Ok(event) => {
                        if subscribed_ids.contains(&event.terminal_id) {
                            if let Some(&stream_id) = stream_id_map.get(&event.terminal_id) {
                                // Binary frame: [proto_version=1] [frame_type=1 (pty)] [u32 stream_id] [data...]
                                let mut frame = Vec::with_capacity(6 + event.data.len());
                                frame.push(1u8); // proto_version
                                frame.push(1u8); // frame_type: pty
                                frame.extend_from_slice(&stream_id.to_be_bytes());
                                frame.extend_from_slice(&event.data);
                                if socket.send(Message::Binary(frame.into())).await.is_err() {
                                    break;
                                }
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        let resp = serde_json::to_string(&WsOutbound::Dropped { count: n }).expect("BUG: WsOutbound must serialize");
                        if socket.send(Message::Text(resp.into())).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }

            // Periodic state version check (every 500ms)
            _ = tokio::time::sleep(std::time::Duration::from_millis(500)) => {
                let current = state.state_version.load(Ordering::Relaxed);
                if current != last_version {
                    last_version = current;
                    let resp = serde_json::to_string(&WsOutbound::StateChanged {
                        state_version: current,
                    }).expect("BUG: WsOutbound must serialize");
                    if socket.send(Message::Text(resp.into())).await.is_err() {
                        break;
                    }
                }
            }
        }
    }
}
