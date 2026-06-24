//! GPUI-free PTY event loop: the headless analogue of the GUI's batched
//! `async_channel` drain in `okena-app`'s `app/mod.rs` / `app/headless.rs`.
//!
//! The GUI reads [`PtyEvent`]s off the [`PtyManager`]'s channel on the GPUI
//! thread, feeds `Data` into the per-terminal `process_output`, and on `Exit`
//! cleans up the PTY handle and lets the [`ServiceManager`] decide whether the
//! terminal was a service (so it can restart it or keep its crash output). The
//! daemon does the same against `Arc<parking_lot::Mutex<…>>` state and a tokio
//! task — dropping only the GUI-only bits (window/pane notify, hook monitor,
//! soft-close toast handling, which the daemon has no surface for).
//!
//! ## Runs inside the [`LocalSet`](tokio::task::LocalSet)
//!
//! [`run_pty_loop`] MUST be driven by `spawn_local` (or directly inside a
//! running `LocalSet`): on `Exit` it calls
//! [`ServiceManager::handle_service_exit`](okena_services::manager::ServiceManager::handle_service_exit),
//! which for a crashed-but-restart service calls
//! [`ServiceCx::spawn_main`](okena_services::manager::ServiceCx::spawn_main) —
//! and the daemon's `spawn_main` is `tokio::task::spawn_local`, which panics
//! outside a `LocalSet`. This is the same constraint the observer tasks document
//! (see [`crate::observers`]). The blocking subprocess offloads still reach the
//! multi-thread pool via the held [`Handle`](tokio::runtime::Handle).

use std::collections::HashSet;
use std::sync::Arc;

use async_channel::Receiver;
use okena_services::manager::ServiceManager;
use okena_terminal::pty_manager::{PtyEvent, PtyManager};
use okena_terminal::TerminalsRegistry;
use parking_lot::Mutex;
use tokio::runtime::Handle;
use tokio::sync::watch;

use crate::service_cx::ServiceReactorRef;

/// Per-turn work budget. A single high-bandwidth terminal (`cat hugefile`,
/// `yes`, a runaway build log) can otherwise keep this loop draining the channel
/// forever, starving the other tasks sharing the LocalSet thread. Once we've
/// parsed this many bytes in one drain pass we stop and yield back to the
/// executor; the remaining events stay in the bounded channel and are picked up
/// next turn (nothing is dropped). Mirrors the GUI's `MAX_BYTES_PER_TURN`.
const MAX_BYTES_PER_TURN: usize = 256 * 1024;

/// Run the daemon PTY event loop until the channel closes (all PTY senders
/// dropped, i.e. shutdown).
///
/// Dependencies are passed individually so `DaemonCore::new` wires them
/// explicitly:
/// * `pty_events` — the [`Receiver<PtyEvent>`] returned by [`PtyManager::new`].
/// * `terminals` — the shared [`TerminalsRegistry`]; `Data` events look up the
///   `Arc<Terminal>` here and feed `process_output`.
/// * `pty_manager` — for `cleanup_exited` (reap reader/writer threads on EOF)
///   and `kill` (SIGTERM the lingering session for non-service terminals).
/// * `service_manager` + `runtime` + `service_tick` — the same triple
///   [`ServiceReactorRef`] needs to mint a `DaemonServiceCx` so
///   `handle_service_exit` can `notify`/`spawn_main` (the service restart path).
/// * `state_version` — bumped once per batch that contained exits, mirroring the
///   GUI headless loop's `state_version` bump (drives the snapshot/broadcast
///   observer).
#[allow(clippy::too_many_arguments)]
pub async fn run_pty_loop(
    pty_events: Receiver<PtyEvent>,
    terminals: TerminalsRegistry,
    pty_manager: Arc<PtyManager>,
    service_manager: Arc<Mutex<ServiceManager>>,
    runtime: Handle,
    service_tick: watch::Sender<u64>,
    state_version: watch::Sender<u64>,
) {
    // The reactor bits needed to build a top-level `DaemonServiceCx` for
    // `handle_service_exit`. Built once; `cx()` is re-borrowed per exit batch.
    // It re-locks `service_manager` internally on reentry, so the loop locks the
    // manager itself (below) only while the cx is alive — never across an await.
    let reactor_ref = ServiceReactorRef::new(service_manager.clone(), runtime, service_tick);

    loop {
        // Block until at least one event arrives. `Err` means every sender was
        // dropped — the PtyManager is gone, so the loop is done.
        let event = match pty_events.recv().await {
            Ok(event) => event,
            Err(_) => break,
        };

        // Exits collected across this drain pass, handled together after.
        let mut exit_events: Vec<(String, Option<u32>)> = Vec::new();
        // Bytes parsed so far in this pass (across batched `Data` events).
        let mut bytes_this_turn: usize = 0;

        process_event(&event, &terminals, &pty_manager, &mut exit_events, &mut bytes_this_turn);

        // Drain additional pending events (batch processing), stopping once we
        // exceed the per-turn byte budget so we yield instead of monopolizing
        // the LocalSet thread.
        while bytes_this_turn < MAX_BYTES_PER_TURN {
            let event = match pty_events.try_recv() {
                Ok(event) => event,
                Err(_) => break,
            };
            process_event(&event, &terminals, &pty_manager, &mut exit_events, &mut bytes_this_turn);
        }

        if !exit_events.is_empty() {
            handle_exits(&exit_events, &terminals, &pty_manager, &service_manager, &reactor_ref);
            // Coarse "something changed" tick, mirroring the GUI headless loop.
            state_version.send_modify(|v| *v += 1);
        }
    }
}

