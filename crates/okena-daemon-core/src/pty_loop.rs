//! GPUI-free PTY event loop: the headless analogue of the GUI's batched
//! `async_channel` drain previously hosted by `okena-app`.
//!
//! The GUI reads [`PtyEvent`]s off the [`PtyManager`]'s channel on the GPUI
//! thread, feeds `Data` into the per-terminal `process_output`, and on `Exit`
//! cleans up the PTY handle and lets the [`ServiceManager`] decide whether the
//! terminal was a service (so it can restart it or keep its crash output). The
//! daemon does the same against `Arc<parking_lot::Mutex<…>>` state and a tokio
//! task — but, unlike a thin GUI client, the daemon OWNS the workspace, hooks,
//! and lifecycle state, so it also runs the full terminal-exit lifecycle:
//!
//! * hook-terminal exits → status updates + pending worktree-close resolution
//!   (deleting the worktree project DIRECTLY in the workspace — the GUI client
//!   instead dispatched a remote `DeleteProject`),
//! * `terminal.on_close` hooks for plain user terminals,
//! * hook-exit-via-OSC-title (`__okena_hook_exit:<code>`),
//! * stale soft-close-record cleanup.
//!
//! The only GUI-only bits dropped are the ones with no daemon surface: window /
//! pane notify and soft-close *toast* dismissal (the daemon still does the
//! soft-close workspace-state cleanup, just without the UI toast).
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
use okena_hooks::{HookMonitor, HookRunner};
use okena_services::manager::ServiceManager;
use okena_terminal::TerminalsRegistry;
use okena_terminal::backend::TerminalBackend;
use okena_terminal::pty_manager::{PtyEvent, PtyGeneration, PtyManager};
use okena_workspace::context::WorkspaceCx;
use okena_workspace::persistence::AppSettings;
use okena_workspace::state::{HookTerminalStatus, Workspace};
use parking_lot::Mutex;
use tokio::runtime::Handle;
use tokio::sync::watch;

use crate::service_cx::ServiceReactorRef;
use crate::workspace_cx::DaemonWorkspaceCx;

/// Per-turn work budget. A single high-bandwidth terminal (`cat hugefile`,
/// `yes`, a runaway build log) can otherwise keep this loop draining the channel
/// forever, starving the other tasks sharing the LocalSet thread. Once we've
/// parsed this many bytes in one drain pass we stop and yield back to the
/// executor; the remaining events stay in the bounded channel and are picked up
/// next turn (nothing is dropped). Mirrors the GUI's `MAX_BYTES_PER_TURN`.
const MAX_BYTES_PER_TURN: usize = 256 * 1024;

/// The shared reactor handles the PTY loop needs to run terminal-exit lifecycle
/// work directly against the daemon-owned workspace + hooks. Bundled so the loop
/// signature (and the per-batch handlers it calls) stay readable.
///
/// Everything here is cheaply clonable (`Arc<Mutex<…>>`, `watch::Sender`, the
/// `Arc`-backed hook services) — the loop holds it for its whole lifetime and
/// re-borrows per batch.
pub struct PtyLoopReactor {
    /// The daemon-owned workspace: hook-terminal status, pending worktree close,
    /// soft-close records, and project deletion all mutate it directly.
    pub workspace: Arc<Mutex<Workspace>>,
    /// Terminal backend used to stop and restore project terminals around a
    /// background worktree removal.
    pub backend: Arc<dyn TerminalBackend>,
    /// Hook runner — threaded into `DaemonWorkspaceCx` so workspace mutators that
    /// need it (e.g. project deletion firing lifecycle hooks) can reach it.
    pub hook_runner: Option<HookRunner>,
    /// Hook monitor — `notify_exit` / `finish_by_terminal_id` updates and the
    /// `terminal.on_close` hook run reach it directly.
    pub hook_monitor: Option<HookMonitor>,
    /// Bumped by `DaemonWorkspaceCx::notify` on each workspace mutation.
    pub workspace_tick: watch::Sender<u64>,
    /// App settings (read for the global `terminal.on_close` hook + the
    /// global-hooks arg passed into project deletion / hook firing).
    pub settings: Arc<Mutex<AppSettings>>,
}

impl PtyLoopReactor {
    /// Build a fresh [`DaemonWorkspaceCx`] borrowing this reactor's notify channel
    /// + hook services, for a single workspace mutation site.
    fn workspace_cx(&self) -> DaemonWorkspaceCx<'_> {
        DaemonWorkspaceCx::new(&self.workspace_tick, &self.hook_runner, &self.hook_monitor)
    }
}

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
/// * `reactor` — the daemon-owned workspace + hook handles the lifecycle work
///   (hook-terminal exits, `terminal.on_close`, OSC hook-exit, soft-close reap)
///   mutates directly.
/// * `state_version` — bumped once per batch that contained exits, so clients
///   resync after the lifecycle mutations.
#[allow(clippy::too_many_arguments)]
pub async fn run_pty_loop(
    pty_events: Receiver<PtyEvent>,
    terminals: TerminalsRegistry,
    pty_manager: Arc<PtyManager>,
    service_manager: Arc<Mutex<ServiceManager>>,
    runtime: Handle,
    service_tick: watch::Sender<u64>,
    reactor: PtyLoopReactor,
    state_version: watch::Sender<u64>,
) {
    // The reactor bits needed to build a top-level `DaemonServiceCx` for
    // `handle_service_exit`. Built once; `cx()` is re-borrowed per exit batch.
    // It re-locks `service_manager` internally on reentry, so the loop locks the
    // manager itself (below) only while the cx is alive — never across an await.
    let reactor_ref = ServiceReactorRef::new(
        service_manager.clone(),
        runtime.clone(),
        service_tick.clone(),
    );

    loop {
        // Block until at least one event arrives. `Err` means every sender was
        // dropped — the PtyManager is gone, so the loop is done.
        let event = match pty_events.recv().await {
            Ok(event) => event,
            Err(_) => break,
        };

        // Exits collected across this drain pass, handled together after.
        let mut exit_events: Vec<(String, PtyGeneration, Option<u32>)> = Vec::new();
        // Terminals that produced output this pass (for the OSC hook-exit title
        // check, mirroring the GUI's `dirty_terminal_ids`).
        let mut dirty_terminal_ids: Vec<String> = Vec::new();
        // Bytes parsed so far in this pass (across batched `Data` events).
        let mut bytes_this_turn: usize = 0;

        process_event(
            &event,
            &terminals,
            &pty_manager,
            &mut exit_events,
            &mut dirty_terminal_ids,
            &mut bytes_this_turn,
        );

        // Drain additional pending events (batch processing), stopping once we
        // exceed the per-turn byte budget so we yield instead of monopolizing
        // the LocalSet thread.
        while bytes_this_turn < MAX_BYTES_PER_TURN {
            let event = match pty_events.try_recv() {
                Ok(event) => event,
                Err(_) => break,
            };
            process_event(
                &event,
                &terminals,
                &pty_manager,
                &mut exit_events,
                &mut dirty_terminal_ids,
                &mut bytes_this_turn,
            );
        }

        // Hook terminals can report their exit code via an OSC title
        // (`__okena_hook_exit:<code>`) while the interactive shell stays alive —
        // independent of any PTY `Exit`. Mirror the GUI's post-batch dirty-title
        // scan. (Runs whether or not there were exits.)
        if !dirty_terminal_ids.is_empty() {
            let osc_hook_exits = process_osc_hook_exits(&dirty_terminal_ids, &terminals, &reactor);
            if !osc_hook_exits.is_empty() {
                resolve_osc_worktree_closes(
                    &osc_hook_exits,
                    &terminals,
                    &pty_manager,
                    &service_manager,
                    &service_tick,
                    &runtime,
                    &reactor,
                );
                // The workspace tick carries the authoritative mutation to the
                // state observer, but bump the coarse version here too so a
                // client resync is not delayed behind another PTY event.
                state_version.send_modify(|v| *v += 1);
            }
            // Activity edges — OSC 133 ;D command-finish, bell, and OSC 9/777
            // notification — stamp `last_activity_at` on the owning project so the
            // activity-sorted sidebar floats it up. Bump `state_version` if
            // anything was stamped so clients resync (the bump is mirrored into
            // StateResponse). Stamping on the daemon — not the client mirror,
            // which the next sync overwrites — is what makes bell/notification
            // recency persist and reach every client.
            if process_activity_edges(&dirty_terminal_ids, &terminals, &reactor) {
                state_version.send_modify(|v| *v += 1);
            }

            // Agent status is runtime-only, so its dirty edge must advance the
            // daemon's state version explicitly for remote clients.
            if drain_remote_dirty(&dirty_terminal_ids, &terminals) {
                state_version.send_modify(|v| *v += 1);
            }

            // Agent sessions are sticky on Terminal but persistent in the
            // workspace. The workspace tick observer handles the durable save
            // and subsequent state-version bump.
            persist_agent_sessions(&dirty_terminal_ids, &terminals, &reactor);
        }

        if !exit_events.is_empty() {
            let context = ExitHandlingContext {
                terminals: &terminals,
                pty_manager: pty_manager.as_ref(),
                service_manager: &service_manager,
                reactor_ref: &reactor_ref,
                service_tick: &service_tick,
                runtime: &runtime,
                reactor: &reactor,
            };
            handle_exits(&exit_events, &context);
            // Coarse "something changed" tick: the lifecycle mutations above
            // (hook status, project deletion, soft-close cleanup) are now visible
            // to clients on their next resync.
            state_version.send_modify(|v| *v += 1);
        }
    }
}

