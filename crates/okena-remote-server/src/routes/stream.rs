// All `.expect("BUG: WsOutbound must serialize")` sites in this file serialize
// internal WsOutbound DTOs whose Serialize impls cannot fail in practice.
#![allow(clippy::expect_used)]

use crate::bridge::{BridgeMessage, BridgeSender, CommandResult, RemoteCommand};
use crate::pty_broadcaster::PtyBroadcaster;
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
use std::collections::{HashMap, HashSet};
use std::sync::atomic::Ordering;
use std::time::Duration;
use sysinfo::System;
use tokio::sync::{broadcast, mpsc};

const SYSTEM_STATS_REFRESH_INTERVAL: Duration = Duration::from_secs(2);
/// How long a batch of snapshots waits for terminal models that stopped
/// advancing to reach the last published output.
const SNAPSHOT_CATCH_UP_DEADLINE: Duration = Duration::from_secs(2);
const SNAPSHOT_CATCH_UP_POLL: Duration = Duration::from_millis(10);

fn output_is_newer(watermarks: &HashMap<String, u64>, terminal_id: &str, sequence: u64) -> bool {
    sequence > watermarks.get(terminal_id).copied().unwrap_or(0)
}

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
    // A bearer session keeps its token identity so revocation and expiry can end
    // the connection, not just refuse the next handshake. The receiver comes
    // back with the session so a revocation racing the handshake is not lost.
    let local_peer = matches!(peer, PeerInfo::Local);
    let (mut auth_revocations, session) = if local_peer {
        (state.auth_store.subscribe_revocations(), None)
    } else if let Some(token) = query_token {
        state.auth_store.authenticate_watched(&token)
    } else {
        // Wait for first-message auth (2 second timeout)
        match tokio::time::timeout(std::time::Duration::from_secs(2), socket.recv()).await {
            Ok(Some(Ok(Message::Text(text)))) => match serde_json::from_str::<WsInbound>(&text) {
                Ok(WsInbound::Auth { token }) => state.auth_store.authenticate_watched(&token),
                _ => (state.auth_store.subscribe_revocations(), None),
            },
            _ => (state.auth_store.subscribe_revocations(), None),
        }
    };

    if !local_peer && session.is_none() {
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
    let mut output_watermarks: HashMap<String, u64> = HashMap::new();
    // Subscribed terminals the daemon had no snapshot for; their next output
    // triggers one instead of being forwarded onto a grid the client never got.
    let mut unsnapshotted: HashSet<String> = HashSet::new();
    let mut reverse_stream_map: HashMap<u32, String> = HashMap::new();
    let mut next_stream_id: u32 = 1;
    let connection_id = state.next_connection_id.fetch_add(1, Ordering::Relaxed);
    let connection_owner_id = connection_id.to_string();
    // Register this live connection (deregistered in the cleanup block below).
    // `/v1/shutdown` counts these to refuse while any client is still connected.
    // We register only after auth so unauthenticated dial attempts don't count.
    state.active_connections.fetch_add(1, Ordering::SeqCst);
    // Record that this daemon has served a client, arming the idle-exit monitor.
    state.had_client.store(true, Ordering::SeqCst);
    // A desktop on this host is one `POST /v1/update/restart` can relaunch.
    if peer.is_local_trusted()
        && let Ok(mut set) = state.local_connections.write()
    {
        set.insert(connection_id);
    }

    // Subscribe to state_version and git status changes
    let mut state_rx = state.state_version.subscribe();
    let mut git_rx = state.git_status.subscribe();
    // Subscribe to daemon-originated toasts (fire-and-forget broadcast).
    let mut toast_rx = state.toast_tx.subscribe();
    let mut presentation_rx = state.presentation_tx.subscribe();
    let mut app_restart_rx = state.app_restart_tx.subscribe();
    // Agent usage: push the current snapshot now, then every change. The
    // usage polls run while at least one client is registered.
    let _usage_client = state.usage.register_client();
    let mut usage_rx = state.usage.subscribe();
    let mut usage_open = true;
    {
        let usage = usage_rx.borrow_and_update().clone();
        let msg = serde_json::to_string(&WsOutbound::UsageChanged { usage })
            .expect("BUG: WsOutbound must serialize");
        // A closed writer is noticed by the main loop, which runs the
        // connection cleanup; returning here would skip it.
        let _ = out_tx.send(Message::Text(msg.into())).await;
    }
    // Once a sender is gone we disable its select arm, otherwise `recv()` would
    // resolve `Err(Closed)` instantly and busy-spin the loop.
    let mut toast_open = true;
    let mut presentation_open = true;
    let mut app_restart_open = true;
    let mut auth_watch_open = session.is_some();
    let mut auth_deadline = session_deadline(session.as_ref());
    let mut system_stats = SystemStatsCache::new();
    let mut system_stats_interval = tokio::time::interval(SYSTEM_STATS_REFRESH_INTERVAL);
    system_stats_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    // Pin the writer handle for use in select!
    tokio::pin!(writer_handle);
    let mut writer_finished = false;

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

                                let sent = match send_snapshots_and_reconcile(
                                    &out_tx,
                                    &state,
                                    &mut pty_rx,
                                    &terminal_ids,
                                    &subscribed_ids,
                                    &connection_owner_id,
                                )
                                .await
                                {
                                    Ok(sent) => sent,
                                    Err(()) => break,
                                };
                                sent.record(&mut output_watermarks, &mut unsnapshotted);
                            }
                            Ok(WsInbound::Unsubscribe { terminal_ids }) => {
                                for id in &terminal_ids {
                                    if let Some(sid) = subscribed_ids.remove(id) {
                                        reverse_stream_map.remove(&sid);
                                    }
                                    output_watermarks.remove(id);
                                    unsnapshotted.remove(id);
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
                            Ok(WsInbound::SetVisibleProjects { project_ids }) => {
                                // Full replacement set, kept even when empty: the git
                                // poller trusts a declared viewport over subscriptions.
                                if let Ok(mut map) = state.remote_visible_projects.write() {
                                    map.insert(connection_id, project_ids.into_iter().collect());
                                }
                                if let Some(tx) = &state.git_poll_trigger_tx {
                                    let _ = tx.send(GitPollTrigger::visibility_changed());
                                }
                            }
                            Ok(WsInbound::SetWindows { windows }) => {
                                if let Ok(mut map) = state.remote_windows.write() {
                                    map.insert(connection_id, windows);
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
                            Ok(WsInbound::SendBytes { terminal_id, data }) => {
                                okena_core::latency_probe::daemon_input_received(
                                    &terminal_id,
                                    &data,
                                );
                                let _ = state.bridge_tx.send(BridgeMessage {
                                    command: RemoteCommand::ActionFromConnection {
                                        action: ActionRequest::SendBytes { terminal_id, data },
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
                                okena_core::latency_probe::daemon_input_received(
                                    terminal_id,
                                    payload,
                                );
                                let _ = state.bridge_tx.send(BridgeMessage {
                                    command: RemoteCommand::ActionFromConnection {
                                        action: ActionRequest::SendBytes {
                                            terminal_id: terminal_id.clone(),
                                            data: payload.to_vec(),
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
                        let mut resnapshot: HashSet<String> = HashSet::new();

                        match &event {
                            crate::pty_broadcaster::PtyBroadcastEvent::Output { terminal_id, data, sequence } => {
                                if unsnapshotted.contains(terminal_id) {
                                    resnapshot.insert(terminal_id.clone());
                                } else if output_is_newer(&output_watermarks, terminal_id, *sequence)
                                    && let Some(&stream_id) = subscribed_ids.get(terminal_id)
                                {
                                    okena_core::latency_probe::daemon_stream_queued(terminal_id);
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
                                    crate::pty_broadcaster::PtyBroadcastEvent::Output { terminal_id, data, sequence } => {
                                        if unsnapshotted.contains(terminal_id) {
                                            resnapshot.insert(terminal_id.clone());
                                        } else if output_is_newer(&output_watermarks, terminal_id, *sequence)
                                            && let Some(&sid) = subscribed_ids.get(terminal_id)
                                        {
                                            okena_core::latency_probe::daemon_stream_queued(terminal_id);
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
                                    resnapshot.clear();
                                    let ids: Vec<String> = subscribed_ids.keys().cloned().collect();
                                    match send_snapshots_and_reconcile(
                                        &out_tx,
                                        &state,
                                        &mut pty_rx,
                                        &ids,
                                        &subscribed_ids,
                                        &connection_owner_id,
                                    )
                                    .await
                                    {
                                        Ok(sent) => sent.record(&mut output_watermarks, &mut unsnapshotted),
                                        Err(()) => {
                                            channel_closed = true;
                                            break;
                                        }
                                    }
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

                        if !resnapshot.is_empty() {
                            let ids: Vec<String> = resnapshot.into_iter().collect();
                            match send_snapshots_and_reconcile(
                                &out_tx,
                                &state,
                                &mut pty_rx,
                                &ids,
                                &subscribed_ids,
                                &connection_owner_id,
                            )
                            .await
                            {
                                Ok(sent) => sent.record(&mut output_watermarks, &mut unsnapshotted),
                                Err(()) => break,
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        let resp = serde_json::to_string(&WsOutbound::Dropped { count: n }).expect("BUG: WsOutbound must serialize");
                        if out_tx.send(Message::Text(resp.into())).await.is_err() {
                            break;
                        }

                        // Auto-resync: send fresh snapshot for all subscribed terminals
                        let ids: Vec<String> = subscribed_ids.keys().cloned().collect();
                        match send_snapshots_and_reconcile(
                            &out_tx,
                            &state,
                            &mut pty_rx,
                            &ids,
                            &subscribed_ids,
                            &connection_owner_id,
                        )
                        .await
                        {
                            Ok(sent) => sent.record(&mut output_watermarks, &mut unsnapshotted),
                            Err(()) => break,
                        }
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

            // Agent usage snapshot changed (fetch started/finished, settings).
            result = usage_rx.changed(), if usage_open => {
                if result.is_ok() {
                    let usage = usage_rx.borrow_and_update().clone();
                    let resp = serde_json::to_string(&WsOutbound::UsageChanged { usage })
                        .expect("BUG: WsOutbound must serialize");
                    if out_tx.send(Message::Text(resp.into())).await.is_err() {
                        break;
                    }
                } else {
                    // The hub's sender lives in AppState, so this only happens
                    // during teardown; stop polling the closed channel.
                    usage_open = false;
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

            // One-shot presentation request for connected desktop clients.
            result = presentation_rx.recv(), if presentation_open => {
                match result {
                    Ok(request) => {
                        let resp = serde_json::to_string(&WsOutbound::from(request))
                            .expect("BUG: WsOutbound must serialize");
                        if out_tx.send(Message::Text(resp.into())).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        log::debug!("presentation broadcast lagged, dropped {n} request(s) for a client");
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        presentation_open = false;
                    }
                }
            }

            // Relaunch request for the desktops `POST /v1/update/restart` named.
            result = app_restart_rx.recv(), if app_restart_open => {
                match result {
                    Ok(push) if push.connections.contains(&connection_id) => {
                        let resp = serde_json::to_string(&WsOutbound::AppRestartRequested(push.request))
                            .expect("BUG: WsOutbound must serialize");
                        if out_tx.send(Message::Text(resp.into())).await.is_err() {
                            break;
                        }
                    }
                    Ok(_) => {}
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        log::warn!("app restart broadcast lagged, dropped {n} request(s) for a client");
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        app_restart_open = false;
                    }
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

            // The token behind this connection was revoked, reloaded away, or evicted.
            result = auth_revocations.changed(), if auth_watch_open => {
                match (result, session.as_ref()) {
                    (Err(_), _) => auth_watch_open = false,
                    (Ok(()), Some(session)) if !state.auth_store.session_is_live(session) => {
                        log::info!("Closing WS connection {connection_id}: its token is no longer valid");
                        send_auth_lost(&out_tx);
                        break;
                    }
                    _ => {}
                }
            }

            // The token behind this connection reached its expiry. The monotonic
            // timer does not advance across host suspend, so the wall clock — the
            // one `validate_token` uses — decides.
            _ = sleep_until(auth_deadline) => {
                match session.as_ref() {
                    Some(session) if std::time::SystemTime::now() < session.expires_at => {
                        auth_deadline = session_deadline(Some(session));
                    }
                    _ => {
                        log::info!("Closing WS connection {connection_id}: its token expired");
                        send_auth_lost(&out_tx);
                        break;
                    }
                }
            }

            // Writer task died (socket write error) — stop the reader too
            _ = &mut writer_handle => {
                writer_finished = true;
                break;
            }
        }
    }

    // Cleanup: remove this connection's subscribed terminals from shared state
    if let Ok(mut map) = state.remote_subscribed_terminals.write() {
        map.remove(&connection_id);
    }
    // Same for its declared viewport and windows — a gone client renders nothing.
    if let Ok(mut map) = state.remote_visible_projects.write() {
        map.remove(&connection_id);
    }
    if let Ok(mut map) = state.remote_windows.write() {
        map.remove(&connection_id);
    }
    if let Ok(mut set) = state.local_connections.write() {
        set.remove(&connection_id);
    }

    // Deregister the live connection (paired with the fetch_add above).
    let remaining = state
        .active_connections
        .fetch_sub(1, Ordering::SeqCst)
        .saturating_sub(1);
    if remaining == 0 && state.shutdown_when_idle.swap(false, Ordering::SeqCst) {
        log::info!("Last client disconnected from UI-owned daemon; shutting down");
        super::shutdown::schedule_process_shutdown(&state);
    }

    // Release resize ownership so a reconnecting client isn't denied by a
    // dead owner; the next resize from any connection adopts it.
    okena_terminal::terminal::release_remote_resize_owner(&connection_owner_id);

    // Shutdown: dropping out_tx closes the writer's channel → writer exits.
    drop(out_tx);
    if !writer_finished {
        let _ = writer_handle.await;
    }
}

/// Deadline at which a connection's bearer token expires. `None` for the local
/// socket, which carries no token.
fn session_deadline(session: Option<&crate::auth::AuthSession>) -> Option<tokio::time::Instant> {
    let remaining = session?
        .expires_at
        .duration_since(std::time::SystemTime::now())
        .unwrap_or_default();
    Some(tokio::time::Instant::now() + remaining)
}

/// Never resolves without a deadline, so the select arm stays idle.
async fn sleep_until(deadline: Option<tokio::time::Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

/// Non-blocking: a revoked client that stopped reading must not be able to hold
/// the connection — and its `active_connections` slot — open.
fn send_auth_lost(out_tx: &mpsc::Sender<Message>) {
    let msg = serde_json::to_string(&WsOutbound::AuthFailed {
        error: "authentication no longer valid".into(),
    })
    .expect("BUG: WsOutbound must serialize");
    let _ = out_tx.try_send(Message::Text(msg.into()));
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

enum PostSnapshotDrain {
    Complete,
    Lagged(u64),
}

/// Drop only output already included in each snapshot and forward newer events.
async fn drain_post_snapshot(
    out_tx: &mpsc::Sender<Message>,
    pty_rx: &mut broadcast::Receiver<crate::pty_broadcaster::PtyBroadcastEvent>,
    subscribed_ids: &HashMap<String, u32>,
    sent: &SnapshotBatch,
    connection_owner_id: &str,
) -> Result<PostSnapshotDrain, ()> {
    use crate::pty_broadcaster::PtyBroadcastEvent;

    let mut batch: HashMap<u32, Vec<u8>> = HashMap::new();
    let mut resize_msgs: Vec<WsOutbound> = Vec::new();

    loop {
        match pty_rx.try_recv() {
            Ok(PtyBroadcastEvent::Output {
                terminal_id,
                data,
                sequence,
            }) => {
                // A skipped terminal gets a snapshot on its next output instead.
                let watermark = sent.watermarks.get(&terminal_id).copied().unwrap_or(0);
                if sequence > watermark
                    && !sent.skipped.contains(&terminal_id)
                    && let Some(&stream_id) = subscribed_ids.get(&terminal_id)
                {
                    batch.entry(stream_id).or_default().extend_from_slice(&data);
                }
            }
            Ok(PtyBroadcastEvent::Resized {
                terminal_id,
                cols,
                rows,
                server_owns,
                owner_connection_id,
            }) => {
                if subscribed_ids.contains_key(&terminal_id) {
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
            Err(broadcast::error::TryRecvError::Empty) => break,
            Err(broadcast::error::TryRecvError::Lagged(count)) => {
                return Ok(PostSnapshotDrain::Lagged(count));
            }
            Err(broadcast::error::TryRecvError::Closed) => return Err(()),
        }
    }

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
    Ok(PostSnapshotDrain::Complete)
}

/// Snapshots sent for a batch of terminals, and the terminals the daemon had
/// no snapshot for.
#[derive(Debug, Default, PartialEq)]
struct SnapshotBatch {
    watermarks: HashMap<String, u64>,
    skipped: Vec<String>,
}

impl SnapshotBatch {
    fn record(self, watermarks: &mut HashMap<String, u64>, unsnapshotted: &mut HashSet<String>) {
        for id in self.watermarks.keys() {
            unsnapshotted.remove(id);
        }
        watermarks.extend(self.watermarks);
        unsnapshotted.extend(self.skipped);
    }
}

/// Send snapshot frames for the given terminal IDs via the mpsc channel.
///
/// A terminal the daemon cannot snapshot is skipped on its own; only a closed
/// channel or bridge ends the connection.
async fn send_snapshots(
    out_tx: &mpsc::Sender<Message>,
    bridge_tx: &BridgeSender,
    broadcaster: &PtyBroadcaster,
    terminal_ids: &[String],
    subscribed_ids: &HashMap<String, u32>,
) -> Result<SnapshotBatch, ()> {
    let mut sent = SnapshotBatch::default();
    // One deadline for the batch, so stuck terminals do not wait in series.
    let mut deadline = tokio::time::Instant::now() + SNAPSHOT_CATCH_UP_DEADLINE;
    for id in terminal_ids {
        if let Some(&stream_id) = subscribed_ids.get(id) {
            let target_sequence = broadcaster.last_published_sequence(id);
            let Some((data, sequence)) =
                render_snapshot_after(bridge_tx, id, target_sequence, &mut deadline).await?
            else {
                sent.skipped.push(id.clone());
                continue;
            };
            let frame = build_binary_frame(FRAME_TYPE_SNAPSHOT, stream_id, &data);
            if out_tx.send(Message::Binary(frame.into())).await.is_err() {
                return Err(());
            }
            sent.watermarks.insert(id.clone(), sequence);
        }
    }
    Ok(sent)
}

/// Render a snapshot that includes output up to `target_sequence`.
///
/// Returns `Ok(None)` when the daemon has no snapshot for the terminal. A
/// model that keeps advancing pushes `deadline` out; one that stopped short of
/// the target (its output was published but never applied) gets its latest
/// snapshot returned once `deadline` passes, as the daemon's whole knowledge
/// of the terminal.
async fn render_snapshot_after(
    bridge_tx: &BridgeSender,
    terminal_id: &str,
    target_sequence: u64,
    deadline: &mut tokio::time::Instant,
) -> Result<Option<(Vec<u8>, u64)>, ()> {
    let mut last_sequence = None;
    loop {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        bridge_tx
            .send(BridgeMessage {
                command: RemoteCommand::RenderSnapshot {
                    terminal_id: terminal_id.to_string(),
                },
                reply: Some(reply_tx),
            })
            .await
            .map_err(|_| ())?;
        let (data, sequence) = match reply_rx.await.map_err(|_| ())? {
            CommandResult::OkSnapshot { data, sequence } => (data, sequence),
            other => {
                log::warn!("Terminal {terminal_id} has no snapshot, skipping it: {other:?}");
                return Ok(None);
            }
        };
        if sequence >= target_sequence {
            return Ok(Some((data, sequence)));
        }
        let now = tokio::time::Instant::now();
        if last_sequence.is_some_and(|last| sequence > last) {
            *deadline = (*deadline).max(now + SNAPSHOT_CATCH_UP_DEADLINE);
        }
        last_sequence = Some(sequence);
        if now >= *deadline {
            log::warn!(
                "Terminal {terminal_id} snapshot stopped at sequence {sequence}, expected {target_sequence}; sending it as is"
            );
            return Ok(Some((data, sequence)));
        }
        tokio::time::sleep(SNAPSHOT_CATCH_UP_POLL).await;
    }
}

async fn send_snapshots_and_reconcile(
    out_tx: &mpsc::Sender<Message>,
    state: &AppState,
    pty_rx: &mut broadcast::Receiver<crate::pty_broadcaster::PtyBroadcastEvent>,
    terminal_ids: &[String],
    subscribed_ids: &HashMap<String, u32>,
    connection_owner_id: &str,
) -> Result<SnapshotBatch, ()> {
    loop {
        let sent = send_snapshots(
            out_tx,
            &state.bridge_tx,
            &state.broadcaster,
            terminal_ids,
            subscribed_ids,
        )
        .await?;
        match drain_post_snapshot(out_tx, pty_rx, subscribed_ids, &sent, connection_owner_id)
            .await?
        {
            PostSnapshotDrain::Complete => return Ok(sent),
            PostSnapshotDrain::Lagged(count) => {
                let message = serde_json::to_string(&WsOutbound::Dropped { count })
                    .expect("BUG: WsOutbound must serialize");
                if out_tx.send(Message::Text(message.into())).await.is_err() {
                    return Err(());
                }
            }
        }
    }
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

    #[test]
    fn late_broadcasts_already_covered_by_snapshot_are_ignored() {
        let watermarks = HashMap::from([("terminal".to_string(), 42)]);
        assert!(!output_is_newer(&watermarks, "terminal", 41));
        assert!(!output_is_newer(&watermarks, "terminal", 42));
        assert!(output_is_newer(&watermarks, "terminal", 43));
    }

    fn drained_pty_payloads(out_rx: &mut mpsc::Receiver<Message>) -> Vec<(u32, Vec<u8>)> {
        let mut payloads = Vec::new();
        while let Ok(message) = out_rx.try_recv() {
            let Message::Binary(frame) = message else {
                panic!("expected PTY frame");
            };
            let (frame_type, stream_id, payload) = parse_binary_frame(&frame).unwrap();
            assert_eq!(frame_type, okena_core::ws::FRAME_TYPE_PTY);
            payloads.push((stream_id, payload.to_vec()));
        }
        payloads
    }

    #[tokio::test]
    async fn post_snapshot_drain_forwards_only_output_after_watermark() {
        let broadcaster = crate::pty_broadcaster::PtyBroadcaster::new();
        let mut pty_rx = broadcaster.subscribe();
        let covered = broadcaster.publish("terminal".to_string(), b"covered".to_vec());
        broadcaster.publish("terminal".to_string(), b"new".to_vec());

        let (out_tx, mut out_rx) = mpsc::channel(4);
        let subscribed = HashMap::from([("terminal".to_string(), 7)]);
        let sent = SnapshotBatch {
            watermarks: HashMap::from([("terminal".to_string(), covered)]),
            skipped: Vec::new(),
        };
        let outcome = drain_post_snapshot(&out_tx, &mut pty_rx, &subscribed, &sent, "connection")
            .await
            .unwrap();
        assert!(matches!(outcome, PostSnapshotDrain::Complete));

        assert_eq!(
            drained_pty_payloads(&mut out_rx),
            vec![(7, b"new".to_vec())]
        );
    }

    #[tokio::test]
    async fn post_snapshot_drain_holds_back_output_of_a_skipped_terminal() {
        let broadcaster = crate::pty_broadcaster::PtyBroadcaster::new();
        let mut pty_rx = broadcaster.subscribe();
        broadcaster.publish("skipped".to_string(), b"orphan".to_vec());
        broadcaster.publish("other".to_string(), b"live".to_vec());

        let (out_tx, mut out_rx) = mpsc::channel(4);
        let subscribed = HashMap::from([("skipped".to_string(), 1), ("other".to_string(), 2)]);
        let sent = SnapshotBatch {
            watermarks: HashMap::new(),
            skipped: vec!["skipped".to_string()],
        };
        drain_post_snapshot(&out_tx, &mut pty_rx, &subscribed, &sent, "connection")
            .await
            .unwrap();

        assert_eq!(
            drained_pty_payloads(&mut out_rx),
            vec![(2, b"live".to_vec())]
        );
    }

    #[test]
    fn a_later_snapshot_clears_a_skipped_terminal() {
        let mut watermarks = HashMap::new();
        let mut unsnapshotted = HashSet::new();
        SnapshotBatch {
            watermarks: HashMap::new(),
            skipped: vec!["t".to_string()],
        }
        .record(&mut watermarks, &mut unsnapshotted);
        assert!(unsnapshotted.contains("t"));

        SnapshotBatch {
            watermarks: HashMap::from([("t".to_string(), 5)]),
            skipped: Vec::new(),
        }
        .record(&mut watermarks, &mut unsnapshotted);
        assert!(unsnapshotted.is_empty());
        assert_eq!(watermarks, HashMap::from([("t".to_string(), 5)]));
    }

    /// Answers `RenderSnapshot` like the daemon, with `snapshot` deciding
    /// each terminal's reply.
    fn spawn_bridge(
        mut snapshot: impl FnMut(&str) -> CommandResult + Send + 'static,
    ) -> BridgeSender {
        let (bridge_tx, bridge_rx) = crate::bridge::bridge_channel();
        tokio::spawn(async move {
            while let Ok(message) = bridge_rx.recv().await {
                let RemoteCommand::RenderSnapshot { terminal_id } = message.command else {
                    continue;
                };
                let result = snapshot(&terminal_id);
                if let Some(reply) = message.reply {
                    let _ = reply.send(result);
                }
            }
        });
        bridge_tx
    }

    /// A snapshot at a fixed sequence for known terminals, an error for
    /// unknown ones.
    fn spawn_snapshot_bridge(terminals: HashMap<&'static str, u64>) -> BridgeSender {
        spawn_bridge(move |terminal_id| match terminals.get(terminal_id) {
            Some(&sequence) => CommandResult::OkSnapshot {
                data: terminal_id.as_bytes().to_vec(),
                sequence,
            },
            None => CommandResult::Err(format!("terminal not found: {terminal_id}")),
        })
    }

    fn snapshot_payloads(out_rx: &mut mpsc::Receiver<Message>) -> Vec<(u32, Vec<u8>)> {
        let mut payloads = Vec::new();
        while let Ok(message) = out_rx.try_recv() {
            let Message::Binary(frame) = message else {
                panic!("expected snapshot frame");
            };
            let (frame_type, stream_id, payload) = parse_binary_frame(&frame).unwrap();
            assert_eq!(frame_type, FRAME_TYPE_SNAPSHOT);
            payloads.push((stream_id, payload.to_vec()));
        }
        payloads
    }

    #[tokio::test]
    async fn a_terminal_without_snapshot_is_skipped_and_the_others_still_get_theirs() {
        let bridge_tx = spawn_snapshot_bridge(HashMap::from([("a", 0), ("c", 0)]));
        let broadcaster = PtyBroadcaster::new();
        let (out_tx, mut out_rx) = mpsc::channel(8);
        let ids = ["a", "missing", "c"].map(String::from);
        let subscribed = HashMap::from([
            ("a".to_string(), 1),
            ("missing".to_string(), 2),
            ("c".to_string(), 3),
        ]);

        let sent = send_snapshots(&out_tx, &bridge_tx, &broadcaster, &ids, &subscribed)
            .await
            .expect("one terminal's failure must not end the connection");

        assert_eq!(
            snapshot_payloads(&mut out_rx),
            vec![(1, b"a".to_vec()), (3, b"c".to_vec())]
        );
        assert_eq!(
            sent,
            SnapshotBatch {
                watermarks: HashMap::from([("a".to_string(), 0), ("c".to_string(), 0)]),
                skipped: vec!["missing".to_string()],
            }
        );
    }

    #[tokio::test(start_paused = true)]
    async fn stuck_terminals_share_one_deadline() {
        // The broadcaster published output the terminal models never applied.
        let broadcaster = PtyBroadcaster::new();
        let published = broadcaster.publish("stuck-a".to_string(), b"lost".to_vec());
        broadcaster.publish("stuck-b".to_string(), b"lost".to_vec());
        let bridge_tx = spawn_snapshot_bridge(HashMap::from([("stuck-a", 0), ("stuck-b", 0)]));
        let (out_tx, mut out_rx) = mpsc::channel(8);
        let ids = ["stuck-a", "stuck-b"].map(String::from);
        let subscribed = HashMap::from([("stuck-a".to_string(), 4), ("stuck-b".to_string(), 5)]);
        let start = tokio::time::Instant::now();

        let sent = send_snapshots(&out_tx, &bridge_tx, &broadcaster, &ids, &subscribed)
            .await
            .expect("a lagging terminal must not end the connection");

        assert!(published > 0);
        assert!(start.elapsed() < SNAPSHOT_CATCH_UP_DEADLINE + SNAPSHOT_CATCH_UP_POLL * 2);
        assert_eq!(
            snapshot_payloads(&mut out_rx),
            vec![(4, b"stuck-a".to_vec()), (5, b"stuck-b".to_vec())]
        );
        assert_eq!(
            sent.watermarks,
            HashMap::from([("stuck-a".to_string(), 0), ("stuck-b".to_string(), 0)])
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_model_that_keeps_advancing_is_waited_for_past_the_deadline() {
        let broadcaster = PtyBroadcaster::new();
        let mut target = 0;
        for _ in 0..400 {
            target = broadcaster.publish("slow".to_string(), b"x".to_vec());
        }
        // The model applies one more event per snapshot request.
        let mut applied = 0;
        let bridge_tx = spawn_bridge(move |terminal_id| {
            applied += 1;
            CommandResult::OkSnapshot {
                data: terminal_id.as_bytes().to_vec(),
                sequence: applied,
            }
        });
        let (out_tx, _out_rx) = mpsc::channel(8);
        let ids = ["slow".to_string()];
        let subscribed = HashMap::from([("slow".to_string(), 1)]);
        let start = tokio::time::Instant::now();

        let sent = send_snapshots(&out_tx, &bridge_tx, &broadcaster, &ids, &subscribed)
            .await
            .unwrap();

        assert!(start.elapsed() > SNAPSHOT_CATCH_UP_DEADLINE);
        assert_eq!(
            sent.watermarks,
            HashMap::from([("slow".to_string(), target)])
        );
    }

    #[tokio::test]
    async fn a_closed_bridge_still_ends_the_connection() {
        let (bridge_tx, bridge_rx) = crate::bridge::bridge_channel();
        drop(bridge_rx);
        let broadcaster = PtyBroadcaster::new();
        let (out_tx, _out_rx) = mpsc::channel(8);
        let ids = ["a".to_string()];
        let subscribed = HashMap::from([("a".to_string(), 1)]);

        assert!(
            send_snapshots(&out_tx, &bridge_tx, &broadcaster, &ids, &subscribed)
                .await
                .is_err()
        );
    }
}
