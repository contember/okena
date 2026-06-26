//! GPUI-free PTY event loop: the headless analogue of the GUI's batched
//! `async_channel` drain in `okena-app`'s `app/mod.rs` / `app/headless.rs`.
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
use okena_terminal::pty_manager::{PtyEvent, PtyManager};
use okena_terminal::TerminalsRegistry;
use okena_workspace::focus::FocusManager;
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
            process_osc_hook_exits(&dirty_terminal_ids, &terminals, &reactor);
            // Command-finished (OSC 133 ;D) activity: stamp `last_activity_at` on
            // the owning project so the activity-sorted sidebar floats it up. Bump
            // `state_version` if anything was stamped so clients resync (the bump
            // is mirrored into StateResponse).
            if process_command_finished_activity(&dirty_terminal_ids, &terminals, &reactor) {
                state_version.send_modify(|v| *v += 1);
            }
        }

        if !exit_events.is_empty() {
            handle_exits(
                &exit_events,
                &terminals,
                &pty_manager,
                &service_manager,
                &reactor_ref,
                &reactor,
            );
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
    exit_events: &mut Vec<(String, Option<u32>)>,
    dirty_terminal_ids: &mut Vec<String>,
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
            dirty_terminal_ids.push(terminal_id.clone());
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

/// Hook-exit-via-OSC-title: for any terminal that produced output this batch and
/// IS a hook terminal, if its title is `__okena_hook_exit:<code>`, set the hook
/// status to Succeeded (code 0) / Failed otherwise.
///
/// Mirrors the GUI's post-batch dirty-title scan (`app/mod.rs`). This happens for
/// keep-alive hooks whose command finished but whose PTY stays alive as an
/// interactive shell, so there is no PTY `Exit` to drive the status.
fn process_osc_hook_exits(
    dirty_terminal_ids: &[String],
    terminals: &TerminalsRegistry,
    reactor: &PtyLoopReactor,
) {
    // Collect status updates under the registry + workspace read locks, then
    // apply them under a single workspace write lock (matching the GUI's split).
    let mut status_updates: Vec<(String, HookTerminalStatus)> = Vec::new();
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
                status_updates.push((tid.clone(), status));
            }
        }
    }
    if !status_updates.is_empty() {
        let mut cx = reactor.workspace_cx();
        let mut ws = reactor.workspace.lock();
        for (tid, status) in status_updates {
            ws.update_hook_terminal_status(&tid, status, &mut cx);
        }
    }
}