/// Handle a single [`PtyEvent`]: feed `Data` into the terminal (dropping the
/// registry lock before the parse, as the GUI does) and record it dirty, or
/// reap + record `Exit`.
fn process_event(
    event: &PtyEvent,
    terminals: &TerminalsRegistry,
    pty_manager: &PtyManager,
    exit_events: &mut Vec<(String, PtyGeneration, Option<u32>)>,
    dirty_terminal_ids: &mut Vec<String>,
    bytes_this_turn: &mut usize,
) {
    match event {
        PtyEvent::Data {
            terminal_id,
            generation,
            data,
            sequence,
        } => {
            if !pty_manager.is_current_generation(terminal_id, *generation) {
                return;
            }
            // Hold the registry lock only for the HashMap lookup — clone the
            // `Arc<Terminal>` out and drop the guard before the (potentially
            // long) ANSI parse, so input/resize/kill on OTHER terminals don't
            // block behind it.
            let term = terminals.lock().get(terminal_id).cloned();
            if let Some(term) = term {
                *bytes_this_turn += data.len();
                term.process_output_with_sequence(data, *sequence);
            }
            dirty_terminal_ids.push(terminal_id.clone());
        }
        PtyEvent::Exit {
            terminal_id,
            generation,
            exit_code,
        } => {
            // Clean up the PtyHandle (reader/writer threads) but don't remove
            // the Terminal yet — the service manager may keep it so users can
            // see crash output.
            if pty_manager.cleanup_exited(terminal_id, *generation) {
                exit_events.push((terminal_id.clone(), *generation, *exit_code));
            }
        }
    }
}

/// Hook-exit-via-OSC-title: for any terminal that produced output this batch and
/// IS a hook terminal, if its title is `__okena_hook_exit:<code>`, set the hook
/// status and HookMonitor execution to Succeeded (code 0) / Failed otherwise,
/// and return its authoritative result for pending worktree-close resolution.
///
/// This happens for keep-alive hooks whose command finished but whose PTY stays
/// alive as an interactive shell, so there is no PTY `Exit` to drive completion.
fn process_osc_hook_exits(
    dirty_terminal_ids: &[String],
    terminals: &TerminalsRegistry,
    reactor: &PtyLoopReactor,
) -> Vec<(String, i32)> {
    // Collect status updates under the registry + workspace read locks, then
    // apply them under a single workspace write lock (matching the GUI's split).
    let mut status_updates: Vec<(String, HookTerminalStatus, Option<u32>)> = Vec::new();
    {
        let terminals_guard = terminals.lock();
        let ws = reactor.workspace.lock();
        for tid in dirty_terminal_ids {
            if ws.is_hook_terminal(tid).is_none() {
                continue;
            }
            if let Some(terminal) = terminals_guard.get(tid)
                && let Some(title) = terminal.title()
                && let Some(code_str) = title.strip_prefix("__okena_hook_exit:")
            {
                let exit_code = code_str.parse::<i32>().unwrap_or(-1);
                let status = if exit_code == 0 {
                    HookTerminalStatus::Succeeded
                } else {
                    HookTerminalStatus::Failed { exit_code }
                };
                status_updates.push((tid.clone(), status, u32::try_from(exit_code).ok()));
            }
        }
    }
    let mut results = Vec::with_capacity(status_updates.len());
    if !status_updates.is_empty() {
        let mut cx = reactor.workspace_cx();
        let mut ws = reactor.workspace.lock();
        for (tid, status, exit_code) in status_updates {
            if let Some(monitor) = reactor.hook_monitor.as_ref() {
                monitor.finish_by_terminal_id(&tid, exit_code);
            }
            let code = match &status {
                HookTerminalStatus::Succeeded => 0,
                HookTerminalStatus::Failed { exit_code } => *exit_code,
                HookTerminalStatus::Running => unreachable!("OSC produces a completed hook status"),
            };
            ws.update_hook_terminal_status(&tid, status, &mut cx);
            results.push((tid, code));
        }
    }
    results
}

/// Resolve before-remove hooks that reported an authoritative result through
/// OSC while their PTY remains alive. The pending map is the exactly-once claim:
/// a late PTY Exit or repeated title observes no pending entry and cannot delete
/// a project or overwrite the completed hook state.
#[allow(clippy::too_many_arguments)]
fn resolve_osc_worktree_closes(
    osc_hook_exits: &[(String, i32)],
    terminals: &TerminalsRegistry,
    pty_manager: &PtyManager,
    service_manager: &Arc<Mutex<ServiceManager>>,
    service_tick: &watch::Sender<u64>,
    runtime: &Handle,
    reactor: &PtyLoopReactor,
) {
    let global_hooks = reactor.settings.lock().hooks.clone();
    for (terminal_id, exit_code) in osc_hook_exits {
        let mut cx = reactor.workspace_cx();
        let mut ws = reactor.workspace.lock();
        let Some(pending) = ws.take_pending_worktree_close(terminal_id) else {
            continue;
        };

        if *exit_code == 0 {
            // A keep-alive shell can retain the worktree CWD after reporting
            // success. Tear down its PTY before starting the canonical removal;
            // unlike the watchdog this result is authoritative, not inferred.
            ws.remove_hook_terminal(terminal_id, &mut cx);
            match ws.begin_worktree_removal(&pending.project_id, &global_hooks, &mut cx) {
                Ok(plan) => {
                    let operation_epoch = ws.data_replacement_epoch();
                    drop(ws);
                    pty_manager.kill(terminal_id);
                    terminals.lock().remove(terminal_id);
                    let _ = crate::command_loop::spawn_background_worktree_removal(
                        plan,
                        operation_epoch,
                        false,
                        std::slice::from_ref(terminal_id),
                        &global_hooks,
                        &reactor.workspace,
                        &reactor.workspace_tick,
                        &reactor.hook_runner,
                        &reactor.hook_monitor,
                        &reactor.backend,
                        terminals,
                        &reactor.settings,
                        service_manager,
                        service_tick,
                        runtime,
                    );
                }
                Err(error) => {
                    let project_name = ws
                        .project(&pending.project_id)
                        .map(|project| project.name.clone())
                        .unwrap_or_else(|| pending.project_id.clone());
                    ws.finish_closing_project(&pending.project_id);
                    cx.notify();
                    if let Some(monitor) = reactor.hook_monitor.as_ref() {
                        monitor.push_toast(okena_state::Toast::error(format!(
                            "\"{project_name}\" was not closed: {error}"
                        )));
                    }
                }
            }
        } else {
            ws.finish_closing_project(&pending.project_id);
            cx.notify();
            // `process_osc_hook_exits` already completed the HookMonitor with
            // this authoritative nonzero code, which queues its single failure
            // toast. Do not enqueue a second toast for the same hook result.
        }
    }
}

