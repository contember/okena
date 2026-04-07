use crate::remote::bridge::{BridgeMessage, CommandResult, RemoteCommand};
use crate::remote::routes::AppState;
use crate::remote::types::{
    ActionRequest, WsInbound, WsOutbound, build_binary_frame, build_pty_frame, parse_binary_frame,
    FRAME_TYPE_INPUT, FRAME_TYPE_SNAPSHOT,
};
use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Query, State, WebSocketUpgrade};
use axum::response::IntoResponse;
use futures::{SinkExt, StreamExt};
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use tokio::sync::{broadcast, mpsc};

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

    // ── Split socket into reader/writer ─────────────────────────────────
    let (ws_write, mut ws_read) = socket.split();
    let (out_tx, out_rx) = mpsc::channel::<Message>(512);

    // Writer task: pumps messages from out_rx to the WebSocket sink.
    // Exits when out_rx is closed (reader dropped out_tx) or on write error.
    let writer_handle = tokio::spawn(ws_writer(ws_write, out_rx));

    // ── Main loop state ─────────────────────────────────────────────────
    let mut pty_rx = state.broadcaster.subscribe();
    let mut subscribed_ids: HashMap<String, u32> = HashMap::new(); // terminal_id -> stream_id
    let mut reverse_stream_map: HashMap<u32, String> = HashMap::new();
    let mut next_stream_id: u32 = 1;
    let connection_id = state.next_connection_id.fetch_add(1, Ordering::Relaxed);

    // Subscribe to state_version and git status changes
    let mut state_rx = state.state_version.subscribe();
    let mut git_rx = state.git_status.subscribe();

    // Pin the writer handle for use in select!
    tokio::pin!(writer_handle);

    loop {
        tokio::select! {
            // Incoming messages from client
            msg = ws_read.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        let parsed = serde_json::from_str::<WsInbound>(&text);
                        match parsed {
                            Ok(WsInbound::Subscribe { terminal_ids }) => {
                                for id in &terminal_ids {
                                    if !subscribed_ids.contains_key(id) {
                                        let sid = next_stream_id;
                                        subscribed_ids.insert(id.clone(), sid);
                                        reverse_stream_map.insert(sid, id.clone());
                                        next_stream_id += 1;
                                    }
                                }
                                // Sync to shared state for git polling
                                if let Ok(mut map) = state.remote_subscribed_terminals.write() {
                                    map.insert(connection_id, subscribed_ids.keys().cloned().collect());
                                }
                                let mappings: HashMap<String, u32> = terminal_ids
                                    .iter()
                                    .filter_map(|id| {
                                        subscribed_ids.get(id).map(|sid| (id.clone(), *sid))
                                    })
                                    .collect();
                                // Query terminal sizes so client can pre-resize before snapshot
                                let sizes = {
                                    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
                                    let ids = terminal_ids.clone();
                                    if state.bridge_tx.send(BridgeMessage {
                                        command: RemoteCommand::GetTerminalSizes { terminal_ids: ids },
                                        reply: Some(reply_tx),
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
                                if out_tx.send(Message::Text(resp.into())).await.is_err() {
                                    break;
                                }

                                // Send initial snapshots for all subscribed terminals
                                if send_snapshots(&out_tx, &state, &terminal_ids, &subscribed_ids).await.is_err() {
                                    break;
                                }
                                // Drain PTY events that accumulated before/during snapshot generation.
                                // The snapshot already contains their effects — replaying would garble the display.
                                while pty_rx.try_recv().is_ok() {}
                            }
                            Ok(WsInbound::Unsubscribe { terminal_ids }) => {
                                for id in &terminal_ids {
                                    if let Some(sid) = subscribed_ids.remove(id) {
                                        reverse_stream_map.remove(&sid);
                                    }
                                }
                                // Sync to shared state for git polling
                                if let Ok(mut map) = state.remote_subscribed_terminals.write() {
                                    if subscribed_ids.is_empty() {
                                        map.remove(&connection_id);
                                    } else {
                                        map.insert(connection_id, subscribed_ids.keys().cloned().collect());
                                    }
                                }
                            }
                            Ok(WsInbound::SendText { terminal_id, text }) => {
                                let _ = state.bridge_tx.send(BridgeMessage {
                                    command: RemoteCommand::Action(ActionRequest::SendText { terminal_id, text }),
                                    reply: None,
                                }).await;
                            }
                            Ok(WsInbound::SendSpecialKey { terminal_id, key }) => {
                                let _ = state.bridge_tx.send(BridgeMessage {
                                    command: RemoteCommand::Action(ActionRequest::SendSpecialKey { terminal_id, key }),
                                    reply: None,
                                }).await;
                            }
                            Ok(WsInbound::Resize { terminal_id, cols, rows }) => {
                                let _ = state.bridge_tx.send(BridgeMessage {
                                    command: RemoteCommand::Action(ActionRequest::Resize { terminal_id, cols, rows }),
                                    reply: None,
                                }).await;
                            }
                            Ok(WsInbound::Ping) => {
                                let resp = serde_json::to_string(&WsOutbound::Pong).expect("BUG: WsOutbound must serialize");
                                if out_tx.send(Message::Text(resp.into())).await.is_err() {
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
                                let _ = out_tx.send(Message::Text(resp.into())).await;
                            }
                        }
                    }
                    Some(Ok(Message::Binary(data))) => {
                        // Binary input frame from client — fire-and-forget
                        if let Some((FRAME_TYPE_INPUT, stream_id, payload)) = parse_binary_frame(&data) {
                            if let Some(terminal_id) = reverse_stream_map.get(&stream_id) {
                                let text = String::from_utf8_lossy(payload).to_string();
                                let _ = state.bridge_tx.send(BridgeMessage {
                                    command: RemoteCommand::Action(ActionRequest::SendText {
                                        terminal_id: terminal_id.clone(),
                                        text,
                                    }),
                                    reply: None,
                                }).await;
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {} // Ignore ping, pong
                }
            }

            // PTY output broadcast — coalesce pending events
            result = pty_rx.recv() => {
                match result {
                    Ok(event) => {
                        // Start a batch with the first event
                        let mut batch: HashMap<u32, Vec<u8>> = HashMap::new();
                        let mut resize_msgs: Vec<WsOutbound> = Vec::new();

                        match &event {
                            crate::remote::pty_broadcaster::PtyBroadcastEvent::Output { terminal_id, data } => {
                                if let Some(&stream_id) = subscribed_ids.get(terminal_id) {
                                    batch.entry(stream_id).or_default().extend_from_slice(data);
                                }
                            }
                            crate::remote::pty_broadcaster::PtyBroadcastEvent::Resized { terminal_id, cols, rows } => {
                                if subscribed_ids.contains_key(terminal_id) {
                                    resize_msgs.push(WsOutbound::TerminalResized {
                                        terminal_id: terminal_id.clone(),
                                        cols: *cols,
                                        rows: *rows,
                                    });
                                }
                            }
                        }

                        // Drain additional pending events (coalescing)
                        let mut channel_closed = false;
                        loop {
                            match pty_rx.try_recv() {
                                Ok(ev) => match &ev {
                                    crate::remote::pty_broadcaster::PtyBroadcastEvent::Output { terminal_id, data } => {
                                        if let Some(&sid) = subscribed_ids.get(terminal_id) {
                                            batch.entry(sid).or_default().extend_from_slice(data);
                                        }
                                    }
                                    crate::remote::pty_broadcaster::PtyBroadcastEvent::Resized { terminal_id, cols, rows } => {
                                        if subscribed_ids.contains_key(terminal_id) {
                                            // Keep only the latest resize per terminal
                                            resize_msgs.retain(|m| !matches!(m, WsOutbound::TerminalResized { terminal_id: id, .. } if id == terminal_id));
                                            resize_msgs.push(WsOutbound::TerminalResized {
                                                terminal_id: terminal_id.clone(),
                                                cols: *cols,
                                                rows: *rows,
                                            });
                                        }
                                    }
                                },
                                Err(broadcast::error::TryRecvError::Empty) => break,
                                Err(broadcast::error::TryRecvError::Lagged(n)) => {
                                    // Batch is stale — clear it and send snapshots instead
                                    batch.clear();
                                    resize_msgs.clear();
                                    let resp = serde_json::to_string(&WsOutbound::Dropped { count: n })
                                        .expect("BUG: WsOutbound must serialize");
                                    if out_tx.send(Message::Text(resp.into())).await.is_err() {
                                        channel_closed = true;
                                        break;
                                    }
                                    let ids: Vec<String> = subscribed_ids.keys().cloned().collect();
                                    if send_snapshots(&out_tx, &state, &ids, &subscribed_ids).await.is_err() {
                                        channel_closed = true;
                                        break;
                                    }
                                    while pty_rx.try_recv().is_ok() {}
                                    break;
                                }
                                Err(broadcast::error::TryRecvError::Closed) => {
                                    channel_closed = true;
                                    break;
                                }
                            }
                        }
                        if channel_closed {
                            break;
                        }

                        // Send resize notifications first (so client updates grid before PTY data)
                        for msg in resize_msgs {
                            let resp = serde_json::to_string(&msg).expect("BUG: WsOutbound must serialize");
                            if out_tx.send(Message::Text(resp.into())).await.is_err() {
                                channel_closed = true;
                                break;
                            }
                        }
                        if channel_closed {
                            break;
                        }

                        // Send coalesced PTY frames
                        for (stream_id, data) in batch {
                            let frame = build_pty_frame(stream_id, &data);
                            if out_tx.send(Message::Binary(frame.into())).await.is_err() {
                                channel_closed = true;
                                break;
                            }
                        }
                        if channel_closed {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        let resp = serde_json::to_string(&WsOutbound::Dropped { count: n }).expect("BUG: WsOutbound must serialize");
                        if out_tx.send(Message::Text(resp.into())).await.is_err() {
                            break;
                        }

                        // Auto-resync: send fresh snapshot for all subscribed terminals
                        let ids: Vec<String> = subscribed_ids.keys().cloned().collect();
                        if send_snapshots(&out_tx, &state, &ids, &subscribed_ids).await.is_err() {
                            break;
                        }
                        // Drain stale PTY events — snapshot already includes their effects.
                        while pty_rx.try_recv().is_ok() {}
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }

            // Immediate state version push
            result = state_rx.changed() => {
                if result.is_ok() {
                    let current = *state_rx.borrow_and_update();
                    let resp = serde_json::to_string(&WsOutbound::StateChanged {
                        state_version: current,
                    }).expect("BUG: WsOutbound must serialize");
                    if out_tx.send(Message::Text(resp.into())).await.is_err() {
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
                    if out_tx.send(Message::Text(resp.into())).await.is_err() {
                        break;
                    }
                }
            }

            // Writer task died (socket write error) — stop the reader too
            _ = &mut writer_handle => {
                break;
            }
        }
    }

    // Cleanup: remove this connection's subscribed terminals from shared state
    if let Ok(mut map) = state.remote_subscribed_terminals.write() {
        map.remove(&connection_id);
    }

    // Shutdown: dropping out_tx closes the writer's channel → writer exits.
    drop(out_tx);
    let _ = writer_handle.await;
}

/// Writer task: pumps messages from the mpsc channel to the WebSocket sink.
async fn ws_writer(
    mut ws_write: futures::stream::SplitSink<WebSocket, Message>,
    mut out_rx: mpsc::Receiver<Message>,
) {
    while let Some(msg) = out_rx.recv().await {
        if ws_write.send(msg).await.is_err() {
            break;
        }
    }
}

/// Send snapshot frames for the given terminal IDs via the mpsc channel.
/// Returns Err if the channel send fails (caller should break).
async fn send_snapshots(
    out_tx: &mpsc::Sender<Message>,
    state: &AppState,
    terminal_ids: &[String],
    subscribed_ids: &HashMap<String, u32>,
) -> Result<(), ()> {
    for id in terminal_ids {
        if let Some(&stream_id) = subscribed_ids.get(id) {
            let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
            if state
                .bridge_tx
                .send(BridgeMessage {
                    command: RemoteCommand::RenderSnapshot {
                        terminal_id: id.clone(),
                    },
                    reply: Some(reply_tx),
                })
                .await
                .is_ok()
            {
                if let Ok(CommandResult::OkBytes(snapshot)) = reply_rx.await {
                    let frame = build_binary_frame(FRAME_TYPE_SNAPSHOT, stream_id, &snapshot);
                    if out_tx.send(Message::Binary(frame.into())).await.is_err() {
                        return Err(());
                    }
                }
            }
        }
    }
    Ok(())
}