/// Drain the one-shot command-finished (OSC 133 ;D) edge for each terminal that
/// produced output this batch and stamp `last_activity_at` on the owning project
/// (drives the activity-sorted sidebar). Returns `true` if any activity was
/// stamped, so the caller can bump `state_version` for clients to resync.
///
/// Mirrors the GUI's `Okena::process_command_finished_activity` +
/// `bump_activity_for_terminals`: drain the cheap atomic edge first (almost every
/// batch drains nothing), resolve the finished terminals to their owning projects
/// (deduplicated), then `bump_activity` once per project. The daemon's own
/// `Terminal`s parse OSC 133 in `process_output`, so the edge is available here.
fn process_command_finished_activity(
    dirty_terminal_ids: &[String],
    terminals: &TerminalsRegistry,
    reactor: &PtyLoopReactor,
) -> bool {
    // Drain edges first (cheap atomic swap); collect the terminals that actually
    // saw a command finish. The lock is dropped before touching the workspace.
    let finished: Vec<String> = {
        let reg = terminals.lock();
        dirty_terminal_ids
            .iter()
            .filter(|tid| {
                reg.get(*tid)
                    .is_some_and(|t| t.take_pending_command_finished())
            })
            .cloned()
            .collect()
    };
    if finished.is_empty() {
        return false;
    }

    // Resolve each finished terminal to its owning project, deduplicating so a
    // batch touching several terminals of the same project bumps it once.
    let project_ids: HashSet<String> = {
        let ws = reactor.workspace.lock();
        finished
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

/// Handle the exits collected in one batch:
/// 1. Let the service manager claim its service terminals (restart /
///    keep-crash-output) — yields the `service_tids` set.
/// 2. Resolve hook-terminal exits: notify the monitor, set hook status, and
///    resolve any pending worktree close (delete the project DIRECTLY in the
///    daemon workspace on success; finish-closing on failure) — yields the
///    `hook_tids` set.
/// 3. Fire `terminal.on_close` for plain user terminals (non-service, non-hook).
/// 4. Kill + remove the UI Terminal for every non-service, non-hook terminal.
/// 5. Drop stale soft-close records for any exited terminal.
///
/// Mirrors the GUI's PTY-exit handling, adapted: the GUI is a thin client and
/// dispatched a remote `DeleteProject`; the daemon owns the workspace and
/// deletes directly. The GUI-only window/pane notify + toast dismissal have no
/// daemon surface and are dropped (the soft-close *state* cleanup still runs).
fn handle_exits(
    exit_events: &[(String, Option<u32>)],
    terminals: &TerminalsRegistry,
    pty_manager: &PtyManager,
    service_manager: &Arc<Mutex<ServiceManager>>,
    reactor_ref: &ServiceReactorRef,
    reactor: &PtyLoopReactor,
) {
    // ── 1. Service terminals ────────────────────────────────────────────────
    // For a crashed service with `restart_on_crash`, `handle_service_exit` calls
    // `spawn_main` (lands on this LocalSet) to restart after a delay; otherwise
    // it marks the service crashed and keeps the Terminal so the crash output
    // stays visible. The returned set is the service-claimed terminal ids — the
    // daemon's equivalent of the GUI's (always-empty, since services run here)
    // `service_tids`.
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

    // ── 2. Hook-terminal exits ──────────────────────────────────────────────
    // Phase 1 (here): `notify_exit` unblocks any sync hook threads waiting on a
    // PTY terminal. This MUST happen before phase 2 (status updates / pending
    // worktree-close resolution) which may delete a project.
    if let Some(monitor) = reactor.hook_monitor.as_ref() {
        for (terminal_id, exit_code) in exit_events {
            monitor.notify_exit(terminal_id, *exit_code);
        }
    }
    let hook_tids =
        handle_hook_terminal_exits(exit_events, &service_tids, reactor);

    // ── 3. terminal.on_close for plain user terminals ───────────────────────
    // Same gating as the GUI: a global, project, OR parent-worktree on_close
    // must be present. Collect the args under a workspace read lock, then fire
    // the hooks (which spawn background subprocesses) outside it.
    let global_hooks = reactor.settings.lock().hooks.clone();
    let close_infos = collect_terminal_close_infos(exit_events, &service_tids, &hook_tids, reactor, &global_hooks);
    let monitor = reactor.hook_monitor.as_ref();
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
        let mut reg = terminals.lock();
        for (terminal_id, _) in exit_events {
            if !service_tids.contains(terminal_id) && !hook_tids.contains(terminal_id) {
                pty_manager.kill(terminal_id);
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
        let mut cx = reactor.workspace_cx();
        let mut ws = reactor.workspace.lock();
        for (tid, _) in exit_events {
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
/// ## Worktree-close adaptation (direct delete, not remote dispatch)
///
/// The GUI is a thin client whose `Workspace` mirror is read-only, so on a
/// successful close it dispatched a remote `ActionRequest::DeleteProject` to the
/// daemon. The daemon OWNS the workspace, so it deletes the worktree project
/// DIRECTLY via the workspace action layer (`delete_project`) — the same path
/// `execute_action(DeleteProject)` takes — instead of round-tripping an action.
///
/// (The GUI additionally fired `on_worktree_close` / `worktree_removed` hooks and
/// did the `git worktree remove` in `handle_pending_close_result`; those steps
/// are the still-pending daemon-side worktree work — they are out of scope here.)
fn handle_hook_terminal_exits(
    exit_events: &[(String, Option<u32>)],
    service_tids: &HashSet<String>,
    reactor: &PtyLoopReactor,
) -> HashSet<String> {
    let hook_tids: HashSet<String> = {
        let ws = reactor.workspace.lock();
        exit_events
            .iter()
            .filter(|(tid, _)| !service_tids.contains(tid))
            .filter(|(tid, _)| ws.is_hook_terminal(tid).is_some())
            .map(|(tid, _)| tid.clone())
            .collect()
    };

    let global_hooks = reactor.settings.lock().hooks.clone();

    for (terminal_id, exit_code) in exit_events {
        if !hook_tids.contains(terminal_id) {
            continue;
        }

        let success = *exit_code == Some(0);
        let tid = terminal_id.clone();

        // Update HookMonitor so the hook log shows correct status.
        if let Some(monitor) = reactor.hook_monitor.as_ref() {
            monitor.finish_by_terminal_id(&tid, *exit_code);
        }

        // Set hook status + resolve any pending worktree close.
        //
        // On a successful close the project deletion is done DIRECTLY here (the
        // daemon owns the workspace). `delete_project` needs a `&mut
        // FocusManager`; daemon focus state is dormant (never drives a render),
        // so an ephemeral one is fine.
        let mut focus_manager = FocusManager::new();
        let mut cx = reactor.workspace_cx();
        let mut ws = reactor.workspace.lock();

        let status = if success {
            HookTerminalStatus::Succeeded
        } else {
            let code = exit_code.map(|c| i32::try_from(c).unwrap_or(i32::MAX)).unwrap_or(-1);
            HookTerminalStatus::Failed { exit_code: code }
        };
        ws.update_hook_terminal_status(&tid, status, &mut cx);

        if let Some(pending) = ws.take_pending_worktree_close(&tid) {
            if success {
                // Drop the hook terminal record, then delete the worktree
                // project directly (mirrors the GUI's remove_hook_terminal +
                // dispatched DeleteProject).
                ws.remove_hook_terminal(&tid, &mut cx);
                ws.delete_project(&mut focus_manager, &pending.project_id, &global_hooks, &mut cx);
            } else {
                // Hook failed → abort the close: unmark the project as closing.
                ws.finish_closing_project(&pending.project_id);
            }
        }
        // Hook terminal persists on non-close paths — no auto-cleanup. A client
        // can dismiss or rerun it.
    }

    hook_tids
}

/// Args for a single `terminal.on_close` hook firing, collected under the
/// workspace read lock so the (subprocess-spawning) hook run happens outside it.
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
/// parent-worktree `terminal.on_close` is configured.
fn collect_terminal_close_infos(
    exit_events: &[(String, Option<u32>)],
    service_tids: &HashSet<String>,
    hook_tids: &HashSet<String>,
    reactor: &PtyLoopReactor,
    global_hooks: &okena_state::HooksConfig,
) -> Vec<TerminalCloseInfo> {
    let global_on_close = global_hooks.terminal.on_close.is_some();
    let ws = reactor.workspace.lock();
    exit_events
        .iter()
        .filter(|(tid, _)| !service_tids.contains(tid) && !hook_tids.contains(tid))
        .filter_map(|(tid, exit_code)| {
            let p = ws.find_project_for_terminal(tid)?;
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
            let terminal_name = p.terminal_names.get(tid).cloned();
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

    use okena_terminal::backend::LocalBackend;
    use okena_terminal::session_backend::SessionBackend;
    use okena_terminal::terminal::{Terminal, TerminalSize};
    use okena_workspace::state::WorkspaceData;

    fn terminal_size() -> TerminalSize {
        TerminalSize {
            cols: 80,
            rows: 24,
            cell_width: 8.0,
            cell_height: 16.0,
        }
    }

    fn test_reactor(workspace: Workspace, settings: AppSettings) -> PtyLoopReactor {
        let (workspace_tick, _wrx) = watch::channel(0u64);
        PtyLoopReactor {
            workspace: Arc::new(Mutex::new(workspace)),
            hook_runner: None,
            hook_monitor: Some(HookMonitor::new()),
            workspace_tick,
            settings: Arc::new(Mutex::new(settings)),
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
        let reactor = test_reactor(Workspace::new(WorkspaceData::empty()), AppSettings::default());

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
