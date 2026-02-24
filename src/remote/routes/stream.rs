use crate::remote::bridge::{BridgeMessage, CommandResult, RemoteCommand};
use crate::remote::routes::AppState;
use crate::remote::types::{
    ActionRequest, WsInbound, WsOutbound, build_binary_frame, build_pty_frame, parse_binary_frame,
    FRAME_TYPE_INPUT, FRAME_TYPE_SNAPSHOT,
};
use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Query, State, WebSocketUpgrade};
use axum::response::IntoResponse;
use std::collections::{HashMap, HashSet};
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
    let mut reverse_stream_map: HashMap<u32, String> = HashMap::new();
    let mut next_stream_id: u32 = 1;

    // Subscribe to state_version changes (immediate push, no polling)
    let mut state_rx = state.state_version.subscribe();

    // Subscribe to git status changes
    let mut git_rx = state.git_status.subscribe();

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
                                        let sid = next_stream_id;
                                        stream_id_map.insert(id.clone(), sid);
                                        reverse_stream_map.insert(sid, id.clone());
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
                                // Query terminal sizes so client can pre-resize before snapshot
                                let sizes = {
                                    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
                                    let ids = terminal_ids.clone();
                                    if state.bridge_tx.send(BridgeMessage {
                                        command: RemoteCommand::GetTerminalSizes { terminal_ids: ids },
                                        reply: reply_tx,
                                    }).await.is_ok() {
                                        match reply_rx.await {
                                            Ok(CommandResult::Ok(Some(val))) => {
                                                serde_json::from_value(val).unwrap_or_default()
                                            }
                                            _ => HashMap::new(),
                                        }
                                    } else {
                                        HashMap::new()
                                    }
                                };
                                let resp = serde_json::to_string(&WsOutbound::Subscribed { mappings, sizes }).expect("BUG: WsOutbound must serialize");
                                if socket.send(Message::Text(resp.into())).await.is_err() {
                                    break;
                                }

                                // Send initial snapshots for all subscribed terminals
                                if send_snapshots(&mut socket, &state, &terminal_ids, &stream_id_map).await.is_err() {
                                    break;
                                }
                                // Drain PTY events that accumulated before/during snapshot generation.
                                // The snapshot already contains their effects — replaying would garble the display.
                                while pty_rx.try_recv().is_ok() {}
                            }
                            Ok(WsInbound::Unsubscribe { terminal_ids }) => {
                                for id in &terminal_ids {
                                    subscribed_ids.remove(id);
                                }
                            }
                            Ok(WsInbound::SendText { terminal_id, text }) => {
                                let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
                                if state.bridge_tx.send(BridgeMessage {
                                    command: RemoteCommand::Action(ActionRequest::SendText { terminal_id, text }),
                                    reply: reply_tx,
                                }).await.is_ok() {
                                    let _ = reply_rx.await;
                                }
                            }
                            Ok(WsInbound::SendSpecialKey { terminal_id, key }) => {
                                let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
                                if state.bridge_tx.send(BridgeMessage {
                                    command: RemoteCommand::Action(ActionRequest::SendSpecialKey { terminal_id, key }),
                                    reply: reply_tx,
                                }).await.is_ok() {
                                    let _ = reply_rx.await;
                                }
                            }
                            Ok(WsInbound::Resize { terminal_id, cols, rows }) => {
                                let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
                                if state.bridge_tx.send(BridgeMessage {
                                    command: RemoteCommand::Action(ActionRequest::Resize { terminal_id, cols, rows }),
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
                            Ok(WsInbound::SubscribeApps { .. })
                            | Ok(WsInbound::UnsubscribeApps { .. })
                            | Ok(WsInbound::AppAction { .. }) => {
                                // Not yet implemented — handled in a later issue
                            }
                            Err(_) => {
                                let resp = serde_json::to_string(&WsOutbound::Error {
                                    error: "invalid message".into(),
                                }).expect("BUG: WsOutbound must serialize");
                                let _ = socket.send(Message::Text(resp.into())).await;
                            }
                        }
                    }
                    Some(Ok(Message::Binary(data))) => {
                        // Binary input frame from client
                        if let Some((FRAME_TYPE_INPUT, stream_id, payload)) = parse_binary_frame(&data) {
                            if let Some(terminal_id) = reverse_stream_map.get(&stream_id) {
                                let text = String::from_utf8_lossy(payload).to_string();
                                let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
                                if state.bridge_tx.send(BridgeMessage {
                                    command: RemoteCommand::Action(ActionRequest::SendText {
                                        terminal_id: terminal_id.clone(),
                                        text,
                                    }),
                                    reply: reply_tx,
                                }).await.is_ok() {
                                    let _ = reply_rx.await;
                                }
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {} // Ignore ping, pong
                }
            }

            // PTY output broadcast
            result = pty_rx.recv() => {
                match result {
                    Ok(event) => {
                        if subscribed_ids.contains(&event.terminal_id) {
                            if let Some(&stream_id) = stream_id_map.get(&event.terminal_id) {
                                let frame = build_pty_frame(stream_id, &event.data);
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

                        // Auto-resync: send fresh snapshot for all subscribed terminals
                        let ids: Vec<String> = subscribed_ids.iter().cloned().collect();
                        if send_snapshots(&mut socket, &state, &ids, &stream_id_map).await.is_err() {
                            break;
                        }
                        // Drain stale PTY events — snapshot already includes their effects.
                        while pty_rx.try_recv().is_ok() {}
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }

            // Immediate state version push (no polling)
            result = state_rx.changed() => {
                if result.is_ok() {
                    let current = *state_rx.borrow_and_update();
                    let resp = serde_json::to_string(&WsOutbound::StateChanged {
                        state_version: current,
                    }).expect("BUG: WsOutbound must serialize");
                    if socket.send(Message::Text(resp.into())).await.is_err() {
                        break;
                    }
                } else {
                    // Sender dropped
                    break;
                }
            }

            // Git status changes push
            result = git_rx.changed() => {
                if result.is_ok() {
                    let statuses = git_rx.borrow_and_update().clone();
                    let resp = serde_json::to_string(&WsOutbound::GitStatusChanged {
                        projects: statuses,
                    }).expect("BUG: WsOutbound must serialize");
                    if socket.send(Message::Text(resp.into())).await.is_err() {
                        break;
                    }
                }
            }
        }
    }
}

/// Send snapshot frames for the given terminal IDs.
/// Returns Err if the socket write fails (caller should break).
async fn send_snapshots(
    socket: &mut WebSocket,
    state: &AppState,
    terminal_ids: &[String],
    stream_id_map: &HashMap<String, u32>,
) -> Result<(), ()> {
    for id in terminal_ids {
        if let Some(&stream_id) = stream_id_map.get(id) {
            let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
            if state
                .bridge_tx
                .send(BridgeMessage {
                    command: RemoteCommand::RenderSnapshot {
                        terminal_id: id.clone(),
                    },
                    reply: reply_tx,
                })
                .await
                .is_ok()
            {
                if let Ok(CommandResult::OkBytes(snapshot)) = reply_rx.await {
                    let frame = build_binary_frame(FRAME_TYPE_SNAPSHOT, stream_id, &snapshot);
                    if socket.send(Message::Binary(frame.into())).await.is_err() {
                        return Err(());
                    }
                }
            }
        }
    }
    Ok(())
}