/// Handle a single [`PtyEvent`]: feed `Data` into the terminal (dropping the
/// registry lock before the parse, as the GUI does), or reap + record `Exit`.
fn process_event(
    event: &PtyEvent,
    terminals: &TerminalsRegistry,
    pty_manager: &PtyManager,
    exit_events: &mut Vec<(String, Option<u32>)>,
    bytes_this_turn: &mut usize,
) {
    match event {
        PtyEvent::Data { terminal_id, data } => {
            // Hold the registry lock only for the HashMap lookup — clone the
            // `Arc<Terminal>` out and drop the guard before the (potentially
            // long) ANSI parse, so input/resize/kill on OTHER terminals don't
            // block behind it.
            let term = terminals.lock().get(terminal_id).cloned();
            if let Some(term) = term {
                *bytes_this_turn += data.len();
                term.process_output(data);
            }
        }
        PtyEvent::Exit { terminal_id, exit_code } => {
            // Clean up the PtyHandle (reader/writer threads) but don't remove
            // the Terminal yet — the service manager may keep it so users can
            // see crash output.
            pty_manager.cleanup_exited(terminal_id);
            exit_events.push((terminal_id.clone(), *exit_code));
        }
    }
}

/// Handle the exits collected in one batch: let the service manager claim its
/// service terminals (restart / keep-crash-output), then kill + remove the UI
/// Terminal for every non-service terminal. Mirrors the GUI headless loop's
/// exit handling minus the GUI-only hook-monitor / soft-close work.
fn handle_exits(
    exit_events: &[(String, Option<u32>)],
    terminals: &TerminalsRegistry,
    pty_manager: &PtyManager,
    service_manager: &Arc<Mutex<ServiceManager>>,
    reactor_ref: &ServiceReactorRef,
) {
    // Let the service manager handle service terminals. For a crashed service
    // with `restart_on_crash`, `handle_service_exit` calls `spawn_main` (lands
    // on this LocalSet) to restart after a delay; otherwise it marks the service
    // crashed and keeps the Terminal so the crash output stays visible.
    let service_tids: HashSet<String> = {
        let mut sm = service_manager.lock();
        let mut cx = reactor_ref.cx();
        let mut handled = HashSet::new();
        for (terminal_id, exit_code) in exit_events {
            if sm.handle_service_exit(terminal_id, *exit_code, &mut cx) {
                handled.insert(terminal_id.clone());
            }
        }
        handled
    };

    // Kill session backends and remove UI Terminals for non-service terminals.
    // `kill` is critical for dtach: the PTY exit only means the client
    // disconnected, but the dtach daemon keeps running — `kill` SIGTERMs it and
    // removes the socket file.
    let mut reg = terminals.lock();
    for (terminal_id, _) in exit_events {
        if !service_tids.contains(terminal_id) {
            pty_manager.kill(terminal_id);
            reg.remove(terminal_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use okena_terminal::backend::LocalBackend;
    use okena_terminal::session_backend::SessionBackend;
    use okena_terminal::terminal::{Terminal, TerminalSize};

    fn terminal_size() -> TerminalSize {
        TerminalSize {
            cols: 80,
            rows: 24,
            cell_width: 8.0,
            cell_height: 16.0,
        }
    }

    /// `run_pty_loop` routes a synthesized `Data` event into a registered
    /// terminal: the terminal's `content_generation` advances, proving the
    /// bytes reached `process_output`. Exercises the recv → registry-lookup →
    /// `process_output` path (no exits) on a `LocalSet`, and that the loop
    /// exits cleanly once every sender is dropped.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_pty_loop_processes_data_into_registered_terminal() {
        // Our own event channel — `run_pty_loop` consumes the receiver; we keep
        // the sender to inject one event and then drop it so the loop ends.
        let (tx, pty_events) = async_channel::bounded::<PtyEvent>(16);

        // A real `PtyManager` (no terminals spawned) provides the
        // `TerminalTransport` for the test terminal and the `cleanup_exited` /
        // `kill` no-ops; its own internal channel is unused here.
        let (pty_manager, _pty_manager_events) = PtyManager::new(SessionBackend::None);
        let pty_manager = Arc::new(pty_manager);

        let terminals: TerminalsRegistry = Arc::new(parking_lot::Mutex::new(Default::default()));
        let transport = pty_manager.clone(); // PtyManager: TerminalTransport
        let term = Arc::new(Terminal::new(
            "t1".to_string(),
            terminal_size(),
            transport,
            "/tmp".to_string(),
        ));
        let gen_before = term.content_generation();
        terminals.lock().insert("t1".to_string(), term.clone());

        let backend = Arc::new(LocalBackend::new(pty_manager.clone()));
        let service_manager = Arc::new(Mutex::new(ServiceManager::new(backend, terminals.clone())));

        let (service_tick, _srx) = watch::channel(0u64);
        let (state_version, _vrx) = watch::channel(0u64);

        let local = tokio::task::LocalSet::new();
        local
            .run_until(async move {
                let handle = tokio::task::spawn_local(run_pty_loop(
                    pty_events,
                    terminals.clone(),
                    pty_manager.clone(),
                    service_manager.clone(),
                    Handle::current(),
                    service_tick,
                    state_version,
                ));

                tx.send(PtyEvent::Data {
                    terminal_id: "t1".to_string(),
                    data: b"hello".to_vec(),
                })
                .await
                .expect("send synthesized data event");

                // Drop the only sender so `recv` returns `Err`, ending the loop.
                drop(tx);

                handle.await.expect("pty loop task joins");
            })
            .await;

        // `process_output` bumped the content generation → the data was routed
        // into the registered terminal.
        assert!(
            term.content_generation() > gen_before,
            "process_output should have advanced content_generation (before={gen_before}, after={})",
            term.content_generation(),
        );
    }
}