/// Drain the one-shot activity edges — command-finished (OSC 133 ;D), bell, and
/// desktop-notification (OSC 9/777) — for each terminal that produced output this
/// batch and stamp `last_activity_at` on the owning project (drives the
/// activity-sorted sidebar). Returns `true` if any activity was stamped, so the
/// caller can bump `state_version` for clients to resync.
///
/// Mirrors the GUI's activity bump: drain the cheap atomic edges first (almost
/// every batch drains nothing), resolve the active terminals to their owning
/// projects (deduplicated), then `bump_activity` once per project. The daemon's
/// own `Terminal`s parse all three signals in `process_output`, so the edges are
/// available here — the client's bell/notification bump was on its read-only
/// mirror and lost on the next sync.
fn process_activity_edges(
    dirty_terminal_ids: &[String],
    terminals: &TerminalsRegistry,
    reactor: &PtyLoopReactor,
) -> bool {
    // Drain edges first (cheap atomic swaps); collect the terminals that saw any
    // meaningful activity. The lock is dropped before touching the workspace.
    //
    // Drain ALL THREE edges with `|=` (never `||`): each is a one-shot that must
    // be consumed to clear it, so short-circuiting would leak the notification
    // queue (and miss a bell that follows a command-finish in the same batch).
    let active: Vec<String> = {
        let reg = terminals.lock();
        dirty_terminal_ids
            .iter()
            .filter(|tid| {
                reg.get(*tid).is_some_and(|t| {
                    let mut a = t.take_pending_command_finished();
                    a |= t.take_pending_bell();
                    a |= !t.take_pending_notifications().is_empty();
                    a
                })
            })
            .cloned()
            .collect()
    };
    if active.is_empty() {
        return false;
    }

    // Resolve each active terminal to its owning project, deduplicating so a
    // batch touching several terminals of the same project bumps it once.
    let project_ids: HashSet<String> = {
        let ws = reactor.workspace.lock();
        active
            .iter()
            .filter_map(|tid| ws.find_project_for_terminal(tid).map(|p| p.id.clone()))
            .collect()
    };
    if project_ids.is_empty() {
        return false;
    }

    let mut cx = reactor.workspace_cx();
    let mut ws = reactor.workspace.lock();
    for pid in &project_ids {
        ws.bump_activity(pid, &mut cx);
    }
    true
}

/// Consume runtime-only terminal changes that remote clients must observe.
fn drain_remote_dirty(dirty_terminal_ids: &[String], terminals: &TerminalsRegistry) -> bool {
    let registry = terminals.lock();
    let mut changed = false;
    for terminal_id in dirty_terminal_ids {
        if registry
            .get(terminal_id)
            .is_some_and(|terminal| terminal.take_remote_dirty())
        {
            changed = true;
        }
    }
    changed
}

/// Persist agent sessions captured by the terminal OSC sidecar.
fn persist_agent_sessions(
    dirty_terminal_ids: &[String],
    terminals: &TerminalsRegistry,
    reactor: &PtyLoopReactor,
) {
    let captured: Vec<(String, okena_core::agent_session::AgentSession)> = {
        let registry = terminals.lock();
        dirty_terminal_ids
            .iter()
            .filter_map(|terminal_id| {
                let terminal = registry.get(terminal_id)?;
                if !terminal.take_agent_session_dirty() {
                    return None;
                }
                terminal
                    .agent_session()
                    .map(|session| (terminal_id.clone(), session))
            })
            .collect()
    };
    if captured.is_empty() {
        return;
    }

    let mut cx = reactor.workspace_cx();
    let mut workspace = reactor.workspace.lock();
    for (terminal_id, session) in captured {
        if let Some(project_id) = workspace
            .find_project_for_terminal(&terminal_id)
            .map(|project| project.id.clone())
        {
            workspace.set_agent_session(&project_id, &terminal_id, session, &mut cx);
        }
    }
}

struct ExitHandlingContext<'a> {
    terminals: &'a TerminalsRegistry,
    pty_manager: &'a PtyManager,
    service_manager: &'a Arc<Mutex<ServiceManager>>,
    reactor_ref: &'a ServiceReactorRef,
    service_tick: &'a watch::Sender<u64>,
    runtime: &'a Handle,
    reactor: &'a PtyLoopReactor,
}

/// Handle the exits collected in one batch:
/// 1. Let the service manager claim its service terminals (restart /
///    keep-crash-output) — yields the `service_tids` set.
/// 2. Resolve hook-terminal exits: notify the monitor, set hook status, and
///    resolve any pending worktree close (run the canonical worktree removal
///    DIRECTLY in the daemon workspace on success; finish-closing on failure) —
///    yields the `hook_tids` set.
/// 3. Fire `terminal.on_close` for plain user terminals (non-service, non-hook).
/// 4. Kill + remove the UI Terminal for every non-service, non-hook terminal.
/// 5. Drop stale soft-close records for any exited terminal.
///
/// Mirrors the GUI's PTY-exit handling, adapted: the GUI is a thin client and
/// dispatched a remote action + ran the worktree removal locally; the daemon owns
/// the workspace and runs the canonical removal directly. The GUI-only
/// window/pane notify + toast dismissal have no daemon surface and are dropped
/// (the soft-close *state* cleanup still runs).
fn handle_exits(
    exit_events: &[(String, PtyGeneration, Option<u32>)],
    context: &ExitHandlingContext<'_>,
) {
    // ── 1. Service terminals ────────────────────────────────────────────────
    // For a crashed service with `restart_on_crash`, `handle_service_exit` calls
    // `spawn_main` (lands on this LocalSet) to restart after a delay; otherwise
    // it marks the service crashed and keeps the Terminal so the crash output
    // stays visible. The returned set is the service-claimed terminal ids — the
    // daemon's equivalent of the GUI's (always-empty, since services run here)
    // `service_tids`.
    let service_tids: HashSet<String> = {
        let mut sm = context.service_manager.lock();
        let mut cx = context.reactor_ref.cx();
        let mut handled = HashSet::new();
        for (terminal_id, _, exit_code) in exit_events {
            if sm.handle_service_exit(terminal_id, *exit_code, &mut cx) {
                handled.insert(terminal_id.clone());
            }
        }
        handled
    };

    // ── 2. Hook-terminal exits ──────────────────────────────────────────────
    // Phase 1 (here): `notify_exit` unblocks any sync hook threads waiting on a
    // PTY terminal. This MUST happen before phase 2 (status updates / pending
    // worktree-close resolution) which may delete a project.
    if let Some(monitor) = context.reactor.hook_monitor.as_ref() {
        for (terminal_id, _, exit_code) in exit_events {
            monitor.notify_exit(terminal_id, *exit_code);
        }
    }
    let hook_tids = handle_hook_terminal_exits(exit_events, &service_tids, context);

    // ── 3. terminal.on_close for plain user terminals ───────────────────────
    // Same gating as the GUI: a global, project, OR parent-worktree on_close
    // must be present. Collect the args under a workspace read lock, then fire
    // the hooks (which spawn background subprocesses) outside it.
    let global_hooks = context.reactor.settings.lock().hooks.clone();
    let close_infos = collect_terminal_close_infos(
        exit_events,
        &service_tids,
        &hook_tids,
        context.reactor,
        &global_hooks,
    );
    let monitor = context.reactor.hook_monitor.as_ref();
    for info in close_infos {
        okena_hooks::fire_terminal_on_close_with_services(
            &info.project_hooks,
            info.parent_hooks.as_ref(),
            &info.project_id,
            &info.project_name,
            &info.project_path,
            &info.terminal_id,
            info.terminal_name.as_deref(),
            info.is_worktree,
            info.exit_code,
            info.folder_id.as_deref(),
            info.folder_name.as_deref(),
            &global_hooks,
            monitor,
        );
    }

    // ── 4. Kill + remove non-service, non-hook terminals ────────────────────
    // `kill` is critical for dtach: the PTY exit only means the client
    // disconnected, but the dtach daemon keeps running — `kill` SIGTERMs it and
    // removes the socket file.
    {
        let mut reg = context.terminals.lock();
        for (terminal_id, generation, _) in exit_events {
            if !service_tids.contains(terminal_id) && !hook_tids.contains(terminal_id) {
                context.pty_manager.kill_exited(terminal_id, *generation);
                reg.remove(terminal_id);
            }
        }
    }

    // ── 5. Stale soft-close reap ─────────────────────────────────────────────
    // If an exited terminal was mid soft-close, its pending record would
    // otherwise linger until the grace timer fired a redundant kill — drop it.
    // And if undo had just *restored* a now-doomed pane (racing this exit), tear
    // it back out. The daemon has no undo toast, so the returned toast id is
    // intentionally dropped (no UI dismissal to do).
    {
        let mut cx = context.reactor.workspace_cx();
        let mut ws = context.reactor.workspace.lock();
        for (tid, _, _) in exit_events {
            let _stale_toast = ws.cancel_pending_close(tid);
            ws.reap_restored_close(tid, &mut cx);
        }
    }
}

