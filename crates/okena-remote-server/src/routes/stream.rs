// All `.expect("BUG: WsOutbound must serialize")` sites in this file serialize
// internal WsOutbound DTOs whose Serialize impls cannot fail in practice.
#![allow(clippy::expect_used)]

use crate::bridge::{BridgeMessage, CommandResult, RemoteCommand};
use crate::routes::{AppState, PeerInfo};
use crate::types::{
    ActionRequest, ApiSystemStats, FRAME_TYPE_INPUT, FRAME_TYPE_SNAPSHOT, WsInbound, WsOutbound,
    build_binary_frame, build_pty_frame, parse_binary_frame,
};
use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Extension, Query, State, WebSocketUpgrade};
use axum::response::IntoResponse;
use futures::{SinkExt, StreamExt};
use okena_core::git_poll::GitPollTrigger;
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::time::Duration;
use sysinfo::System;
use tokio::sync::{broadcast, mpsc};

const SYSTEM_STATS_REFRESH_INTERVAL: Duration = Duration::from_secs(2);

struct SystemStatsCache {
    system: System,
    stats: ApiSystemStats,
}

impl SystemStatsCache {
    fn new() -> Self {
        let mut system = System::new();
        system.refresh_cpu_usage();
        system.refresh_memory();

        Self {
            system,
            stats: ApiSystemStats::default(),
        }
    }

    fn refresh(&mut self) {
        self.system.refresh_cpu_usage();
        self.system.refresh_memory();

        let mut total_cpu = 0.0;
        let mut cpu_count = 0.0;
        for cpu in self.system.cpus() {
            total_cpu += cpu.cpu_usage();
            cpu_count += 1.0;
        }

        self.stats = ApiSystemStats {
            cpu_usage: if cpu_count > 0.0 {
                total_cpu / cpu_count
            } else {
                0.0
            },
            memory_used_bytes: self.system.used_memory(),
            memory_total_bytes: self.system.total_memory(),
        };
    }

    fn stats(&self) -> ApiSystemStats {
        self.stats.clone()
    }
}

#[derive(serde::Deserialize)]
pub struct WsQuery {
    pub token: Option<String>,
}

pub async fn ws_handler(
    State(state): State<AppState>,
    Query(query): Query<WsQuery>,
    Extension(peer): Extension<PeerInfo>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_ws(socket, state, query.token, peer))
}

async fn handle_ws(
    mut socket: WebSocket,
    state: AppState,
    query_token: Option<String>,
    peer: PeerInfo,
) {
    // ── Auth phase ──────────────────────────────────────────────────────
    // Unix socket clients are same-user local clients; bearer auth is for TCP.
    let authenticated = if matches!(peer, PeerInfo::Local) {
        true
    } else if let Some(token) = query_token {
        state.auth_store.validate_token(&token)
    } else {
        // Wait for first-message auth (2 second timeout)
        match tokio::time::timeout(std::time::Duration::from_secs(2), socket.recv()).await {
            Ok(Some(Ok(Message::Text(text)))) => {
                if let Ok(WsInbound::Auth { token }) = serde_json::from_str::<WsInbound>(&text) {
                    state.auth_store.validate_token(&token)
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
    let connection_owner_id = connection_id.to_string();
    // Register this live connection (deregistered in the cleanup block below).
    // `/v1/shutdown` counts these to refuse while any client is still connected.
    // We register only after auth so unauthenticated dial attempts don't count.
    state.active_connections.fetch_add(1, Ordering::Relaxed);

    // Subscribe to state_version and git status changes
    let mut state_rx = state.state_version.subscribe();
    let mut git_rx = state.git_status.subscribe();
    // Subscribe to daemon-originated toasts (fire-and-forget broadcast).
    let mut toast_rx = state.toast_tx.subscribe();
    // Once the toast sender is gone we disable that select arm, otherwise its
    // `recv()` would resolve `Err(Closed)` instantly and busy-spin the loop.
    let mut toast_open = true;
    let mut system_stats = SystemStatsCache::new();
    let mut system_stats_interval = tokio::time::interval(SYSTEM_STATS_REFRESH_INTERVAL);
    system_stats_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

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
                                if let Some(tx) = &state.git_poll_trigger_tx {
                                    let _ = tx.send(GitPollTrigger::visibility_changed());
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
                                // Terminals already present in the registry (so
                                // `GetTerminalSizes` returned a size for them)
                                // existed BEFORE this subscribe: their snapshot
                                // reflects current state, so the pending PTY
                                // events that the snapshot already accounts for
                                // must be drained (replaying would garble the
                                // display). Terminals absent here are spawned
                                // lazily by `ensure_terminal` DURING this
                                // subscribe — their snapshot was empty (the shell
                                // hadn't printed yet), so their first output must
                                // NOT be dropped.
                                let pre_existing: std::collections::HashSet<String> =
                                    sizes.keys().cloned().collect();
                                let resp = serde_json::to_string(&WsOutbound::Subscribed { mappings, sizes: sizes.clone() }).expect("BUG: WsOutbound must serialize");
                                if out_tx.send(Message::Text(resp.into())).await.is_err() {
                                    break;
                                }
                                if send_authority_resizes(
                                    &out_tx,
                                    &sizes,
                                    &connection_owner_id,
                                )
                                .await
                                .is_err()
                                {
                                    break;
                                }

                                // Send initial snapshots for all subscribed terminals
                                if send_snapshots(&out_tx, &state, &terminal_ids, &subscribed_ids).await.is_err() {
                                    break;
                                }
                                // Selectively drain PTY events that accumulated
                                // before/during snapshot generation. For
                                // pre-existing terminals the snapshot already
                                // contains their effects, so drop them. For
                                // just-spawned terminals (subscribed but absent
                                // from `pre_existing`) FORWARD the events — the
                                // shell's first prompt arrives here and the empty
                                // snapshot did not cover it, so dropping it would
                                // leave the pane blank until the next keypress.
                                if drain_or_forward_post_snapshot(
                                    &out_tx,
                                    &mut pty_rx,
                                    &subscribed_ids,
                                    &pre_existing,
                                    &connection_owner_id,
                                )
                                .await
                                .is_err()
                                {
                                    break;
                                }
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
                                    command: RemoteCommand::ActionFromConnection {
                                        action: ActionRequest::SendText { terminal_id, text },
                                        connection_id: connection_owner_id.clone(),
                                    },
                                    reply: None,
                                }).await;
                            }
                            Ok(WsInbound::SendSpecialKey { terminal_id, key }) => {
                                let _ = state.bridge_tx.send(BridgeMessage {
                                    command: RemoteCommand::ActionFromConnection {
                                        action: ActionRequest::SendSpecialKey { terminal_id, key },
                                        connection_id: connection_owner_id.clone(),
                                    },
                                    reply: None,
                                }).await;
                            }
                            Ok(WsInbound::Resize { terminal_id, cols, rows }) => {
                                // Ask for a reply: a denied resize carries the
                                // authoritative size, which we bounce back to
                                // THIS client as a server-owned resize so it
                                // reverts its optimistic grid and stops
                                // re-asserting instead of silently diverging.
                                let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
                                let sent = state.bridge_tx.send(BridgeMessage {
                                    command: RemoteCommand::ResizeFromConnection {
                                        terminal_id: terminal_id.clone(),
                                        cols,
                                        rows,
                                        connection_id: connection_owner_id.clone(),
                                    },
                                    reply: Some(reply_tx),
                                }).await.is_ok();
                                if sent
                                    && let Ok(CommandResult::Ok(Some(value))) = reply_rx.await
                                    && value.get("denied").and_then(|d| d.as_bool()) == Some(true)
                                    && let (Some(cols), Some(rows)) = (
                                        value.get("cols").and_then(|c| c.as_u64()),
                                        value.get("rows").and_then(|r| r.as_u64()),
                                    )
                                {
                                    let msg = WsOutbound::TerminalResized {
                                        terminal_id,
                                        cols: cols as u16,
                                        rows: rows as u16,
                                        server_owns: true,
                                    };
                                    let resp = serde_json::to_string(&msg)
                                        .expect("BUG: WsOutbound must serialize");
                                    if out_tx.send(Message::Text(resp.into())).await.is_err() {
                                        break;
                                    }
                                }
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
                        if let Some((FRAME_TYPE_INPUT, stream_id, payload)) = parse_binary_frame(&data)
                            && let Some(terminal_id) = reverse_stream_map.get(&stream_id) {
                                let text = String::from_utf8_lossy(payload).to_string();
                                let _ = state.bridge_tx.send(BridgeMessage {
                                    command: RemoteCommand::ActionFromConnection {
                                        action: ActionRequest::SendText {
                                            terminal_id: terminal_id.clone(),
                                            text,
                                        },
                                        connection_id: connection_owner_id.clone(),
                                    },
                                    reply: None,
                                }).await;
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
                            crate::pty_broadcaster::PtyBroadcastEvent::Output { terminal_id, data } => {
                                if let Some(&stream_id) = subscribed_ids.get(terminal_id) {
                                    batch.entry(stream_id).or_default().extend_from_slice(data);
                                }
                            }
                            crate::pty_broadcaster::PtyBroadcastEvent::Resized {
                                terminal_id,
                                cols,
                                rows,
                                server_owns,
                                owner_connection_id,
                            } => {
                                if subscribed_ids.contains_key(terminal_id) {
                                    resize_msgs.push(terminal_resized_for_recipient(
                                        terminal_id.clone(),
                                        *cols,
                                        *rows,
                                        *server_owns,
                                        owner_connection_id.as_deref(),
                                        &connection_owner_id,
                                    ));
                                }
                            }
                        }

                        // Drain additional pending events (coalescing)
                        let mut channel_closed = false;
                        loop {
                            match pty_rx.try_recv() {
                                Ok(ev) => match &ev {
                                    crate::pty_broadcaster::PtyBroadcastEvent::Output { terminal_id, data } => {
                                        if let Some(&sid) = subscribed_ids.get(terminal_id) {
                                            batch.entry(sid).or_default().extend_from_slice(data);
                                        }
                                    }
                                    crate::pty_broadcaster::PtyBroadcastEvent::Resized {
                                        terminal_id,
                                        cols,
                                        rows,
                                        server_owns,
                                        owner_connection_id,
                                    } => {
                                        if subscribed_ids.contains_key(terminal_id) {
                                            // Keep only the latest resize per terminal
                                            resize_msgs.retain(|m| !matches!(m, WsOutbound::TerminalResized { terminal_id: id, .. } if id == terminal_id));
                                            resize_msgs.push(terminal_resized_for_recipient(
                                                terminal_id.clone(),
                                                *cols,
                                                *rows,
                                                *server_owns,
                                                owner_connection_id.as_deref(),
                                                &connection_owner_id,
                                            ));
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

            // Periodic host metrics for remote status UI. Kept separate from
            // `state_version` so status refreshes do not force workspace resync.
            _ = system_stats_interval.tick() => {
                system_stats.refresh();
                let resp = serde_json::to_string(&WsOutbound::SystemStatsChanged {
                    stats: system_stats.stats(),
                }).expect("BUG: WsOutbound must serialize");
                if out_tx.send(Message::Text(resp.into())).await.is_err() {
                    break;
                }
            }

            // Daemon-originated toast push (fire-and-forget broadcast).
            result = toast_rx.recv(), if toast_open => {
                match result {
                    Ok(api_toast) => {
                        let resp = serde_json::to_string(&WsOutbound::Toast(api_toast))
                            .expect("BUG: WsOutbound must serialize");
                        if out_tx.send(Message::Text(resp.into())).await.is_err() {
                            break;
                        }
                    }
                    // A slow client missed some toasts — they are non-critical
                    // notifications, so just keep receiving the newer ones.
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        log::debug!("toast broadcast lagged, dropped {n} toast(s) for a client");
                    }
                    // Sender gone (daemon shutting down) — stop polling this arm so
                    // it can't busy-spin; the connection lives on until it closes.
                    Err(broadcast::error::RecvError::Closed) => {
                        toast_open = false;
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

    // Deregister the live connection (paired with the fetch_add above).
    state.active_connections.fetch_sub(1, Ordering::Relaxed);

    // Release resize ownership so a reconnecting client isn't denied by a
    // dead owner; the next resize from any connection adopts it.
    okena_terminal::terminal::release_remote_resize_owner(&connection_owner_id);

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

fn terminal_resized_for_recipient(
    terminal_id: String,
    cols: u16,
    rows: u16,
    server_owns: bool,
    owner_connection_id: Option<&str>,
    recipient_connection_id: &str,
) -> WsOutbound {
    let server_owns =
        server_owns || owner_connection_id.is_some_and(|owner| owner != recipient_connection_id);
    WsOutbound::TerminalResized {
        terminal_id,
        cols,
        rows,
        server_owns,
    }
}

async fn send_authority_resizes(
    out_tx: &mpsc::Sender<Message>,
    sizes: &HashMap<String, (u16, u16)>,
    recipient_connection_id: &str,
) -> Result<(), ()> {
    let authority = okena_terminal::terminal::resize_authority_snapshot("");
    if !authority.claimed {
        return Ok(());
    }
    for (terminal_id, (cols, rows)) in sizes {
        let msg = terminal_resized_for_recipient(
            terminal_id.clone(),
            *cols,
            *rows,
            authority.local,
            authority.remote_owner_id.as_deref(),
            recipient_connection_id,
        );
        let resp = serde_json::to_string(&msg).expect("BUG: WsOutbound must serialize");
        if out_tx.send(Message::Text(resp.into())).await.is_err() {
            return Err(());
        }
    }
    Ok(())
}

/// Drain the PTY events that accumulated before/during snapshot generation at
/// subscribe time, discarding those already reflected in a terminal's snapshot
/// and forwarding those that are not.
///
/// The plain blanket drain (`while pty_rx.try_recv().is_ok() {}`) is correct for
/// terminals that already had a live PTY before this subscribe: their snapshot
/// reflects current state, so replaying the queued events would garble the
/// display. But a terminal spawned lazily *during* this subscribe (its
/// `render_snapshot()` was empty because the shell hadn't printed yet) has its
/// first output sitting in the queue — dropping it would leave the pane blank
/// until the next keypress. So:
///
/// * events for `pre_existing` terminals (snapshot covers them) are dropped;
/// * events for subscribed-but-just-spawned terminals are forwarded as the same
///   resize-then-PTY frames the main loop sends;
/// * events for non-subscribed terminals are dropped (not ours).
///
/// On `Lagged` the queued backlog is meaningless (we already lost events), so we
/// simply stop draining; the main loop's own lag handling resynchronizes via
/// fresh snapshots. Returns `Err(())` if a client send fails (caller breaks).
async fn drain_or_forward_post_snapshot(
    out_tx: &mpsc::Sender<Message>,
    pty_rx: &mut broadcast::Receiver<crate::pty_broadcaster::PtyBroadcastEvent>,
    subscribed_ids: &HashMap<String, u32>,
    pre_existing: &std::collections::HashSet<String>,
    connection_owner_id: &str,
) -> Result<(), ()> {
    use crate::pty_broadcaster::PtyBroadcastEvent;

    // Coalesce forwarded output per stream and keep only the latest resize per
    // terminal, matching the main loop's batching.
    let mut batch: HashMap<u32, Vec<u8>> = HashMap::new();
    let mut resize_msgs: Vec<WsOutbound> = Vec::new();

    while let Ok(event) = pty_rx.try_recv() {
        match event {
            PtyBroadcastEvent::Output { terminal_id, data } => {
                // Forward only for subscribed terminals that were NOT already
                // covered by a snapshot (i.e. just spawned this subscribe).
                if !pre_existing.contains(&terminal_id)
                    && let Some(&stream_id) = subscribed_ids.get(&terminal_id)
                {
                    batch.entry(stream_id).or_default().extend_from_slice(&data);
                }
            }
            PtyBroadcastEvent::Resized {
                terminal_id,
                cols,
                rows,
                server_owns,
                owner_connection_id,
            } => {
                if !pre_existing.contains(&terminal_id) && subscribed_ids.contains_key(&terminal_id)
                {
                    resize_msgs.retain(|m| {
                        !matches!(m, WsOutbound::TerminalResized { terminal_id: id, .. } if *id == terminal_id)
                    });
                    resize_msgs.push(terminal_resized_for_recipient(
                        terminal_id,
                        cols,
                        rows,
                        server_owns,
                        owner_connection_id.as_deref(),
                        connection_owner_id,
                    ));
                }
            }
        }
    }

    // Resize first (so the client updates its grid before applying PTY data),
    // then the coalesced PTY frames — same ordering as the main broadcast arm.
    for msg in resize_msgs {
        let resp = serde_json::to_string(&msg).expect("BUG: WsOutbound must serialize");
        if out_tx.send(Message::Text(resp.into())).await.is_err() {
            return Err(());
        }
    }
    for (stream_id, data) in batch {
        let frame = build_pty_frame(stream_id, &data);
        if out_tx.send(Message::Binary(frame.into())).await.is_err() {
            return Err(());
        }
    }
    Ok(())
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
                && let Ok(CommandResult::OkBytes(snapshot)) = reply_rx.await
            {
                let frame = build_binary_frame(FRAME_TYPE_SNAPSHOT, stream_id, &snapshot);
                if out_tx.send(Message::Binary(frame.into())).await.is_err() {
                    return Err(());
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resized_server_owns(
        server_owns: bool,
        owner_connection_id: Option<&str>,
        recipient_connection_id: &str,
    ) -> bool {
        match terminal_resized_for_recipient(
            "t".to_string(),
            120,
            40,
            server_owns,
            owner_connection_id,
            recipient_connection_id,
        ) {
            WsOutbound::TerminalResized { server_owns, .. } => server_owns,
            _ => unreachable!("helper always returns TerminalResized"),
        }
    }

    #[test]
    fn resize_echo_stays_client_owned_for_origin_connection() {
        assert!(!resized_server_owns(false, Some("conn-a"), "conn-a"));
    }

    #[test]
    fn resize_from_other_connection_makes_recipient_defer() {
        assert!(resized_server_owns(false, Some("conn-a"), "conn-b"));
    }

    #[test]
    fn server_owned_resize_makes_every_remote_defer() {
        assert!(resized_server_owns(true, None, "conn-a"));
    }

    #[test]
    fn legacy_unknown_remote_owner_keeps_prior_client_behavior() {
        assert!(!resized_server_owns(false, None, "conn-a"));
    }
}