/// Phase 2 of hook-terminal exit handling: for each exited terminal that IS a
/// hook terminal, update the `HookMonitor`, set `HookTerminalStatus`, and
/// resolve any pending worktree close.
///
/// Returns the set of terminal ids that were hook terminals (so the caller skips
/// them in the `terminal.on_close` / kill+remove passes, mirroring the GUI).
///
/// ## Worktree-close adaptation (direct removal, not remote dispatch)
///
/// The GUI is a thin client whose `Workspace` mirror is read-only, so on a
/// successful close it dispatched a remote action to the daemon and then ran the
/// git worktree removal + `on_worktree_close` / `worktree_removed` hooks locally
/// in `handle_pending_close_result`. The daemon OWNS the workspace, so it runs
/// the canonical worktree removal DIRECTLY via
/// [`Workspace::remove_worktree_project`] — the SAME path
/// `execute_action(RemoveWorktreeProject)` takes: fire `on_worktree_close`, then
/// `git worktree remove`, then `delete_project` (which fires `on_project_close`).
///
/// The exited hook terminal that drives this is the `before_worktree_remove`
/// hook (registered alongside the pending close in the close-worktree dialog),
/// NOT `on_worktree_close`, so converging on `remove_worktree_project` does not
/// double-fire any hook.
///
/// Residual difference vs. the GUI: the daemon does the removal synchronously via
/// `remove_worktree` (`git worktree remove`) rather than the GUI's background
/// `remove_worktree_fast` + `worktree_removed` hook. This matches the normal
/// daemon `RemoveWorktreeProject` action exactly.
fn handle_hook_terminal_exits(
    exit_events: &[(String, PtyGeneration, Option<u32>)],
    service_tids: &HashSet<String>,
    context: &ExitHandlingContext<'_>,
) -> HashSet<String> {
    let hook_tids: HashSet<String> = {
        let ws = context.reactor.workspace.lock();
        exit_events
            .iter()
            .filter(|(tid, _, _)| !service_tids.contains(tid))
            .filter(|(tid, _, _)| ws.is_hook_terminal(tid).is_some())
            .map(|(tid, _, _)| tid.clone())
            .collect()
    };

    let global_hooks = context.reactor.settings.lock().hooks.clone();

    for (terminal_id, generation, exit_code) in exit_events {
        if !hook_tids.contains(terminal_id) {
            continue;
        }

        let success = *exit_code == Some(0);
        let tid = terminal_id.clone();

        // Set hook status + resolve any pending worktree close.
        let mut cx = context.reactor.workspace_cx();
        let mut ws = context.reactor.workspace.lock();
        let hook_is_running = ws.projects().iter().any(|project| {
            project
                .hook_terminals
                .get(&tid)
                .is_some_and(|entry| entry.status == HookTerminalStatus::Running)
        });
        // A completed keep-alive hook may be intentionally torn down while its
        // scrollback remains registered; its late PTY exit must not rewrite it.
        if !hook_is_running {
            continue;
        }

        if let Some(monitor) = context.reactor.hook_monitor.as_ref() {
            monitor.finish_by_terminal_id(&tid, *exit_code);
        }

        let status = if success {
            HookTerminalStatus::Succeeded
        } else {
            let code = exit_code
                .map(|c| i32::try_from(c).unwrap_or(i32::MAX))
                .unwrap_or(-1);
            HookTerminalStatus::Failed { exit_code: code }
        };
        ws.update_hook_terminal_status(&tid, status, &mut cx);

        if let Some(pending) = ws.take_pending_worktree_close(&tid) {
            if success {
                // The exited hook terminal is the `before_worktree_remove` hook
                // registered with this pending close, NOT `on_worktree_close`, so
                // firing `on_worktree_close` (in begin_worktree_removal) does not
                // double-fire any hook.
                ws.remove_hook_terminal(&tid, &mut cx);

                // Snapshot inputs + fire `on_worktree_close` under the lock, then
                // run the removal OFF the reactor. Previously the whole removal —
                // including `git worktree remove`, whose expensive status checks +
                // directory delete take SECONDS on a busy worktree (Docker holding
                // files) — ran synchronously here holding the workspace lock,
                // freezing every other daemon action until it finished. Now we hold
                // the lock only for the snapshot, run the git on a blocking thread
                // (fast removal, matching the GUI's hook-close path), and finalize
                // state (delete_project + worktree_removed) when it completes.
                match ws.begin_worktree_removal(&pending.project_id, &global_hooks, &mut cx) {
                    Ok(plan) => {
                        let operation_epoch = ws.data_replacement_epoch();
                        drop(ws);
                        teardown_completed_pending_hook(
                            &tid,
                            *generation,
                            context.terminals,
                            context.pty_manager,
                        );
                        let _ = crate::command_loop::spawn_background_worktree_removal(
                            plan,
                            operation_epoch,
                            false,
                            std::slice::from_ref(&tid),
                            &global_hooks,
                            &context.reactor.workspace,
                            &context.reactor.workspace_tick,
                            &context.reactor.hook_runner,
                            &context.reactor.hook_monitor,
                            &context.reactor.backend,
                            context.terminals,
                            &context.reactor.settings,
                            context.service_manager,
                            context.service_tick,
                            context.runtime,
                        );
                    }
                    Err(e) => {
                        // The removal was rejected after the hook already ran
                        // (e.g. the worktree is still mid-create). Abort like the
                        // hook-failure arm below: clear the mirrored `is_closing`
                        // marker so no client sticks at "Closing…" (and so the
                        // create-failure rollback isn't blocked by a closing
                        // project), notify, and toast why.
                        log::error!(
                            "worktree-close: begin_worktree_removal failed for {}: {e}",
                            pending.project_id
                        );
                        let project_name = ws
                            .project(&pending.project_id)
                            .map(|p| p.name.clone())
                            .unwrap_or_else(|| pending.project_id.clone());
                        ws.finish_closing_project(&pending.project_id);
                        cx.notify();
                        if let Some(hm) = context.reactor.hook_monitor.as_ref() {
                            hm.push_toast(okena_state::Toast::error(format!(
                                "\"{project_name}\" was not closed: {e}"
                            )));
                        }
                        drop(ws);
                        teardown_completed_pending_hook(
                            &tid,
                            *generation,
                            context.terminals,
                            context.pty_manager,
                        );
                    }
                }
            } else {
                // Hook failed → abort the close: unmark the project as closing
                // (clearing the mirrored `is_closing` marker heals the initiating
                // client's optimistic "Closing…" row), notify so the cleared flag
                // reaches clients, and toast why the worktree wasn't removed.
                let project_name = ws
                    .project(&pending.project_id)
                    .map(|p| p.name.clone())
                    .unwrap_or_else(|| pending.project_id.clone());
                ws.finish_closing_project(&pending.project_id);
                cx.notify();
                if let Some(hm) = context.reactor.hook_monitor.as_ref() {
                    hm.push_toast(okena_state::Toast::error(format!(
                        "before_worktree_remove hook failed — \"{project_name}\" was not closed"
                    )));
                }
            }
        }
        // Hook terminal persists on non-close paths — no auto-cleanup. A client
        // can dismiss or rerun it.
    }

    hook_tids
}

fn teardown_completed_pending_hook(
    terminal_id: &str,
    generation: PtyGeneration,
    terminals: &TerminalsRegistry,
    pty_manager: &PtyManager,
) {
    if !pty_manager.kill_exited(terminal_id, generation) {
        log::warn!(
            "worktree-close: exited before-remove hook {terminal_id} no longer owns its PTY generation"
        );
    }
    terminals.lock().remove(terminal_id);
}

/// Args for a single `terminal.on_close` hook firing, collected under the
/// workspace lock so the (subprocess-spawning) hook run happens outside it.
struct TerminalCloseInfo {
    project_hooks: okena_state::HooksConfig,
    parent_hooks: Option<okena_state::HooksConfig>,
    project_id: String,
    project_name: String,
    project_path: String,
    terminal_id: String,
    terminal_name: Option<String>,
    is_worktree: bool,
    exit_code: Option<u32>,
    folder_id: Option<String>,
    folder_name: Option<String>,
}

/// Collect `terminal.on_close` firing args for exited user terminals (non-service,
/// non-hook), applying the GUI's gating: fire only when a global, project, OR
/// parent-worktree `terminal.on_close` is configured. Retained ownership is
/// consumed here so duplicate exit events cannot fire the lifecycle twice.
fn collect_terminal_close_infos(
    exit_events: &[(String, PtyGeneration, Option<u32>)],
    service_tids: &HashSet<String>,
    hook_tids: &HashSet<String>,
    reactor: &PtyLoopReactor,
    global_hooks: &okena_state::HooksConfig,
) -> Vec<TerminalCloseInfo> {
    let global_on_close = global_hooks.terminal.on_close.is_some();
    let mut ws = reactor.workspace.lock();
    exit_events
        .iter()
        .filter(|(tid, _, _)| !service_tids.contains(tid) && !hook_tids.contains(tid))
        .filter_map(|(tid, _, exit_code)| {
            let layout_project_id = ws.find_project_for_terminal(tid).map(|p| p.id.clone());
            let retained_owner = ws.take_closing_terminal_owner(tid);
            let (project_id, retained_terminal_name) = match (layout_project_id, retained_owner) {
                (Some(project_id), retained) => (project_id, retained.and_then(|(_, name)| name)),
                (None, Some(owner)) => owner,
                (None, None) => return None,
            };
            let p = ws.project(&project_id)?;
            let parent_on_close = p
                .worktree_info
                .as_ref()
                .and_then(|wt| ws.project(&wt.parent_project_id))
                .and_then(|pp| pp.hooks.terminal.on_close.as_ref())
                .is_some();
            if !(global_on_close || p.hooks.terminal.on_close.is_some() || parent_on_close) {
                return None;
            }
            let parent_hooks = p
                .worktree_info
                .as_ref()
                .and_then(|wt| ws.project(&wt.parent_project_id))
                .map(|pp| pp.hooks.clone());
            let terminal_name = p
                .terminal_names
                .get(tid)
                .cloned()
                .or(retained_terminal_name);
            let is_worktree = p.worktree_info.is_some();
            let folder = ws.folder_for_project_or_parent(&p.id);
            let folder_id = folder.map(|f| f.id.clone());
            let folder_name = folder.map(|f| f.name.clone());
            Some(TerminalCloseInfo {
                project_hooks: p.hooks.clone(),
                parent_hooks,
                project_id: p.id.clone(),
                project_name: p.name.clone(),
                project_path: p.path.clone(),
                terminal_id: tid.clone(),
                terminal_name,
                is_worktree,
                exit_code: *exit_code,
                folder_id,
                folder_name,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    use okena_hooks::HookStatus;
    use okena_state::{HookTerminalEntry, ProjectData, WorktreeMetadata};
    use okena_terminal::backend::LocalBackend;
    use okena_terminal::session_backend::SessionBackend;
    use okena_terminal::shell_config::ShellType;
    use okena_terminal::terminal::{Terminal, TerminalSize};
    use okena_workspace::state::{LayoutNode, PendingWorktreeClose, WorkspaceData};
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::time::Duration;

    fn terminal_size() -> TerminalSize {
        TerminalSize {
            cols: 80,
            rows: 24,
            cell_width: 8.0,
            cell_height: 16.0,
        }
    }

    fn test_reactor(workspace: Workspace, settings: AppSettings) -> PtyLoopReactor {
        let (pty_manager, _events) = PtyManager::new(SessionBackend::None);
        test_reactor_with_manager(workspace, settings, Arc::new(pty_manager))
    }

    fn test_reactor_with_manager(
        workspace: Workspace,
        settings: AppSettings,
        pty_manager: Arc<PtyManager>,
    ) -> PtyLoopReactor {
        let (workspace_tick, _wrx) = watch::channel(0u64);
        let backend = Arc::new(LocalBackend::new(pty_manager));
        PtyLoopReactor {
            workspace: Arc::new(Mutex::new(workspace)),
            backend,
            hook_runner: None,
            hook_monitor: Some(HookMonitor::new()),
            workspace_tick,
            settings: Arc::new(Mutex::new(settings)),
        }
    }

    fn run_git(cwd: &Path, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .expect("run git");
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn real_git_worktree() -> (PathBuf, PathBuf) {
        let repo = std::env::temp_dir().join(format!(
            "okena-pty-close-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let worktree = repo.with_extension("worktree");
        std::fs::create_dir_all(&repo).expect("create repository directory");
        run_git(&repo, &["init", "-q", "-b", "main"]);
        run_git(&repo, &["config", "user.email", "test@okena.local"]);
        run_git(&repo, &["config", "user.name", "Okena Test"]);
        std::fs::write(repo.join("file.txt"), "base\n").expect("write fixture");
        run_git(&repo, &["add", "file.txt"]);
        run_git(&repo, &["commit", "-q", "-m", "base"]);
        run_git(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "feature",
                worktree.to_str().expect("utf-8 worktree path"),
            ],
        );
        (repo, worktree)
    }

    fn workspace_with_pending_close(
        main_repo: &Path,
        worktree: &Path,
        hook_terminal_id: &str,
    ) -> Workspace {
        let parent = ProjectData {
            id: "parent".into(),
            name: "Parent".into(),
            path: main_repo.to_string_lossy().into_owned(),
            layout: None,
            terminal_names: Default::default(),
            hidden_terminals: Default::default(),
            worktree_info: None,
            worktree_ids: vec!["wt1".into()],
            folder_color: Default::default(),
            hooks: Default::default(),
            is_remote: false,
            connection_id: None,
            service_terminals: Default::default(),
            agent_sessions: Default::default(),
            pending_agent_resumes: Default::default(),
            default_shell: None,
            hook_terminals: Default::default(),
            pinned: false,
            last_activity_at: None,
            is_creating: false,
            is_closing: false,
        };
        let child = ProjectData {
            id: "wt1".into(),
            name: "Feature".into(),
            path: worktree.to_string_lossy().into_owned(),
            layout: None,
            terminal_names: HashMap::from([(
                hook_terminal_id.to_string(),
                "Before remove".to_string(),
            )]),
            hidden_terminals: Default::default(),
            worktree_info: Some(WorktreeMetadata {
                parent_project_id: "parent".into(),
                color_override: None,
                main_repo_path: main_repo.to_string_lossy().into_owned(),
                worktree_path: worktree.to_string_lossy().into_owned(),
                branch_name: "feature".into(),
            }),
            worktree_ids: Vec::new(),
            folder_color: Default::default(),
            hooks: Default::default(),
            is_remote: false,
            connection_id: None,
            service_terminals: Default::default(),
            agent_sessions: Default::default(),
            pending_agent_resumes: Default::default(),
            default_shell: None,
            hook_terminals: HashMap::from([(
                hook_terminal_id.to_string(),
                HookTerminalEntry {
                    label: "Before remove".into(),
                    status: HookTerminalStatus::Running,
                    hook_type: "before_worktree_remove".into(),
                    command: "true".into(),
                    cwd: worktree.to_string_lossy().into_owned(),
                },
            )]),
            pinned: false,
            last_activity_at: None,
            is_creating: false,
            is_closing: false,
        };
        let mut workspace = Workspace::new(WorkspaceData {
            version: 1,
            projects: vec![parent, child],
            project_order: vec!["parent".into()],
            folders: Vec::new(),
            service_panel_heights: Default::default(),
            hook_panel_heights: Default::default(),
            main_window: Default::default(),
            extra_windows: Vec::new(),
        });
        workspace.register_pending_worktree_close(PendingWorktreeClose {
            project_id: "wt1".into(),
            hook_terminal_id: hook_terminal_id.into(),
            branch: "feature".into(),
            main_repo_path: main_repo.to_string_lossy().into_owned(),
        });
        workspace
    }

    fn plain_project(terminal_id: &str) -> ProjectData {
        ProjectData {
            id: "project-1".into(),
            name: "Project One".into(),
            path: "/tmp/project-one".into(),
            layout: Some(LayoutNode::Terminal {
                terminal_id: Some(terminal_id.into()),
                minimized: false,
                detached: false,
                shell_type: ShellType::Default,
                zoom_level: 1.0,
            }),
            terminal_names: HashMap::from([(terminal_id.into(), "Build shell".into())]),
            hidden_terminals: Default::default(),
            worktree_info: None,
            worktree_ids: Vec::new(),
            folder_color: Default::default(),
            hooks: Default::default(),
            is_remote: false,
            connection_id: None,
            service_terminals: Default::default(),
            agent_sessions: Default::default(),
            pending_agent_resumes: Default::default(),
            default_shell: None,
            hook_terminals: Default::default(),
            pinned: false,
            last_activity_at: None,
            is_creating: false,
            is_closing: false,
        }
    }

    #[test]
    fn terminal_close_uses_retained_owner_after_layout_removal_once() {
        let mut project = plain_project("terminal-1");
        project.hooks.terminal.on_close = Some("echo closed".into());
        let mut workspace = Workspace::new(WorkspaceData {
            version: 1,
            projects: vec![project],
            project_order: vec!["project-1".into()],
            folders: Vec::new(),
            service_panel_heights: Default::default(),
            hook_panel_heights: Default::default(),
            main_window: Default::default(),
            extra_windows: Vec::new(),
        });
        workspace.remember_closing_terminal_owner("project-1", "terminal-1");
        let project = workspace.data.projects.first_mut().expect("project");
        project.layout = None;
        project.terminal_names.clear();

        let reactor = test_reactor(workspace, AppSettings::default());
        let (pty_manager, _events) = PtyManager::new(SessionBackend::None);
        let cwd = std::env::temp_dir().to_string_lossy().into_owned();
        pty_manager
            .create_or_reconnect_terminal(Some("terminal-1"), &cwd)
            .expect("create tracked PTY");
        let generation = pty_manager
            .current_generation("terminal-1")
            .expect("tracked generation");
        let exits = vec![("terminal-1".to_string(), generation, Some(9))];
        let info = collect_terminal_close_infos(
            &exits,
            &HashSet::new(),
            &HashSet::new(),
            &reactor,
            &okena_state::HooksConfig::default(),
        );

        assert_eq!(info.len(), 1);
        assert_eq!(info[0].project_id, "project-1");
        assert_eq!(info[0].terminal_name.as_deref(), Some("Build shell"));
        assert_eq!(info[0].exit_code, Some(9));
        assert!(
            collect_terminal_close_infos(
                &exits,
                &HashSet::new(),
                &HashSet::new(),
                &reactor,
                &okena_state::HooksConfig::default(),
            )
            .is_empty(),
            "retained ownership is consumed by the first exit"
        );
    }

    #[test]
    fn osc_hook_exit_finishes_monitor_and_queues_failure_toast() {
        let mut project = plain_project("hook-1");
        project.hook_terminals.insert(
            "hook-1".into(),
            HookTerminalEntry {
                label: "Open hook".into(),
                status: HookTerminalStatus::Running,
                hook_type: "on_project_open".into(),
                command: "exit 7".into(),
                cwd: "/tmp/project-one".into(),
            },
        );
        let workspace = Workspace::new(WorkspaceData {
            version: 1,
            projects: vec![project],
            project_order: vec!["project-1".into()],
            folders: Vec::new(),
            service_panel_heights: Default::default(),
            hook_panel_heights: Default::default(),
            main_window: Default::default(),
            extra_windows: Vec::new(),
        });
        let reactor = test_reactor(workspace, AppSettings::default());
        let monitor = reactor.hook_monitor.clone().expect("hook monitor");
        monitor.record_start(
            "on_project_open",
            "exit 7",
            "Project One",
            Some("hook-1".into()),
        );

        let terminals: TerminalsRegistry = Arc::new(Mutex::new(Default::default()));
        let terminal = Arc::new(Terminal::new(
            "hook-1".into(),
            terminal_size(),
            reactor.backend.transport(),
            "/tmp/project-one".into(),
        ));
        terminal.process_output(b"\x1b]0;__okena_hook_exit:7\x07");
        terminals.lock().insert("hook-1".into(), terminal);

        process_osc_hook_exits(&["hook-1".into()], &terminals, &reactor);

        let workspace = reactor.workspace.lock();
        assert!(matches!(
            workspace
                .project("project-1")
                .expect("project")
                .hook_terminals["hook-1"]
                .status,
            HookTerminalStatus::Failed { exit_code: 7 }
        ));
        drop(workspace);
        assert!(matches!(
            monitor.history()[0].status,
            HookStatus::Failed { exit_code: 7, .. }
        ));
        assert_eq!(monitor.drain_pending_toasts().len(), 1);
    }

    #[tokio::test]
    async fn nonzero_osc_hook_exit_aborts_pending_worktree_close_once() {
        let repo = std::env::temp_dir().join("okena-osc-hook-failure-main");
        let worktree = std::env::temp_dir().join("okena-osc-hook-failure-worktree");
        let reactor = test_reactor(
            workspace_with_pending_close(&repo, &worktree, "hook-osc"),
            AppSettings::default(),
        );
        let monitor = reactor.hook_monitor.clone().expect("hook monitor");
        monitor.record_start(
            "before_worktree_remove",
            "exit 7",
            "Feature",
            Some("hook-osc".into()),
        );
        let terminals: TerminalsRegistry = Arc::new(Mutex::new(Default::default()));
        let terminal = Arc::new(Terminal::new(
            "hook-osc".into(),
            terminal_size(),
            reactor.backend.transport(),
            worktree.to_string_lossy().into_owned(),
        ));
        terminal.process_output(b"\x1b]0;__okena_hook_exit:7\x07");
        terminals.lock().insert("hook-osc".into(), terminal);
        let osc_results = process_osc_hook_exits(&["hook-osc".into()], &terminals, &reactor);
        assert_eq!(osc_results, vec![("hook-osc".into(), 7)]);

        let (pty_manager, _events) = PtyManager::new(SessionBackend::None);
        let services = Arc::new(Mutex::new(ServiceManager::new(
            reactor.backend.clone(),
            terminals.clone(),
        )));
        let (service_tick, _service_rx) = watch::channel(0u64);
        resolve_osc_worktree_closes(
            &osc_results,
            &terminals,
            &pty_manager,
            &services,
            &service_tick,
            &Handle::current(),
            &reactor,
        );
        let workspace = reactor.workspace.lock();
        let project = workspace
            .project("wt1")
            .expect("failed hook retains worktree");
        assert!(!workspace.is_project_closing("wt1"));
        assert!(!project.is_closing);
        assert!(matches!(
            project.hook_terminals["hook-osc"].status,
            HookTerminalStatus::Failed { exit_code: 7 }
        ));
        drop(workspace);
        assert_eq!(monitor.drain_pending_toasts().len(), 1);

        resolve_osc_worktree_closes(
            &osc_results,
            &terminals,
            &pty_manager,
            &services,
            &service_tick,
            &Handle::current(),
            &reactor,
        );
        assert!(
            monitor.drain_pending_toasts().is_empty(),
            "late OSC is a no-op"
        );
    }

    async fn drive_hook_exit_through_pty_loop(
        reactor: PtyLoopReactor,
        terminals: TerminalsRegistry,
        terminal_id: &str,
        exit_code: Option<u32>,
    ) {
        let (pty_manager, _unused_events) = PtyManager::new(SessionBackend::None);
        let cwd = std::env::temp_dir().to_string_lossy().into_owned();
        pty_manager
            .create_or_reconnect_terminal(Some(terminal_id), &cwd)
            .expect("create tracked hook PTY");
        let generation = pty_manager
            .current_generation(terminal_id)
            .expect("tracked generation");
        let pty_manager = Arc::new(pty_manager);
        let service_manager = Arc::new(Mutex::new(ServiceManager::new(
            reactor.backend.clone(),
            terminals.clone(),
        )));
        let (service_tick, _service_rx) = watch::channel(0u64);
        let runtime = Handle::current();
        let reactor_ref = ServiceReactorRef::new(
            service_manager.clone(),
            runtime.clone(),
            service_tick.clone(),
        );
        let context = ExitHandlingContext {
            terminals: &terminals,
            pty_manager: pty_manager.as_ref(),
            service_manager: &service_manager,
            reactor_ref: &reactor_ref,
            service_tick: &service_tick,
            runtime: &runtime,
            reactor: &reactor,
        };
        handle_exits(
            &[(terminal_id.to_string(), generation, exit_code)],
            &context,
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn successful_hook_pty_exit_removes_real_worktree() {
        let (repo, worktree) = real_git_worktree();
        let (pty_manager, pty_events) = PtyManager::new(SessionBackend::None);
        let hook_terminal_id = pty_manager
            .create_terminal_with_shell(
                worktree.to_str().expect("utf-8 worktree path"),
                Some(&ShellType::for_command("exit 0".to_string())),
            )
            .expect("create before-remove hook PTY");
        let pty_manager = Arc::new(pty_manager);
        let reactor = test_reactor_with_manager(
            workspace_with_pending_close(&repo, &worktree, &hook_terminal_id),
            AppSettings::default(),
            pty_manager.clone(),
        );
        let workspace = reactor.workspace.clone();
        let terminals: TerminalsRegistry = Arc::new(Mutex::new(Default::default()));
        terminals.lock().insert(
            hook_terminal_id.clone(),
            Arc::new(Terminal::new(
                hook_terminal_id.clone(),
                terminal_size(),
                pty_manager.clone(),
                worktree.to_string_lossy().into_owned(),
            )),
        );
        let service_manager = Arc::new(Mutex::new(ServiceManager::new(
            reactor.backend.clone(),
            terminals.clone(),
        )));
        let (service_tick, _service_rx) = watch::channel(0u64);
        let runtime = Handle::current();
        let reactor_ref = ServiceReactorRef::new(
            service_manager.clone(),
            runtime.clone(),
            service_tick.clone(),
        );

        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let mut exit_events = Vec::new();
                let mut dirty_terminal_ids = Vec::new();
                let mut bytes_this_turn = 0;
                tokio::time::timeout(Duration::from_secs(2), async {
                    while exit_events.is_empty() {
                        let event = pty_events.recv().await.expect("receive hook PTY event");
                        process_event(
                            &event,
                            &terminals,
                            &pty_manager,
                            &mut exit_events,
                            &mut dirty_terminal_ids,
                            &mut bytes_this_turn,
                        );
                    }
                })
                .await
                .expect("before-remove hook exits");

                let context = ExitHandlingContext {
                    terminals: &terminals,
                    pty_manager: pty_manager.as_ref(),
                    service_manager: &service_manager,
                    reactor_ref: &reactor_ref,
                    service_tick: &service_tick,
                    runtime: &runtime,
                    reactor: &reactor,
                };
                handle_exits(&exit_events, &context);

                assert!(
                    !terminals.lock().contains_key(&hook_terminal_id),
                    "successful pending hook releases registry ownership"
                );
                assert!(
                    pty_manager.current_generation(&hook_terminal_id).is_none(),
                    "successful pending hook releases PTY generation ownership"
                );

                tokio::time::timeout(Duration::from_secs(3), async {
                    loop {
                        if workspace.lock().project("wt1").is_none() {
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                })
                .await
                .expect("PTY completion removes worktree");
            })
            .await;

        assert!(!worktree.exists(), "checkout was physically removed");
        assert!(
            !String::from_utf8_lossy(
                &Command::new("git")
                    .args(["worktree", "list", "--porcelain"])
                    .current_dir(&repo)
                    .output()
                    .expect("list worktrees")
                    .stdout
            )
            .contains(worktree.to_string_lossy().as_ref()),
            "git worktree registration was pruned"
        );
        assert!(
            workspace
                .lock()
                .project("parent")
                .expect("parent remains")
                .worktree_ids
                .is_empty()
        );
        std::fs::remove_dir_all(repo).ok();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn successful_osc_hook_exit_removes_worktree_once() {
        let (repo, worktree) = real_git_worktree();
        let (pty_manager, _events) = PtyManager::new(SessionBackend::None);
        let hook_terminal_id = pty_manager
            .create_terminal_with_shell(
                worktree.to_str().expect("utf-8 worktree path"),
                Some(&ShellType::for_command("sleep 30".to_string())),
            )
            .expect("create keep-alive before-remove hook PTY");
        let pty_manager = Arc::new(pty_manager);
        let reactor = test_reactor_with_manager(
            workspace_with_pending_close(&repo, &worktree, &hook_terminal_id),
            AppSettings::default(),
            pty_manager.clone(),
        );
        let workspace = reactor.workspace.clone();
        let terminals: TerminalsRegistry = Arc::new(Mutex::new(Default::default()));
        let terminal = Arc::new(Terminal::new(
            hook_terminal_id.clone(),
            terminal_size(),
            pty_manager.clone(),
            worktree.to_string_lossy().into_owned(),
        ));
        terminal.process_output(b"\x1b]0;__okena_hook_exit:0\x07");
        terminals.lock().insert(hook_terminal_id.clone(), terminal);
        let osc_results = process_osc_hook_exits(
            std::slice::from_ref(&hook_terminal_id),
            &terminals,
            &reactor,
        );
        assert_eq!(osc_results, vec![(hook_terminal_id.clone(), 0)]);
        let service_manager = Arc::new(Mutex::new(ServiceManager::new(
            reactor.backend.clone(),
            terminals.clone(),
        )));
        let (service_tick, _service_rx) = watch::channel(0u64);
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                resolve_osc_worktree_closes(
                    &osc_results,
                    &terminals,
                    &pty_manager,
                    &service_manager,
                    &service_tick,
                    &Handle::current(),
                    &reactor,
                );
                tokio::time::timeout(Duration::from_secs(3), async {
                    while workspace.lock().project("wt1").is_some() {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                })
                .await
                .expect("OSC success removes worktree through canonical path");
                resolve_osc_worktree_closes(
                    &osc_results,
                    &terminals,
                    &pty_manager,
                    &service_manager,
                    &service_tick,
                    &Handle::current(),
                    &reactor,
                );
            })
            .await;

        assert!(!worktree.exists(), "checkout was physically removed once");
        assert!(workspace.lock().project("wt1").is_none());
        std::fs::remove_dir_all(repo).ok();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn failed_hook_pty_exit_aborts_pending_close() {
        let repo = std::env::temp_dir().join("okena-hook-failure-main");
        let worktree = std::env::temp_dir().join("okena-hook-failure-worktree");
        let reactor = test_reactor(
            workspace_with_pending_close(&repo, &worktree, "hook-1"),
            AppSettings::default(),
        );
        let workspace = reactor.workspace.clone();
        let hook_monitor = reactor.hook_monitor.clone().expect("hook monitor");
        let terminals: TerminalsRegistry = Arc::new(Mutex::new(Default::default()));

        let local = tokio::task::LocalSet::new();
        local
            .run_until(drive_hook_exit_through_pty_loop(
                reactor,
                terminals,
                "hook-1",
                Some(7),
            ))
            .await;

        let workspace_guard = workspace.lock();
        let project = workspace_guard.project("wt1").expect("project retained");
        assert!(!project.is_closing);
        assert!(!workspace_guard.is_project_closing("wt1"));
        assert!(matches!(
            project.hook_terminals["hook-1"].status,
            HookTerminalStatus::Failed { exit_code: 7 }
        ));
        drop(workspace_guard);
        assert_eq!(hook_monitor.drain_pending_toasts().len(), 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn late_exit_preserves_completed_hook_status_and_registry_buffer() {
        let mut project = plain_project("ordinary");
        project.hook_terminals.insert(
            "completed-hook".into(),
            HookTerminalEntry {
                label: "Completed hook".into(),
                status: HookTerminalStatus::Succeeded,
                hook_type: "on_project_open".into(),
                command: "echo done".into(),
                cwd: "/tmp/project-one".into(),
            },
        );
        let workspace = Workspace::new(WorkspaceData {
            version: 1,
            projects: vec![project],
            project_order: vec!["project-1".into()],
            folders: Vec::new(),
            service_panel_heights: Default::default(),
            hook_panel_heights: Default::default(),
            main_window: Default::default(),
            extra_windows: Vec::new(),
        });
        let reactor = test_reactor(workspace, AppSettings::default());
        let workspace = reactor.workspace.clone();
        let terminals: TerminalsRegistry = Arc::new(Mutex::new(Default::default()));
        let retained = Arc::new(Terminal::new(
            "completed-hook".into(),
            terminal_size(),
            reactor.backend.transport(),
            "/tmp/project-one".into(),
        ));
        retained.process_output(b"preserved output\r\n");
        terminals
            .lock()
            .insert("completed-hook".into(), retained.clone());

        let local = tokio::task::LocalSet::new();
        local
            .run_until(drive_hook_exit_through_pty_loop(
                reactor,
                terminals.clone(),
                "completed-hook",
                None,
            ))
            .await;

        assert!(matches!(
            workspace
                .lock()
                .project("project-1")
                .expect("project")
                .hook_terminals["completed-hook"]
                .status,
            HookTerminalStatus::Succeeded
        ));
        assert!(Arc::ptr_eq(
            terminals
                .lock()
                .get("completed-hook")
                .expect("retained terminal"),
            &retained
        ));
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

        // A real tracked PTY generation validates the synthesized data event.
        let (pty_manager, _pty_manager_events) = PtyManager::new(SessionBackend::None);
        let cwd = std::env::temp_dir().to_string_lossy().into_owned();
        let terminal_id = pty_manager
            .create_terminal_with_shell(&cwd, None)
            .expect("create tracked PTY instance");
        let generation = pty_manager
            .current_generation(&terminal_id)
            .expect("current PTY generation");
        let pty_manager = Arc::new(pty_manager);

        let terminals: TerminalsRegistry = Arc::new(parking_lot::Mutex::new(Default::default()));
        let transport = pty_manager.clone(); // PtyManager: TerminalTransport
        let term = Arc::new(Terminal::new(
            terminal_id.clone(),
            terminal_size(),
            transport,
            cwd,
        ));
        let gen_before = term.content_generation();
        terminals.lock().insert(terminal_id.clone(), term.clone());

        let backend = Arc::new(LocalBackend::new(pty_manager.clone()));
        let service_manager = Arc::new(Mutex::new(ServiceManager::new(backend, terminals.clone())));

        let (service_tick, _srx) = watch::channel(0u64);
        let (state_version, _vrx) = watch::channel(0u64);
        let reactor = test_reactor(
            Workspace::new(WorkspaceData::empty()),
            AppSettings::default(),
        );

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
                    reactor,
                    state_version,
                ));

                tx.send(PtyEvent::Data {
                    terminal_id,
                    generation,
                    data: b"hello".to_vec(),
                    sequence: 0,
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

    #[test]
    fn duplicate_exit_events_are_claimed_once_across_batches() {
        let (pty_manager, _events) = PtyManager::new(SessionBackend::None);
        let cwd = std::env::temp_dir().to_string_lossy().into_owned();
        let terminal_id = pty_manager
            .create_or_reconnect_terminal(Some("duplicate-exit"), &cwd)
            .expect("create PTY");
        let generation = pty_manager
            .current_generation(&terminal_id)
            .expect("generation");
        let event = PtyEvent::Exit {
            terminal_id,
            generation,
            exit_code: Some(0),
        };
        let terminals: TerminalsRegistry = Arc::new(Mutex::new(Default::default()));
        let mut dirty = Vec::new();
        let mut bytes = 0;

        let mut first_batch = Vec::new();
        process_event(
            &event,
            &terminals,
            &pty_manager,
            &mut first_batch,
            &mut dirty,
            &mut bytes,
        );
        assert_eq!(first_batch.len(), 1);

        let mut second_batch = Vec::new();
        process_event(
            &event,
            &terminals,
            &pty_manager,
            &mut second_batch,
            &mut dirty,
            &mut bytes,
        );
        assert!(second_batch.is_empty());
        pty_manager.flush_teardown();
    }

    #[test]
    fn delayed_old_generation_exit_does_not_claim_reconnected_terminal() {
        let (pty_manager, _events) = PtyManager::new(SessionBackend::None);
        let cwd = std::env::temp_dir().to_string_lossy().into_owned();
        let terminal_id = pty_manager
            .create_or_reconnect_terminal(Some("reconnected"), &cwd)
            .expect("create old PTY");
        let old_generation = pty_manager
            .current_generation(&terminal_id)
            .expect("old generation");
        pty_manager.kill(&terminal_id);
        pty_manager
            .create_or_reconnect_terminal(Some(&terminal_id), &cwd)
            .expect("create new PTY generation");
        let new_generation = pty_manager
            .current_generation(&terminal_id)
            .expect("new generation");
        assert_ne!(old_generation, new_generation);

        let event = PtyEvent::Exit {
            terminal_id: terminal_id.clone(),
            generation: old_generation,
            exit_code: Some(0),
        };
        let terminals: TerminalsRegistry = Arc::new(Mutex::new(Default::default()));
        let mut exits = Vec::new();
        process_event(
            &event,
            &terminals,
            &pty_manager,
            &mut exits,
            &mut Vec::new(),
            &mut 0,
        );

        assert!(exits.is_empty());
        assert!(pty_manager.is_current_generation(&terminal_id, new_generation));
        pty_manager.kill(&terminal_id);
        pty_manager.flush_teardown();
    }
}
