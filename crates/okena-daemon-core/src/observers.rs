//! Observer reactor: the GPUI-free analogue of the app's `cx.observe`-driven
//! autosave / state-version / service-sync wiring.
//!
//! The GUI registers `cx.observe(&workspace, …)` / `cx.observe(&service_manager,
//! …)` closures that fire on every `notify`. The daemon has no entity graph, so
//! it converts each `notify` into a `watch` tick (see [`crate::reactor`]) and
//! drives the same behaviors from two long-lived tokio tasks that `await` those
//! ticks:
//!
//! 1. the **workspace-tick task** — bumps `state_version`, runs the debounced
//!    autosave, and runs the project→services load/unload diff
//!    ([`observe_project_services`] / `sync_services` in `okena-app`'s `app/mod.rs`).
//! 2. the **service-tick task** — bumps `state_version` and writes the per-project
//!    service terminal-id maps back into the workspace
//!    (`Workspace::sync_service_terminals`).
//!
//! ## Re-entrancy
//!
//! The write-back notifies the workspace → bumps `workspace_tick` → re-runs the
//! services diff → could bump `service_tick` → storm. Three guards defend against
//! this (all required, see [`spawn_observers`]):
//!
//! * **Coalescing ticks** — a `watch` channel collapses every bump made between
//!   two `changed()` polls into a single wakeup, so a burst is one pass.
//! * **Idempotent diffs** — both `sync_services` (guarded by the `known` set) and
//!   `Workspace::sync_service_terminals` (guarded by an equality check that only
//!   notifies on real change) are no-ops once converged, so the storm terminates.
//! * **Separate lock scopes** — a pass never holds the workspace mutex and the
//!   service-manager mutex at the same time: lock → snapshot → drop → lock the
//!   other.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use okena_services::config::{PreparedProjectConfig, prepare_project_config};
use okena_services::manager::{
    ServiceCx, ServiceLoadStatus, ServiceManager, ServiceTerminalWriteback,
};
use okena_workspace::persistence;

use crate::reactor::DaemonReactor;
use crate::service_cx::ServiceReactorRef;
use crate::workspace_cx::DaemonWorkspaceCx;

/// Debounce window before an autosave is flushed to disk. Mirrors the GUI's
/// 500ms timer in `app/mod.rs`.
const AUTOSAVE_DEBOUNCE: Duration = Duration::from_millis(500);
const SERVICE_RETRY_DELAY: Duration = Duration::from_millis(500);
const SERVICE_RETRY_MAX_DELAY: Duration = Duration::from_secs(30);

#[derive(Default)]
pub(crate) struct AutosaveTracker {
    pending: parking_lot::Mutex<usize>,
    drained: parking_lot::Condvar,
}

impl AutosaveTracker {
    pub(crate) fn start(self: &Arc<Self>) -> AutosaveJob {
        *self.pending.lock() += 1;
        AutosaveJob {
            tracker: Arc::clone(self),
        }
    }

    pub(crate) fn flush(&self) {
        let mut pending = self.pending.lock();
        while *pending != 0 {
            self.drained.wait(&mut pending);
        }
    }
}

pub(crate) struct AutosaveJob {
    tracker: Arc<AutosaveTracker>,
}

impl Drop for AutosaveJob {
    fn drop(&mut self) {
        let mut pending = self.tracker.pending.lock();
        *pending = pending.saturating_sub(1);
        if *pending == 0 {
            self.tracker.drained.notify_all();
        }
    }
}

/// Per-project snapshot taken under the workspace lock so the services diff can
/// run after the lock is dropped (the separate-lock-scope guard).
#[derive(Clone)]
struct ProjectSnapshot {
    id: String,
    path: String,
    is_creating: bool,
    is_closing: bool,
    service_terminals: std::collections::HashMap<String, String>,
    data_replacement_epoch: u64,
}

struct PreparedProjectSnapshot {
    project: ProjectSnapshot,
    config: Option<PreparedProjectConfig>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct KnownProject {
    path: String,
    data_replacement_epoch: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PendingPreservedSessions {
    identity: KnownProject,
    terminal_ids: HashSet<String>,
}

#[derive(Default)]
struct ServiceSyncState {
    known: HashMap<String, KnownProject>,
    pending_preserved: HashMap<String, PendingPreservedSessions>,
    discarded_terminal_ids: HashMap<String, HashSet<String>>,
    retry_projects: HashSet<String>,
    retry_deadline: Option<tokio::time::Instant>,
    retry_round: u32,
}

fn replace_pending_preserved_sessions(
    sync_state: &mut ServiceSyncState,
    project_id: &str,
    identity: &KnownProject,
    terminal_ids: HashSet<String>,
    sm: &ServiceManager,
) {
    if let Some(previous) = sync_state.pending_preserved.remove(project_id) {
        let abandoned: HashSet<String> = previous
            .terminal_ids
            .difference(&terminal_ids)
            .cloned()
            .collect();
        sm.kill_unclaimed_preserved_sessions(project_id, &abandoned);
    }
    if !terminal_ids.is_empty() {
        sync_state.pending_preserved.insert(
            project_id.to_string(),
            PendingPreservedSessions {
                identity: identity.clone(),
                terminal_ids,
            },
        );
    }
}

impl ServiceSyncState {
    fn request_retry(&mut self, project_id: &str) {
        self.retry_projects.insert(project_id.to_string());
        if self.retry_deadline.is_none() {
            let multiplier = 1u32
                .checked_shl(self.retry_round.min(16))
                .unwrap_or(u32::MAX);
            let delay = SERVICE_RETRY_DELAY
                .checked_mul(multiplier)
                .unwrap_or(SERVICE_RETRY_MAX_DELAY)
                .min(SERVICE_RETRY_MAX_DELAY);
            self.retry_deadline = Some(tokio::time::Instant::now() + delay);
        }
    }

    fn resolve_retry(&mut self, project_id: &str) {
        self.retry_projects.remove(project_id);
        if self.retry_projects.is_empty() {
            self.retry_deadline = None;
            self.retry_round = 0;
        }
    }
}

impl DaemonReactor {
    /// Spawn the two observer tasks onto the current `tokio::task::LocalSet`.
    ///
    /// They MUST be `spawn_local` (not `Handle::spawn`): the workspace-tick task
    /// drives `ServiceManager::load_project_services`, which can call
    /// [`ServiceCx::spawn_main`](okena_services::manager::ServiceCx::spawn_main)
    /// — and the daemon's `spawn_main` is `tokio::task::spawn_local`, which
    /// panics outside a `LocalSet`. The caller is responsible for running these
    /// inside `LocalSet::run_until` / `LocalSet::block_on` on a multi-thread
    /// runtime (the `spawn_blocking` offloads in autosave / the service async cx
    /// still reach the multi-thread pool via the held [`tokio::runtime::Handle`]).
    ///
    /// `spawn_local` does not require the futures to be `Send`, which matches the
    /// GUI's single-threaded main executor and lets the service tasks stay `!Send`.
    pub fn spawn_observers(&self) {
        // Subscribe to each tick *here*, synchronously, before spawning. A
        // `watch::Receiver` created now treats any bump made after this call as
        // "changed" — so a tick fired between `spawn_observers()` returning and
        // the spawned task first polling is not lost. (Subscribing inside the
        // task would race: `spawn_local` only schedules, so a bump that lands
        // before the task runs would be marked already-seen at subscribe time.)
        let workspace_rx = self.workspace_tick.subscribe();
        let autosave_rx = self.workspace_tick.subscribe();
        let service_rx = self.service_tick.subscribe();

        // Clone the shared bits here (synchronously) so the spawned futures own
        // them and capture no borrow of `self` — `spawn_local` requires `'static`.
        tokio::task::spawn_local(workspace_tick_task(
            workspace_rx,
            self.workspace.clone(),
            self.service_manager.clone(),
            self.state_version.clone(),
            self.service_tick.clone(),
            self.runtime.clone(),
        ));
        // Autosave runs on its OWN `workspace_tick` subscription so its debounce
        // window + blocking save never delay the `state_version` bump that
        // notifies clients. A worktree close fires several data-changing ticks
        // back-to-back; running the 500ms-debounced save inline in the tick loop
        // serialized them and stalled the client-visible removal by ~2s.
        tokio::task::spawn_local(autosave_task(
            autosave_rx,
            self.workspace.clone(),
            self.runtime.clone(),
            self.autosave_tracker.clone(),
        ));
        tokio::task::spawn_local(service_tick_task(
            service_rx,
            self.workspace.clone(),
            self.service_manager.clone(),
            self.state_version.clone(),
            self.workspace_tick.clone(),
            self.hook_runner.clone(),
            self.hook_monitor.clone(),
        ));
    }
}

type SharedWorkspace = Arc<parking_lot::Mutex<okena_workspace::state::Workspace>>;
type SharedServiceManager = Arc<parking_lot::Mutex<ServiceManager>>;

/// The workspace-tick observer task: bump `state_version` and run the
/// project→services load/unload diff on every `workspace_tick` change. Autosave
/// lives in a separate task ([`autosave_task`]) so its debounce never delays the
/// state_version bump.
async fn workspace_tick_task(
    mut tick_rx: tokio::sync::watch::Receiver<u64>,
    workspace: SharedWorkspace,
    service_manager: SharedServiceManager,
    state_version: tokio::sync::watch::Sender<u64>,
    service_tick: tokio::sync::watch::Sender<u64>,
    runtime: tokio::runtime::Handle,
) {
    let mut sync_state = ServiceSyncState::default();

    // Mirror the GUI's initial load: run one diff pass before awaiting ticks so
    // persisted projects get their services loaded at startup.
    run_services_sync(
        &workspace,
        &service_manager,
        &runtime,
        &service_tick,
        &mut sync_state,
    )
    .await;

    loop {
        let Some(workspace_changed) =
            wait_for_service_sync_trigger(&mut tick_rx, &mut sync_state.retry_deadline).await
        else {
            return;
        };

        if workspace_changed {
            // Retry-only passes do not represent a workspace mutation. Successful
            // service loads notify through `service_tick` on their own.
            state_version.send_modify(|v| *v += 1);
        } else {
            sync_state.retry_round = sync_state.retry_round.saturating_add(1);
        }

        // ── project → services load/unload diff ─────────────────────────────
        run_services_sync(
            &workspace,
            &service_manager,
            &runtime,
            &service_tick,
            &mut sync_state,
        )
        .await;
    }
}

/// Wait for either a real workspace mutation or the one deduplicated retry timer.
/// Returns `None` when the workspace tick sender is gone.
async fn wait_for_service_sync_trigger(
    tick_rx: &mut tokio::sync::watch::Receiver<u64>,
    retry_deadline: &mut Option<tokio::time::Instant>,
) -> Option<bool> {
    if let Some(deadline) = *retry_deadline {
        tokio::select! {
            changed = tick_rx.changed() => changed.ok().map(|_| true),
            _ = tokio::time::sleep_until(deadline) => {
                *retry_deadline = None;
                Some(false)
            }
        }
    } else {
        tick_rx.changed().await.ok().map(|_| true)
    }
}

/// The service-tick observer task: bump `state_version` and write the per-project
/// service terminal-id maps back into the workspace on every `service_tick`
/// change.
async fn service_tick_task(
    mut tick_rx: tokio::sync::watch::Receiver<u64>,
    workspace: SharedWorkspace,
    service_manager: SharedServiceManager,
    state_version: tokio::sync::watch::Sender<u64>,
    workspace_tick: tokio::sync::watch::Sender<u64>,
    hook_runner: Option<okena_hooks::HookRunner>,
    hook_monitor: Option<okena_hooks::HookMonitor>,
) {
    loop {
        if tick_rx.changed().await.is_err() {
            return;
        }

        state_version.send_modify(|v| *v += 1);

        // ── services → workspace terminal-id write-back ─────────────────────
        //
        // Lock scope 1: snapshot the per-project terminal-id maps under the
        // service-manager lock, then DROP it.
        let writebacks = service_manager.lock().service_terminal_writebacks();

        // Lock scope 2: write the maps back under the workspace lock.
        // `sync_service_terminals` only notifies when a map actually changes, so
        // once converged this stops bumping `workspace_tick` and the cross-tick
        // storm terminates.
        apply_service_terminal_writebacks(
            &workspace,
            &workspace_tick,
            &hook_runner,
            &hook_monitor,
            writebacks,
        );
    }
}

fn apply_service_terminal_writebacks(
    workspace: &SharedWorkspace,
    workspace_tick: &tokio::sync::watch::Sender<u64>,
    hook_runner: &Option<okena_hooks::HookRunner>,
    hook_monitor: &Option<okena_hooks::HookMonitor>,
    writebacks: Vec<ServiceTerminalWriteback>,
) {
    let mut ws = workspace.lock();
    if ws.terminal_backend_migration_epoch().is_some() {
        return;
    }
    let current_epoch = ws.data_replacement_epoch();
    let mut cx = DaemonWorkspaceCx::new(workspace_tick, hook_runner, hook_monitor);
    for writeback in writebacks {
        if writeback.data_replacement_epoch != current_epoch
            || ws
                .project(&writeback.project_id)
                .is_none_or(|project| project.path != writeback.project_path)
        {
            continue;
        }
        ws.sync_service_terminals(&writeback.project_id, writeback.terminal_ids, &mut cx);
    }
}

/// Dedicated autosave task, driven by the same `workspace_tick` as
/// [`workspace_tick_task`] but on its OWN subscription. Kept separate so the
/// debounce sleep + blocking save I/O never delay the latency-critical
/// `state_version` bump: a worktree close fires several data-changing ticks
/// back-to-back, and running the 500ms-debounced save inline serialized them,
/// stalling the client-visible removal by ~2s.
async fn autosave_task(
    mut tick_rx: tokio::sync::watch::Receiver<u64>,
    workspace: SharedWorkspace,
    runtime: tokio::runtime::Handle,
    tracker: Arc<AutosaveTracker>,
) {
    // Tracks the `data_version` last persisted, so UI-only changes skip the save.
    let last_saved_version = Arc::new(AtomicU64::new(0));
    loop {
        if tick_rx.changed().await.is_err() {
            // All senders dropped — the reactor is gone; stop the task.
            return;
        }
        autosave(&workspace, &runtime, &last_saved_version, &tracker).await;
    }
}

/// Debounced autosave pass. Skips the save when `data_version` is unchanged
/// since the last persisted version (UI-only change); otherwise waits the
/// debounce window, re-snapshots under a short lock, and runs the blocking
/// `save_workspace` on the multi-thread runtime. Mirrors `app/mod.rs`'s
/// 500ms-debounced save observer.
async fn autosave(
    workspace: &SharedWorkspace,
    runtime: &tokio::runtime::Handle,
    last_saved_version: &Arc<AtomicU64>,
    tracker: &Arc<AutosaveTracker>,
) {
    // Skip UI-only changes: the persistent `data_version` is unchanged.
    let current_version = {
        let workspace = workspace.lock();
        if workspace.terminal_backend_migration_epoch().is_some() {
            return;
        }
        workspace.data_version()
    };
    if current_version == last_saved_version.load(Ordering::Relaxed) {
        return;
    }

    // Debounce: a burst of mutations collapses into one save after the window.
    tokio::time::sleep(AUTOSAVE_DEBOUNCE).await;

    // Re-snapshot after the sleep — the version may have moved again; take the
    // latest under a short lock and DROP it before the blocking I/O.
    let (data, version) = {
        let ws = workspace.lock();
        if ws.terminal_backend_migration_epoch().is_some() {
            return;
        }
        (ws.data().clone(), ws.data_version())
    };

    // Blocking fs I/O — offload onto the multi-thread runtime so it never stalls
    // the LocalSet thread (Windows AV / OneDrive can stall workspace.json saves).
    let job = tracker.start();
    let save_result = runtime
        .spawn_blocking(move || {
            let _job = job;
            persistence::save_workspace(&data)
        })
        .await;

    match save_result {
        Ok(Ok(())) => {
            last_saved_version.store(version, Ordering::Relaxed);
        }
        Ok(Err(e)) => {
            log::error!("Failed to save workspace: {}", e);
            // Don't update last_saved_version — the next mutation retries.
        }
        Err(e) => {
            log::error!("Workspace save task panicked: {}", e);
        }
    }
}

/// Run one project→services load/unload diff pass with separate lock scopes.
///
/// Lock scope 1: snapshot the project list under the workspace lock, then DROP
/// it. Lock scope 2: lock the service manager, build a
/// [`DaemonServiceCx`](crate::service_cx::DaemonServiceCx), and run
/// [`sync_services`].
async fn run_services_sync(
    workspace: &SharedWorkspace,
    service_manager: &SharedServiceManager,
    runtime: &tokio::runtime::Handle,
    service_tick: &tokio::sync::watch::Sender<u64>,
    sync_state: &mut ServiceSyncState,
) {
    run_services_sync_with_preparer(
        workspace,
        service_manager,
        runtime,
        service_tick,
        sync_state,
        prepare_project_snapshots,
    )
    .await;
}

async fn run_services_sync_with_preparer<Prepare>(
    workspace: &SharedWorkspace,
    service_manager: &SharedServiceManager,
    runtime: &tokio::runtime::Handle,
    service_tick: &tokio::sync::watch::Sender<u64>,
    sync_state: &mut ServiceSyncState,
    prepare: Prepare,
) where
    Prepare: FnOnce(Vec<ProjectSnapshot>) -> Vec<PreparedProjectSnapshot> + Send + 'static,
{
    // Lock scope 1: snapshot the projects, then drop the workspace lock.
    let (projects, snapshot_epoch): (Vec<ProjectSnapshot>, u64) = {
        let ws = workspace.lock();
        if ws.terminal_backend_migration_epoch().is_some() {
            return;
        }
        let data_replacement_epoch = ws.data_replacement_epoch();
        (
            ws.data()
                .projects
                .iter()
                .map(|p| ProjectSnapshot {
                    id: p.id.clone(),
                    path: p.path.clone(),
                    is_creating: p.is_creating,
                    is_closing: p.is_closing,
                    service_terminals: p.service_terminals.clone(),
                    data_replacement_epoch,
                })
                .collect(),
            data_replacement_epoch,
        )
    };

    let expected_project_count = projects.len();
    let prepared_projects = match runtime.spawn_blocking(move || prepare(projects)).await {
        Ok(prepared) => prepared,
        Err(error) => {
            log::error!("service config preparation task failed: {error}");
            return;
        }
    };

    // Loading ran without locks. Reject results whose workspace owner changed
    // while the filesystem was being probed.
    let prepared_projects = {
        let ws = workspace.lock();
        let epoch = ws.data_replacement_epoch();
        prepared_projects
            .into_iter()
            .filter(|prepared| {
                prepared.project.data_replacement_epoch == epoch
                    && ws.project(&prepared.project.id).is_some_and(|project| {
                        project.path == prepared.project.path
                            && project.is_creating == prepared.project.is_creating
                            && project.is_closing == prepared.project.is_closing
                    })
            })
            .collect::<Vec<_>>()
    };
    if prepared_projects.len() != expected_project_count {
        return;
    }
    {
        let workspace = workspace.lock();
        if workspace.terminal_backend_migration_epoch().is_some()
            || workspace.data_replacement_epoch() != snapshot_epoch
        {
            return;
        }
    }

    // Lock scope 2: lock the service manager, mint a top-level cx, run the diff.
    // `spawn_main` from inside the loaded services lands on the active LocalSet
    // (the spawn_observers contract), so this must run on the LocalSet thread.
    let reactor_ref = ServiceReactorRef::new(
        service_manager.clone(),
        runtime.clone(),
        service_tick.clone(),
    );
    let mut sm = service_manager.lock();
    let mut cx = reactor_ref.cx();
    sync_prepared_services(prepared_projects, sync_state, &mut sm, &mut cx);
}

fn prepare_project_snapshots(projects: Vec<ProjectSnapshot>) -> Vec<PreparedProjectSnapshot> {
    projects
        .into_iter()
        .map(|project| {
            let config = (!project.is_creating && !project.is_closing)
                .then(|| prepare_project_config(&project.path));
            PreparedProjectSnapshot { project, config }
        })
        .collect()
}

/// GPUI-free port of `okena-app`'s `app/mod.rs::sync_services`: diff the current
/// non-remote, on-disk project set against `known` and load/unload service
/// configs accordingly. The convergence key includes the workspace replacement
/// epoch and service-owning project data, so loading a session cannot preserve a
/// stale manager merely because the new project reused the same id.
fn sync_prepared_services(
    projects: Vec<PreparedProjectSnapshot>,
    sync_state: &mut ServiceSyncState,
    sm: &mut ServiceManager,
    cx: &mut impl ServiceCx,
) {
    let current_ids: HashSet<String> = projects
        .iter()
        .map(|prepared| prepared.project.id.clone())
        .collect();

    for prepared in projects {
        let p = prepared.project;
        if p.is_creating || p.is_closing {
            continue;
        }
        let config = prepared
            .config
            .expect("active project snapshots must have prepared service config");
        let identity = KnownProject {
            path: p.path.clone(),
            data_replacement_epoch: p.data_replacement_epoch,
        };
        let saved_ids: HashSet<String> = p.service_terminals.values().cloned().collect();
        if let Some(discarded) = sync_state.discarded_terminal_ids.get_mut(&p.id) {
            discarded.retain(|terminal_id| saved_ids.contains(terminal_id));
            if discarded.is_empty() {
                sync_state.discarded_terminal_ids.remove(&p.id);
            }
        }
        let discarded = sync_state
            .discarded_terminal_ids
            .get(&p.id)
            .cloned()
            .unwrap_or_default();
        let saved_for_attempt: HashMap<String, String> = p
            .service_terminals
            .iter()
            .filter(|(_, terminal_id)| !discarded.contains(*terminal_id))
            .map(|(service, terminal_id)| (service.clone(), terminal_id.clone()))
            .collect();
        let saved_ids_for_attempt: HashSet<String> = saved_for_attempt.values().cloned().collect();
        let path_exists = !matches!(&config, PreparedProjectConfig::Missing);

        if sync_state
            .pending_preserved
            .get(&p.id)
            .is_some_and(|pending| {
                pending.identity != identity || pending.terminal_ids != saved_ids_for_attempt
            })
        {
            replace_pending_preserved_sessions(
                sync_state,
                &p.id,
                &identity,
                saved_ids_for_attempt.clone(),
                sm,
            );
        }

        let previous = sync_state.known.get(&p.id).cloned();
        let manager_owns_renamed_project = path_exists
            && previous.as_ref().is_some_and(|previous| {
                previous.data_replacement_epoch == identity.data_replacement_epoch
                    && previous.path != identity.path
            })
            && sm.project_path(&p.id) == Some(&p.path)
            && sm.service_terminal_writebacks().iter().any(|writeback| {
                writeback.project_id == p.id
                    && writeback.project_path == p.path
                    && writeback.data_replacement_epoch == p.data_replacement_epoch
            });
        if manager_owns_renamed_project {
            sync_state.known.insert(p.id.clone(), identity);
            sync_state.resolve_retry(&p.id);
            continue;
        }
        if path_exists
            && previous.as_ref() == Some(&identity)
            && sm.project_path(&p.id) == Some(&p.path)
        {
            sync_state.resolve_retry(&p.id);
            continue;
        }
        if previous.is_some() || sm.project_path(&p.id).is_some() {
            let replacing_data = previous.as_ref().is_some_and(|previous| {
                previous.data_replacement_epoch != identity.data_replacement_epoch
            });
            if replacing_data || !path_exists {
                sm.unload_project_services_preserving(&p.id, &saved_ids_for_attempt, cx);
                replace_pending_preserved_sessions(
                    sync_state,
                    &p.id,
                    &identity,
                    saved_ids_for_attempt.clone(),
                    sm,
                );
            } else {
                sm.unload_project_services(&p.id, cx);
            }
            sync_state.known.remove(&p.id);
        }

        if !path_exists {
            if !saved_ids_for_attempt.is_empty() {
                replace_pending_preserved_sessions(
                    sync_state,
                    &p.id,
                    &identity,
                    saved_ids_for_attempt,
                    sm,
                );
            }
            sync_state.request_retry(&p.id);
            continue;
        }

        sm.set_project_writeback_owner(&p.id, &p.path, p.data_replacement_epoch);
        let load_status =
            sm.load_project_services_prepared(&p.id, &p.path, &saved_for_attempt, config, cx);
        let claimed: HashSet<String> = sm.service_terminal_ids(&p.id).into_values().collect();
        let unclaimed: HashSet<String> = saved_ids_for_attempt
            .difference(&claimed)
            .cloned()
            .collect();
        sm.kill_unclaimed_preserved_sessions(&p.id, &saved_ids_for_attempt);
        if !unclaimed.is_empty() {
            sync_state
                .discarded_terminal_ids
                .entry(p.id.clone())
                .or_default()
                .extend(unclaimed);
        }
        sync_state.pending_preserved.remove(&p.id);

        match load_status {
            ServiceLoadStatus::Loaded => {
                sync_state.known.insert(p.id.clone(), identity);
                sync_state.resolve_retry(&p.id);
            }
            ServiceLoadStatus::Failed => {
                sync_state.known.remove(&p.id);
                sync_state.request_retry(&p.id);
            }
        }
    }

    let removed: HashSet<String> = sync_state
        .known
        .keys()
        .chain(sync_state.pending_preserved.keys())
        .chain(sync_state.retry_projects.iter())
        .filter(|id| !current_ids.contains(*id))
        .cloned()
        .collect();
    for id in &removed {
        if let Some(pending) = sync_state.pending_preserved.remove(id) {
            sm.kill_unclaimed_preserved_sessions(id, &pending.terminal_ids);
        }
        sm.unload_project_services(id, cx);
        sync_state.known.remove(id);
        sync_state.discarded_terminal_ids.remove(id);
        sync_state.resolve_retry(id);
    }
}

#[cfg(test)]
fn sync_services(
    projects: &[ProjectSnapshot],
    sync_state: &mut ServiceSyncState,
    sm: &mut ServiceManager,
    cx: &mut impl ServiceCx,
) {
    sync_prepared_services(
        prepare_project_snapshots(projects.to_vec()),
        sync_state,
        sm,
        cx,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{StubBackend, StubTransport, empty_workspace_data};
    use okena_terminal::backend::TerminalBackend;
    use okena_terminal::shell_config::ShellType;
    use okena_terminal::terminal::TerminalTransport;
    use std::sync::Mutex;
    use std::sync::atomic::AtomicBool;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct RecordingBackend {
        killed: Mutex<Vec<String>>,
        reconnected: Mutex<Vec<String>>,
        reconnect_fail_after: Option<usize>,
        reconnect_count: AtomicUsize,
    }

    impl TerminalBackend for RecordingBackend {
        fn transport(&self) -> Arc<dyn TerminalTransport> {
            Arc::new(StubTransport)
        }

        fn create_terminal(
            &self,
            _cwd: &str,
            _shell: Option<&ShellType>,
        ) -> anyhow::Result<String> {
            anyhow::bail!("unexpected create")
        }

        fn reconnect_terminal(
            &self,
            terminal_id: &str,
            _cwd: &str,
            _shell: Option<&ShellType>,
        ) -> anyhow::Result<String> {
            self.reconnected
                .lock()
                .expect("reconnect lock")
                .push(terminal_id.to_string());
            let reconnect_count = self.reconnect_count.fetch_add(1, Ordering::Relaxed);
            if self
                .reconnect_fail_after
                .is_some_and(|limit| reconnect_count >= limit)
            {
                anyhow::bail!("configured reconnect failure");
            }
            Ok(terminal_id.to_string())
        }

        fn kill(&self, terminal_id: &str) {
            self.killed
                .lock()
                .expect("kill lock")
                .push(terminal_id.to_string());
        }

        fn capture_buffer(&self, _terminal_id: &str) -> Option<std::path::PathBuf> {
            None
        }

        fn supports_buffer_capture(&self) -> bool {
            false
        }

        fn is_remote(&self) -> bool {
            false
        }

        fn get_shell_pid(&self, _terminal_id: &str) -> Option<u32> {
            None
        }

        fn get_service_pids(&self, _terminal_id: &str) -> Vec<u32> {
            Vec::new()
        }
    }

    struct BlockingReconnectBackend {
        started: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
        release: Mutex<std::sync::mpsc::Receiver<()>>,
        killed: Mutex<Vec<String>>,
    }

    impl TerminalBackend for BlockingReconnectBackend {
        fn transport(&self) -> Arc<dyn TerminalTransport> {
            Arc::new(StubTransport)
        }

        fn create_terminal(
            &self,
            _cwd: &str,
            _shell: Option<&ShellType>,
        ) -> anyhow::Result<String> {
            anyhow::bail!("reserved-ID launches must reconnect")
        }

        fn reconnect_terminal(
            &self,
            terminal_id: &str,
            _cwd: &str,
            _shell: Option<&ShellType>,
        ) -> anyhow::Result<String> {
            if let Some(started) = self.started.lock().expect("started lock").take() {
                let _ = started.send(());
            }
            self.release
                .lock()
                .expect("release lock")
                .recv()
                .map_err(|error| anyhow::anyhow!(error.to_string()))?;
            Ok(terminal_id.to_string())
        }

        fn kill(&self, terminal_id: &str) {
            self.killed
                .lock()
                .expect("kill lock")
                .push(terminal_id.to_string());
        }

        fn capture_buffer(&self, _terminal_id: &str) -> Option<std::path::PathBuf> {
            None
        }

        fn supports_buffer_capture(&self) -> bool {
            false
        }

        fn is_remote(&self) -> bool {
            false
        }

        fn get_shell_pid(&self, _terminal_id: &str) -> Option<u32> {
            None
        }

        fn get_service_pids(&self, _terminal_id: &str) -> Vec<u32> {
            Vec::new()
        }
    }

    /// A `known`-set + project snapshot fixture for the diff logic.
    fn project(id: &str, path: &str) -> ProjectSnapshot {
        ProjectSnapshot {
            id: id.to_string(),
            path: path.to_string(),
            is_creating: false,
            is_closing: false,
            service_terminals: Default::default(),
            data_replacement_epoch: 0,
        }
    }

    fn workspace_with_service_terminal(
        project_id: &str,
        project_path: &str,
        service_name: &str,
        terminal_id: &str,
    ) -> okena_workspace::state::Workspace {
        workspace_with_project(
            project_id,
            project_path,
            HashMap::from([(service_name.to_string(), terminal_id.to_string())]),
        )
    }

    fn workspace_with_project(
        project_id: &str,
        project_path: &str,
        service_terminals: HashMap<String, String>,
    ) -> okena_workspace::state::Workspace {
        use okena_workspace::state::ProjectData;

        let mut data = empty_workspace_data();
        data.project_order.push(project_id.to_string());
        data.projects.push(ProjectData {
            id: project_id.to_string(),
            name: "Project".into(),
            path: project_path.to_string(),
            layout: None,
            terminal_names: Default::default(),
            hidden_terminals: Default::default(),
            worktree_info: None,
            worktree_ids: Vec::new(),
            folder_color: Default::default(),
            hooks: Default::default(),
            connection_id: None,
            service_terminals,
            default_shell: None,
            hook_terminals: Default::default(),
            pinned: false,
            last_activity_at: None,
            is_creating: false,
            is_closing: false,
            creating_progress: None,
        });
        okena_workspace::state::Workspace::new(data)
    }

    /// The on-disk path used for "exists" projects in the diff tests — the crate
    /// dir always exists, so the deferred-worktree skip is not triggered.
    fn existing_path() -> String {
        env!("CARGO_MANIFEST_DIR").to_string()
    }

    /// A `ServiceManager` with a stub backend. Load is a no-op when there is no
    /// `okena.yaml` / docker-compose, so the diff's `known`-set bookkeeping is
    /// what the tests assert.
    fn manager() -> ServiceManager {
        let backend = Arc::new(StubBackend);
        let terminals = Arc::new(parking_lot::Mutex::new(Default::default()));
        ServiceManager::new(backend, terminals)
    }

    /// Build a top-level `DaemonServiceCx` over a throwaway reactor for tests
    /// that need to pass a `cx` into `sync_services`. The notify just bumps a
    /// detached watch channel.
    fn reactor_ref(
        manager: &std::sync::Arc<parking_lot::Mutex<ServiceManager>>,
    ) -> ServiceReactorRef {
        let (tick, _rx) = tokio::sync::watch::channel(0u64);
        ServiceReactorRef::new(manager.clone(), tokio::runtime::Handle::current(), tick)
    }

    async fn wait_for_test_condition(condition: impl Fn() -> bool) {
        tokio::time::timeout(Duration::from_secs(2), async {
            while !condition() {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .expect("service task condition timed out");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn blocking_service_preparation_keeps_localset_live_and_discards_stale_snapshot() {
        let workspace = Arc::new(parking_lot::Mutex::new(workspace_with_project(
            "project",
            "/captured",
            HashMap::new(),
        )));
        let sm = Arc::new(parking_lot::Mutex::new(manager()));
        let (service_tick, _service_rx) = tokio::sync::watch::channel(0u64);
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let progressed = Arc::new(AtomicBool::new(false));
        let progress_manager = sm.clone();
        let local = tokio::task::LocalSet::new();

        local
            .run_until(async {
                let task_workspace = workspace.clone();
                let task_manager = sm.clone();
                let task_tick = service_tick.clone();
                let runtime = tokio::runtime::Handle::current();
                let task = tokio::task::spawn_local(async move {
                    let mut sync_state = ServiceSyncState::default();
                    run_services_sync_with_preparer(
                        &task_workspace,
                        &task_manager,
                        &runtime,
                        &task_tick,
                        &mut sync_state,
                        move |projects| {
                            let _ = started_tx.send(());
                            release_rx.recv().expect("release preparation");
                            projects
                                .into_iter()
                                .map(|project| PreparedProjectSnapshot {
                                    project,
                                    config: Some(PreparedProjectConfig::Loaded {
                                        config: None,
                                        detected_compose_file: None,
                                    }),
                                })
                                .collect()
                        },
                    )
                    .await;
                });

                started_rx.await.expect("preparation started");
                let progressed_task = progressed.clone();
                tokio::task::spawn_local(async move {
                    let _guard = progress_manager.lock();
                    progressed_task.store(true, Ordering::Release);
                })
                .await
                .expect("local task completed");
                *workspace.lock() =
                    workspace_with_project("project", "/replacement", HashMap::new());
                release_tx.send(()).expect("release preparation");
                task.await.expect("sync task completed");
            })
            .await;

        assert!(progressed.load(Ordering::Acquire));
        assert!(sm.lock().project_path("project").is_none());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn blocking_service_preparation_discards_lifecycle_change() {
        let workspace = Arc::new(parking_lot::Mutex::new(workspace_with_project(
            "project",
            "/captured",
            HashMap::new(),
        )));
        let sm = Arc::new(parking_lot::Mutex::new(manager()));
        let (service_tick, _service_rx) = tokio::sync::watch::channel(0u64);
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let local = tokio::task::LocalSet::new();

        local
            .run_until(async {
                let task_workspace = workspace.clone();
                let task_manager = sm.clone();
                let task_tick = service_tick.clone();
                let runtime = tokio::runtime::Handle::current();
                let task = tokio::task::spawn_local(async move {
                    let mut sync_state = ServiceSyncState::default();
                    run_services_sync_with_preparer(
                        &task_workspace,
                        &task_manager,
                        &runtime,
                        &task_tick,
                        &mut sync_state,
                        move |projects| {
                            let _ = started_tx.send(());
                            release_rx.recv().expect("release preparation");
                            projects
                                .into_iter()
                                .map(|project| PreparedProjectSnapshot {
                                    project,
                                    config: Some(PreparedProjectConfig::Loaded {
                                        config: None,
                                        detected_compose_file: None,
                                    }),
                                })
                                .collect()
                        },
                    )
                    .await;
                });

                started_rx.await.expect("preparation started");
                workspace
                    .lock()
                    .mark_closing_project_authoritative("project");
                release_tx.send(()).expect("release preparation");
                task.await.expect("sync task completed");
            })
            .await;

        assert!(sm.lock().project_path("project").is_none());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn service_sync_is_suppressed_during_backend_migration() {
        let mut workspace_value =
            workspace_with_project("project", &existing_path(), HashMap::new());
        workspace_value
            .begin_terminal_backend_migration(
                okena_terminal::session_backend::SessionBackend::None,
                &ShellType::Default,
            )
            .expect("begin migration");
        let workspace = Arc::new(parking_lot::Mutex::new(workspace_value));
        let sm = Arc::new(parking_lot::Mutex::new(manager()));
        let (service_tick, _service_rx) = tokio::sync::watch::channel(0u64);
        let preparer_called = Arc::new(AtomicBool::new(false));
        let preparer_flag = preparer_called.clone();
        let mut sync_state = ServiceSyncState::default();

        run_services_sync_with_preparer(
            &workspace,
            &sm,
            &tokio::runtime::Handle::current(),
            &service_tick,
            &mut sync_state,
            move |_| {
                preparer_flag.store(true, Ordering::Release);
                Vec::new()
            },
        )
        .await;

        assert!(!preparer_called.load(Ordering::Acquire));
        assert!(sm.lock().project_path("project").is_none());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn autosave_is_suppressed_during_backend_migration() {
        let (workspace_tick, _workspace_rx) = tokio::sync::watch::channel(0u64);
        let no_hook_runner = None;
        let no_hook_monitor = None;
        let mut workspace_value =
            workspace_with_project("project", &existing_path(), HashMap::new());
        let mut cx = DaemonWorkspaceCx::new(&workspace_tick, &no_hook_runner, &no_hook_monitor);
        workspace_value.notify_data(&mut cx);
        workspace_value
            .begin_terminal_backend_migration(
                okena_terminal::session_backend::SessionBackend::None,
                &ShellType::Default,
            )
            .expect("begin migration");
        let workspace = Arc::new(parking_lot::Mutex::new(workspace_value));
        let last_saved_version = Arc::new(AtomicU64::new(0));
        let tracker = Arc::new(AutosaveTracker::default());

        autosave(
            &workspace,
            &tokio::runtime::Handle::current(),
            &last_saved_version,
            &tracker,
        )
        .await;

        assert_eq!(last_saved_version.load(Ordering::Relaxed), 0);
        assert_eq!(*tracker.pending.lock(), 0);
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn missing_empty_project_retries_when_mount_appears() {
        let project_dir = std::env::temp_dir().join(format!(
            "okena-observer-empty-mount-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let project_path = project_dir.to_string_lossy().into_owned();
        let workspace = Arc::new(parking_lot::Mutex::new(workspace_with_project(
            "project",
            &project_path,
            HashMap::new(),
        )));
        let sm = Arc::new(parking_lot::Mutex::new(manager()));
        let (service_tick, _service_rx) = tokio::sync::watch::channel(0u64);
        let runtime = tokio::runtime::Handle::current();
        let mut sync_state = ServiceSyncState::default();

        run_services_sync(&workspace, &sm, &runtime, &service_tick, &mut sync_state).await;
        assert!(sync_state.retry_projects.contains("project"));
        assert!(sync_state.retry_deadline.is_some());

        std::fs::create_dir_all(&project_dir).expect("create mounted project");
        std::fs::write(project_dir.join("okena.yaml"), "services: []\n")
            .expect("write service config");
        tokio::time::advance(SERVICE_RETRY_DELAY).await;
        let (_workspace_tick, mut workspace_rx) = tokio::sync::watch::channel(0u64);
        assert_eq!(
            wait_for_service_sync_trigger(&mut workspace_rx, &mut sync_state.retry_deadline,).await,
            Some(false)
        );
        sync_state.retry_round += 1;
        run_services_sync(&workspace, &sm, &runtime, &service_tick, &mut sync_state).await;

        assert_eq!(sm.lock().project_path("project"), Some(&project_path));
        assert!(!sync_state.retry_projects.contains("project"));
        std::fs::remove_dir_all(project_dir).expect("remove mounted project");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn blocking_service_launch_releases_manager_and_discards_stale_result() {
        let project_dir = std::env::temp_dir().join(format!(
            "okena-observer-blocking-launch-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&project_dir).expect("create project dir");
        std::fs::write(
            project_dir.join("okena.yaml"),
            "services:\n  - name: web\n    command: echo web\n",
        )
        .expect("write service config");
        let project_path = project_dir.to_string_lossy().into_owned();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let backend = Arc::new(BlockingReconnectBackend {
            started: Mutex::new(Some(started_tx)),
            release: Mutex::new(release_rx),
            killed: Mutex::new(Vec::new()),
        });
        let terminals = Arc::new(parking_lot::Mutex::new(Default::default()));
        let sm = Arc::new(parking_lot::Mutex::new(ServiceManager::new(
            backend.clone(),
            terminals,
        )));
        let rr = reactor_ref(&sm);
        let local = tokio::task::LocalSet::new();

        local
            .run_until(async {
                let mut snapshot = project("project", &project_path);
                snapshot
                    .service_terminals
                    .insert("web".into(), "persistent-web".into());
                let mut sync_state = ServiceSyncState::default();
                {
                    let mut manager = sm.lock();
                    let mut cx = rr.cx();
                    sync_services(&[snapshot], &mut sync_state, &mut manager, &mut cx);
                }
                started_rx.await.expect("backend launch started");
                {
                    let mut manager = sm.lock();
                    let mut cx = rr.cx();
                    manager.unload_project_services("project", &mut cx);
                }
                release_tx.send(()).expect("release backend launch");
                wait_for_test_condition(|| !backend.killed.lock().expect("kill lock").is_empty())
                    .await;
            })
            .await;

        assert!(sm.lock().service_terminal_ids("project").is_empty());
        assert!(
            backend
                .killed
                .lock()
                .expect("kill lock")
                .iter()
                .any(|terminal_id| terminal_id == "persistent-web")
        );
        std::fs::remove_dir_all(project_dir).expect("remove project dir");
    }

    #[test]
    fn service_writeback_is_epoch_fenced_and_can_clear_zero_instance_ownership() {
        let workspace = Arc::new(parking_lot::Mutex::new(workspace_with_service_terminal(
            "project",
            "/project",
            "web",
            "incoming-terminal",
        )));
        let (workspace_tick, _rx) = tokio::sync::watch::channel(0u64);

        apply_service_terminal_writebacks(
            &workspace,
            &workspace_tick,
            &None,
            &None,
            vec![ServiceTerminalWriteback {
                project_id: "project".into(),
                project_path: "/project".into(),
                data_replacement_epoch: 1,
                terminal_ids: HashMap::from([("web".into(), "stale-terminal".into())]),
            }],
        );
        assert_eq!(
            workspace
                .lock()
                .project("project")
                .unwrap()
                .service_terminals,
            HashMap::from([("web".into(), "incoming-terminal".into())])
        );

        apply_service_terminal_writebacks(
            &workspace,
            &workspace_tick,
            &None,
            &None,
            vec![ServiceTerminalWriteback {
                project_id: "project".into(),
                project_path: "/project".into(),
                data_replacement_epoch: 0,
                terminal_ids: HashMap::new(),
            }],
        );
        assert!(
            workspace
                .lock()
                .project("project")
                .unwrap()
                .service_terminals
                .is_empty()
        );
    }

    #[test]
    fn service_writeback_is_suppressed_during_backend_migration() {
        let mut workspace_value =
            workspace_with_service_terminal("project", "/project", "web", "owned-terminal");
        let migration_epoch = workspace_value
            .begin_terminal_backend_migration(
                okena_terminal::session_backend::SessionBackend::None,
                &ShellType::Default,
            )
            .expect("begin migration");
        let migration_epoch = migration_epoch.epoch;
        let workspace = Arc::new(parking_lot::Mutex::new(workspace_value));
        let (workspace_tick, _rx) = tokio::sync::watch::channel(0u64);

        apply_service_terminal_writebacks(
            &workspace,
            &workspace_tick,
            &None,
            &None,
            vec![ServiceTerminalWriteback {
                project_id: "project".into(),
                project_path: "/project".into(),
                data_replacement_epoch: migration_epoch,
                terminal_ids: HashMap::from([("web".into(), "old-backend".into())]),
            }],
        );

        assert!(
            workspace
                .lock()
                .project("project")
                .expect("project")
                .service_terminals
                .is_empty()
        );
    }

    #[tokio::test]
    async fn due_service_retry_is_an_autonomous_sync_trigger() {
        let (_tick, mut tick_rx) = tokio::sync::watch::channel(0u64);
        let mut deadline = Some(tokio::time::Instant::now());

        assert_eq!(
            wait_for_service_sync_trigger(&mut tick_rx, &mut deadline).await,
            Some(false)
        );
        assert!(deadline.is_none());
    }

    #[tokio::test]
    async fn sync_services_loads_new_projects_and_tracks_them() {
        let sm = std::sync::Arc::new(parking_lot::Mutex::new(manager()));
        let rr = reactor_ref(&sm);

        let projects = vec![project("local", &existing_path())];
        let mut sync_state = ServiceSyncState::default();

        {
            let mut guard = sm.lock();
            let mut cx = rr.cx();
            sync_services(&projects, &mut sync_state, &mut guard, &mut cx);
        }

        assert!(sync_state.known.contains_key("local"));
    }

    #[tokio::test]
    async fn sync_services_skips_nonexistent_paths() {
        let sm = std::sync::Arc::new(parking_lot::Mutex::new(manager()));
        let rr = reactor_ref(&sm);

        let projects = vec![project("ghost", "/path/that/does/not/exist/okena")];
        let mut sync_state = ServiceSyncState::default();

        {
            let mut guard = sm.lock();
            let mut cx = rr.cx();
            sync_services(&projects, &mut sync_state, &mut guard, &mut cx);
        }

        // Deferred worktree (missing dir) is NOT tracked, so a later pass retries.
        assert!(!sync_state.known.contains_key("ghost"));
    }

    #[tokio::test]
    async fn sync_services_unloads_removed_projects() {
        let sm = std::sync::Arc::new(parking_lot::Mutex::new(manager()));
        let rr = reactor_ref(&sm);

        // Pass 1: load a local project.
        let mut sync_state = ServiceSyncState::default();
        {
            let projects = vec![project("local", &existing_path())];
            let mut guard = sm.lock();
            let mut cx = rr.cx();
            sync_services(&projects, &mut sync_state, &mut guard, &mut cx);
        }
        assert!(sync_state.known.contains_key("local"));

        // Pass 2: the project is gone from the workspace → it is unloaded.
        {
            let projects: Vec<ProjectSnapshot> = vec![];
            let mut guard = sm.lock();
            let mut cx = rr.cx();
            sync_services(&projects, &mut sync_state, &mut guard, &mut cx);
        }
        assert!(!sync_state.known.contains_key("local"));
    }

    #[tokio::test]
    async fn sync_services_suspends_closing_known_project_without_manager() {
        let sm = std::sync::Arc::new(parking_lot::Mutex::new(manager()));
        let rr = reactor_ref(&sm);
        let path = existing_path();
        let mut closing = project("local", &path);
        closing.is_closing = true;
        let identity = KnownProject {
            path,
            data_replacement_epoch: 0,
        };
        let mut sync_state = ServiceSyncState {
            known: HashMap::from([("local".to_string(), identity.clone())]),
            ..ServiceSyncState::default()
        };

        {
            let mut guard = sm.lock();
            let mut cx = rr.cx();
            sync_services(&[closing], &mut sync_state, &mut guard, &mut cx);
        }

        assert_eq!(sync_state.known.get("local"), Some(&identity));
        assert!(!sync_state.retry_projects.contains("local"));
        assert!(sm.lock().project_path("local").is_none());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn sync_services_suspends_lifecycle_then_resumes_once() {
        let project_dir = std::env::temp_dir().join(format!(
            "okena-observer-lifecycle-suspend-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&project_dir).expect("create project dir");
        std::fs::write(
            project_dir.join("okena.yaml"),
            "services:\n  - name: web\n    command: echo web\n",
        )
        .expect("write service config");
        let project_path = project_dir.to_string_lossy().into_owned();
        let backend = Arc::new(RecordingBackend {
            killed: Mutex::new(Vec::new()),
            reconnected: Mutex::new(Vec::new()),
            reconnect_fail_after: None,
            reconnect_count: AtomicUsize::new(0),
        });
        let terminals = Arc::new(parking_lot::Mutex::new(Default::default()));
        let sm = Arc::new(parking_lot::Mutex::new(ServiceManager::new(
            backend.clone(),
            terminals,
        )));
        let rr = reactor_ref(&sm);
        let mut creating = project("project", &project_path);
        creating.is_creating = true;
        creating
            .service_terminals
            .insert("web".into(), "persistent-web".into());
        let mut active = creating.clone();
        active.is_creating = false;
        let mut sync_state = ServiceSyncState::default();
        let local = tokio::task::LocalSet::new();

        local
            .run_until(async {
                {
                    let mut manager = sm.lock();
                    let mut cx = rr.cx();
                    sync_services(&[creating], &mut sync_state, &mut manager, &mut cx);
                }
                assert!(
                    backend
                        .reconnected
                        .lock()
                        .expect("reconnect lock")
                        .is_empty()
                );
                assert!(!sync_state.known.contains_key("project"));

                {
                    let mut manager = sm.lock();
                    let mut cx = rr.cx();
                    sync_services(&[active.clone()], &mut sync_state, &mut manager, &mut cx);
                }
                wait_for_test_condition(|| {
                    backend.reconnected.lock().expect("reconnect lock").len() == 1
                        && sm.lock().project_path("project") == Some(&project_path)
                })
                .await;

                {
                    let mut manager = sm.lock();
                    let mut cx = rr.cx();
                    sync_services(&[active.clone()], &mut sync_state, &mut manager, &mut cx);
                }
                assert_eq!(backend.reconnected.lock().expect("reconnect lock").len(), 1);

                let mut closing = active;
                closing.is_closing = true;
                {
                    let mut manager = sm.lock();
                    let mut cx = rr.cx();
                    sync_services(&[closing], &mut sync_state, &mut manager, &mut cx);
                }
            })
            .await;

        assert_eq!(sm.lock().project_path("project"), Some(&project_path));
        assert!(sync_state.known.contains_key("project"));
        assert_eq!(backend.reconnected.lock().expect("reconnect lock").len(), 1);
        assert!(backend.killed.lock().expect("kill lock").is_empty());
        std::fs::remove_dir_all(project_dir).expect("remove project dir");
    }

    #[tokio::test]
    async fn sync_services_is_idempotent_when_converged() {
        let sm = std::sync::Arc::new(parking_lot::Mutex::new(manager()));
        let rr = reactor_ref(&sm);

        let projects = vec![project("local", &existing_path())];
        let mut sync_state = ServiceSyncState::default();

        // First pass loads and tracks.
        {
            let mut guard = sm.lock();
            let mut cx = rr.cx();
            sync_services(&projects, &mut sync_state, &mut guard, &mut cx);
        }
        let known_after_first = sync_state.known.clone();

        // Second pass with the same project set is a no-op (already in `known`).
        {
            let mut guard = sm.lock();
            let mut cx = rr.cx();
            sync_services(&projects, &mut sync_state, &mut guard, &mut cx);
        }
        assert_eq!(sync_state.known, known_after_first);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn sync_services_adopts_recovered_directory_runtime_without_resetting_intent() {
        let project_dir = std::env::temp_dir().join(format!(
            "okena-observer-renamed-runtime-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&project_dir).expect("create renamed project dir");
        std::fs::write(
            project_dir.join("okena.yaml"),
            "services:\n  - name: manual\n    command: echo manual\n  - name: automatic\n    command: echo automatic\n    auto_start: true\n",
        )
        .expect("write service config");
        let new_path = project_dir.to_string_lossy().into_owned();
        let backend = Arc::new(RecordingBackend {
            killed: Mutex::new(Vec::new()),
            reconnected: Mutex::new(Vec::new()),
            reconnect_fail_after: None,
            reconnect_count: AtomicUsize::new(0),
        });
        let terminals = Arc::new(parking_lot::Mutex::new(Default::default()));
        let sm = Arc::new(parking_lot::Mutex::new(ServiceManager::new(
            backend.clone(),
            terminals,
        )));
        let rr = reactor_ref(&sm);
        let local = tokio::task::LocalSet::new();

        let mut sync_state = ServiceSyncState {
            known: HashMap::from([(
                "project".to_string(),
                KnownProject {
                    path: "/previous/project/path".to_string(),
                    data_replacement_epoch: 0,
                },
            )]),
            ..ServiceSyncState::default()
        };
        let mut renamed = project("project", &new_path);
        renamed
            .service_terminals
            .insert("manual".into(), "manual-terminal".into());

        let (reconnected_before_sync, killed_before_sync) = local
            .run_until(async {
                {
                    let mut manager = sm.lock();
                    let mut cx = rr.cx();
                    manager.set_project_writeback_owner("project", &new_path, 0);
                    assert_eq!(
                        manager.load_project_services_prepared(
                            "project",
                            &new_path,
                            &renamed.service_terminals,
                            prepare_project_config(&new_path),
                            &mut cx,
                        ),
                        ServiceLoadStatus::Loaded
                    );
                }
                wait_for_test_condition(|| {
                    sm.lock()
                        .services_for_project("project")
                        .iter()
                        .any(|service| {
                            service.definition.name == "manual"
                                && service.status == okena_services::manager::ServiceStatus::Running
                        })
                })
                .await;
                {
                    let mut manager = sm.lock();
                    let mut cx = rr.cx();
                    manager.stop_service("project", "automatic", &mut cx);
                    let reconnected_before_sync =
                        backend.reconnected.lock().expect("reconnect lock").clone();
                    let killed_before_sync = backend.killed.lock().expect("kill lock").clone();
                    sync_services(&[renamed], &mut sync_state, &mut manager, &mut cx);
                    (reconnected_before_sync, killed_before_sync)
                }
            })
            .await;

        let manager = sm.lock();
        let statuses: HashMap<_, _> = manager
            .services_for_project("project")
            .iter()
            .map(|service| (service.definition.name.as_str(), service.status.clone()))
            .collect();
        assert_eq!(
            statuses.get("manual"),
            Some(&okena_services::manager::ServiceStatus::Running)
        );
        assert_eq!(
            statuses.get("automatic"),
            Some(&okena_services::manager::ServiceStatus::Stopped)
        );
        assert_eq!(
            sync_state.known.get("project"),
            Some(&KnownProject {
                path: new_path,
                data_replacement_epoch: 0,
            })
        );
        assert_eq!(
            *backend.reconnected.lock().expect("reconnect lock"),
            reconnected_before_sync
        );
        assert_eq!(
            *backend.killed.lock().expect("kill lock"),
            killed_before_sync
        );
        drop(manager);
        std::fs::remove_dir_all(project_dir).expect("remove renamed project dir");
    }

    #[tokio::test]
    async fn sync_services_reconciles_reused_id_after_data_replacement() {
        let sm = std::sync::Arc::new(parking_lot::Mutex::new(manager()));
        let rr = reactor_ref(&sm);
        let first_path = existing_path();
        let second_path = std::path::Path::new(&first_path)
            .parent()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let mut sync_state = ServiceSyncState::default();

        {
            let projects = vec![project("reused", &first_path)];
            let mut guard = sm.lock();
            let mut cx = rr.cx();
            sync_services(&projects, &mut sync_state, &mut guard, &mut cx);
        }

        {
            let mut replacement = project("reused", &second_path);
            replacement.data_replacement_epoch = 1;
            let mut guard = sm.lock();
            let mut cx = rr.cx();
            sync_services(&[replacement], &mut sync_state, &mut guard, &mut cx);
        }

        assert_eq!(sm.lock().project_path("reused"), Some(&second_path));
        assert_eq!(
            sync_state
                .known
                .get("reused")
                .map(|entry| entry.data_replacement_epoch),
            Some(1)
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn replacement_reconciliation_preserves_incoming_service_session() {
        let project_dir = std::env::temp_dir().join(format!(
            "okena-observer-services-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&project_dir).expect("create project dir");
        std::fs::write(
            project_dir.join("okena.yaml"),
            "services:\n  - name: web\n    command: echo web\n",
        )
        .expect("write service config");
        let project_path = project_dir.to_string_lossy().into_owned();
        let backend = Arc::new(RecordingBackend {
            killed: Mutex::new(Vec::new()),
            reconnected: Mutex::new(Vec::new()),
            reconnect_fail_after: None,
            reconnect_count: AtomicUsize::new(0),
        });
        let terminals = Arc::new(parking_lot::Mutex::new(Default::default()));
        let sm = Arc::new(parking_lot::Mutex::new(ServiceManager::new(
            backend.clone(),
            terminals,
        )));
        let rr = reactor_ref(&sm);
        let local = tokio::task::LocalSet::new();

        local
            .run_until(async {
                let mut initial = project("project", &project_path);
                initial
                    .service_terminals
                    .insert("web".into(), "persistent-web".into());
                let mut sync_state = ServiceSyncState::default();
                {
                    let mut manager = sm.lock();
                    let mut cx = rr.cx();
                    sync_services(&[initial], &mut sync_state, &mut manager, &mut cx);
                }
                wait_for_test_condition(|| {
                    !backend
                        .reconnected
                        .lock()
                        .expect("reconnect lock")
                        .is_empty()
                })
                .await;

                let mut replacement = project("project", &project_path);
                replacement.data_replacement_epoch = 1;
                replacement
                    .service_terminals
                    .insert("web".into(), "persistent-web".into());
                {
                    let mut manager = sm.lock();
                    let mut cx = rr.cx();
                    sync_services(&[replacement], &mut sync_state, &mut manager, &mut cx);
                }
                wait_for_test_condition(|| {
                    backend.reconnected.lock().expect("reconnect lock").len() >= 2
                })
                .await;
            })
            .await;

        assert!(backend.killed.lock().expect("kill lock").is_empty());
        assert_eq!(
            backend
                .reconnected
                .lock()
                .expect("reconnect lock")
                .as_slice(),
            &["persistent-web", "persistent-web"]
        );
        std::fs::remove_dir_all(&project_dir).expect("remove project dir");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn replacement_kills_preserved_session_removed_from_current_config() {
        let project_dir = std::env::temp_dir().join(format!(
            "okena-observer-stale-service-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&project_dir).expect("create project dir");
        let config_path = project_dir.join("okena.yaml");
        std::fs::write(
            &config_path,
            "services:\n  - name: web\n    command: echo web\n",
        )
        .expect("write initial service config");
        let project_path = project_dir.to_string_lossy().into_owned();
        let backend = Arc::new(RecordingBackend {
            killed: Mutex::new(Vec::new()),
            reconnected: Mutex::new(Vec::new()),
            reconnect_fail_after: None,
            reconnect_count: AtomicUsize::new(0),
        });
        let terminals = Arc::new(parking_lot::Mutex::new(Default::default()));
        let sm = Arc::new(parking_lot::Mutex::new(ServiceManager::new(
            backend.clone(),
            terminals,
        )));
        let rr = reactor_ref(&sm);
        let local = tokio::task::LocalSet::new();

        local
            .run_until(async {
                let mut initial = project("project", &project_path);
                initial
                    .service_terminals
                    .insert("web".into(), "persistent-web".into());
                let mut sync_state = ServiceSyncState::default();
                {
                    let mut manager = sm.lock();
                    let mut cx = rr.cx();
                    sync_services(&[initial], &mut sync_state, &mut manager, &mut cx);
                }
                wait_for_test_condition(|| {
                    !backend
                        .reconnected
                        .lock()
                        .expect("reconnect lock")
                        .is_empty()
                })
                .await;

                std::fs::write(
                    &config_path,
                    "services:\n  - name: replacement\n    command: echo replacement\n",
                )
                .expect("write replacement service config");
                let mut replacement = project("project", &project_path);
                replacement.data_replacement_epoch = 1;
                replacement
                    .service_terminals
                    .insert("web".into(), "persistent-web".into());
                {
                    let mut manager = sm.lock();
                    let mut cx = rr.cx();
                    sync_services(&[replacement], &mut sync_state, &mut manager, &mut cx);
                }
                wait_for_test_condition(|| !backend.killed.lock().expect("kill lock").is_empty())
                    .await;
            })
            .await;

        assert_eq!(
            backend.killed.lock().expect("kill lock").as_slice(),
            &["persistent-web"]
        );
        assert_eq!(
            backend
                .reconnected
                .lock()
                .expect("reconnect lock")
                .as_slice(),
            &["persistent-web"]
        );
        assert!(sm.lock().service_terminal_ids("project").is_empty());
        std::fs::remove_dir_all(&project_dir).expect("remove project dir");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn replacement_kills_preserved_session_after_reconnect_failure_once() {
        let project_dir = std::env::temp_dir().join(format!(
            "okena-observer-failed-reconnect-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&project_dir).expect("create project dir");
        std::fs::write(
            project_dir.join("okena.yaml"),
            "services:\n  - name: web\n    command: echo web\n",
        )
        .expect("write service config");
        let project_path = project_dir.to_string_lossy().into_owned();
        let backend = Arc::new(RecordingBackend {
            killed: Mutex::new(Vec::new()),
            reconnected: Mutex::new(Vec::new()),
            reconnect_fail_after: Some(1),
            reconnect_count: AtomicUsize::new(0),
        });
        let terminals = Arc::new(parking_lot::Mutex::new(Default::default()));
        let sm = Arc::new(parking_lot::Mutex::new(ServiceManager::new(
            backend.clone(),
            terminals,
        )));
        let rr = reactor_ref(&sm);
        let local = tokio::task::LocalSet::new();

        local
            .run_until(async {
                let mut initial = project("project", &project_path);
                initial
                    .service_terminals
                    .insert("web".into(), "persistent-web".into());
                let mut sync_state = ServiceSyncState::default();
                {
                    let mut manager = sm.lock();
                    let mut cx = rr.cx();
                    sync_services(&[initial], &mut sync_state, &mut manager, &mut cx);
                }
                wait_for_test_condition(|| {
                    !backend
                        .reconnected
                        .lock()
                        .expect("reconnect lock")
                        .is_empty()
                })
                .await;

                let mut replacement = project("project", &project_path);
                replacement.data_replacement_epoch = 1;
                replacement
                    .service_terminals
                    .insert("web".into(), "persistent-web".into());
                {
                    let mut manager = sm.lock();
                    let mut cx = rr.cx();
                    sync_services(&[replacement], &mut sync_state, &mut manager, &mut cx);
                }
                wait_for_test_condition(|| {
                    backend.reconnected.lock().expect("reconnect lock").len() >= 2
                })
                .await;
                wait_for_test_condition(|| !backend.killed.lock().expect("kill lock").is_empty())
                    .await;
            })
            .await;

        assert_eq!(
            backend.killed.lock().expect("kill lock").as_slice(),
            &["persistent-web"]
        );
        assert_eq!(
            backend
                .reconnected
                .lock()
                .expect("reconnect lock")
                .as_slice(),
            &["persistent-web", "persistent-web"]
        );
        assert!(sm.lock().service_terminal_ids("project").is_empty());
        std::fs::remove_dir_all(&project_dir).expect("remove project dir");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn missing_replacement_kills_pending_session_once_when_project_is_removed() {
        let project_dir = std::env::temp_dir().join(format!(
            "okena-observer-pending-remove-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&project_dir).expect("create project dir");
        std::fs::write(
            project_dir.join("okena.yaml"),
            "services:\n  - name: web\n    command: echo web\n",
        )
        .expect("write service config");
        let project_path = project_dir.to_string_lossy().into_owned();
        let missing_path = project_dir.with_extension("missing");
        let missing_path = missing_path.to_string_lossy().into_owned();
        let backend = Arc::new(RecordingBackend {
            killed: Mutex::new(Vec::new()),
            reconnected: Mutex::new(Vec::new()),
            reconnect_fail_after: None,
            reconnect_count: AtomicUsize::new(0),
        });
        let terminals = Arc::new(parking_lot::Mutex::new(Default::default()));
        let sm = Arc::new(parking_lot::Mutex::new(ServiceManager::new(
            backend.clone(),
            terminals,
        )));
        let rr = reactor_ref(&sm);
        let mut sync_state = ServiceSyncState::default();
        let local = tokio::task::LocalSet::new();

        local
            .run_until(async {
                let mut initial = project("project", &project_path);
                initial
                    .service_terminals
                    .insert("web".into(), "persistent-web".into());
                {
                    let mut manager = sm.lock();
                    let mut cx = rr.cx();
                    sync_services(&[initial], &mut sync_state, &mut manager, &mut cx);
                }

                let mut replacement = project("project", &missing_path);
                replacement.data_replacement_epoch = 1;
                replacement
                    .service_terminals
                    .insert("web".into(), "persistent-web".into());
                {
                    let mut manager = sm.lock();
                    let mut cx = rr.cx();
                    sync_services(
                        &[replacement.clone()],
                        &mut sync_state,
                        &mut manager,
                        &mut cx,
                    );
                    sync_services(&[replacement], &mut sync_state, &mut manager, &mut cx);
                }
                assert!(backend.killed.lock().expect("kill lock").is_empty());
                assert!(sync_state.pending_preserved.contains_key("project"));
                assert!(sm.lock().service_terminal_writebacks().is_empty());

                {
                    let mut manager = sm.lock();
                    let mut cx = rr.cx();
                    sync_services(&[], &mut sync_state, &mut manager, &mut cx);
                    sync_services(&[], &mut sync_state, &mut manager, &mut cx);
                }
            })
            .await;
        assert_eq!(
            backend.killed.lock().expect("kill lock").as_slice(),
            &["persistent-web"]
        );
        assert!(!sync_state.pending_preserved.contains_key("project"));
        std::fs::remove_dir_all(&project_dir).expect("remove project dir");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn missing_replacement_reconnects_pending_session_when_path_returns() {
        let base = std::env::temp_dir().join(format!(
            "okena-observer-pending-recover-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let initial_dir = base.join("initial");
        let recovered_dir = base.join("recovered");
        std::fs::create_dir_all(&initial_dir).expect("create initial project dir");
        std::fs::write(
            initial_dir.join("okena.yaml"),
            "services:\n  - name: web\n    command: echo web\n",
        )
        .expect("write initial service config");
        let initial_path = initial_dir.to_string_lossy().into_owned();
        let recovered_path = recovered_dir.to_string_lossy().into_owned();
        let backend = Arc::new(RecordingBackend {
            killed: Mutex::new(Vec::new()),
            reconnected: Mutex::new(Vec::new()),
            reconnect_fail_after: None,
            reconnect_count: AtomicUsize::new(0),
        });
        let terminals = Arc::new(parking_lot::Mutex::new(Default::default()));
        let sm = Arc::new(parking_lot::Mutex::new(ServiceManager::new(
            backend.clone(),
            terminals,
        )));
        let rr = reactor_ref(&sm);
        let mut sync_state = ServiceSyncState::default();
        let local = tokio::task::LocalSet::new();

        local
            .run_until(async {
                let mut initial = project("project", &initial_path);
                initial
                    .service_terminals
                    .insert("web".into(), "persistent-web".into());
                {
                    let mut manager = sm.lock();
                    let mut cx = rr.cx();
                    sync_services(&[initial], &mut sync_state, &mut manager, &mut cx);
                }
                wait_for_test_condition(|| {
                    !backend
                        .reconnected
                        .lock()
                        .expect("reconnect lock")
                        .is_empty()
                })
                .await;

                let mut replacement = project("project", &recovered_path);
                replacement.data_replacement_epoch = 1;
                replacement
                    .service_terminals
                    .insert("web".into(), "persistent-web".into());
                {
                    let mut manager = sm.lock();
                    let mut cx = rr.cx();
                    sync_services(
                        &[replacement.clone()],
                        &mut sync_state,
                        &mut manager,
                        &mut cx,
                    );
                }
                assert!(sync_state.pending_preserved.contains_key("project"));
                assert!(sm.lock().service_terminal_writebacks().is_empty());

                std::fs::create_dir_all(&recovered_dir).expect("create recovered project dir");
                std::fs::write(
                    recovered_dir.join("okena.yaml"),
                    "services:\n  - name: web\n    command: echo web\n",
                )
                .expect("write recovered service config");
                {
                    let mut manager = sm.lock();
                    let mut cx = rr.cx();
                    sync_services(&[replacement], &mut sync_state, &mut manager, &mut cx);
                }
                wait_for_test_condition(|| {
                    backend.reconnected.lock().expect("reconnect lock").len() >= 2
                })
                .await;
            })
            .await;

        assert!(backend.killed.lock().expect("kill lock").is_empty());
        assert_eq!(
            backend
                .reconnected
                .lock()
                .expect("reconnect lock")
                .as_slice(),
            &["persistent-web", "persistent-web"]
        );
        assert!(!sync_state.pending_preserved.contains_key("project"));
        assert!(sync_state.known.contains_key("project"));
        std::fs::remove_dir_all(&base).expect("remove project dirs");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn failed_load_retries_without_rekilling_discarded_session() {
        let project_dir = std::env::temp_dir().join(format!(
            "okena-observer-load-retry-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&project_dir).expect("create project dir");
        let config_path = project_dir.join("okena.yaml");
        std::fs::write(&config_path, "services: [").expect("write malformed config");
        let project_path = project_dir.to_string_lossy().into_owned();
        let backend = Arc::new(RecordingBackend {
            killed: Mutex::new(Vec::new()),
            reconnected: Mutex::new(Vec::new()),
            reconnect_fail_after: None,
            reconnect_count: AtomicUsize::new(0),
        });
        let terminals = Arc::new(parking_lot::Mutex::new(Default::default()));
        let sm = Arc::new(parking_lot::Mutex::new(ServiceManager::new(
            backend.clone(),
            terminals,
        )));
        let rr = reactor_ref(&sm);
        let mut sync_state = ServiceSyncState::default();
        let mut snapshot = project("project", &project_path);
        snapshot
            .service_terminals
            .insert("web".into(), "persistent-web".into());

        {
            let mut manager = sm.lock();
            let mut cx = rr.cx();
            sync_services(&[snapshot.clone()], &mut sync_state, &mut manager, &mut cx);
        }
        let first_deadline = sync_state.retry_deadline;
        assert!(first_deadline.is_some());
        assert_eq!(
            backend.killed.lock().expect("kill lock").as_slice(),
            &["persistent-web"]
        );
        assert_eq!(
            sm.lock().service_terminal_writebacks(),
            vec![ServiceTerminalWriteback {
                project_id: "project".into(),
                project_path: project_path.clone(),
                data_replacement_epoch: 0,
                terminal_ids: HashMap::new(),
            }]
        );

        {
            let mut manager = sm.lock();
            let mut cx = rr.cx();
            sync_services(&[snapshot.clone()], &mut sync_state, &mut manager, &mut cx);
        }
        assert_eq!(sync_state.retry_deadline, first_deadline);
        assert_eq!(
            backend.killed.lock().expect("kill lock").as_slice(),
            &["persistent-web"]
        );

        std::fs::write(&config_path, "services: []\n").expect("repair config");
        sync_state.retry_deadline = None;
        {
            let mut manager = sm.lock();
            let mut cx = rr.cx();
            sync_services(&[snapshot], &mut sync_state, &mut manager, &mut cx);
        }
        assert!(sync_state.known.contains_key("project"));
        assert!(!sync_state.retry_projects.contains("project"));
        assert_eq!(
            backend.killed.lock().expect("kill lock").as_slice(),
            &["persistent-web"]
        );
        std::fs::remove_dir_all(&project_dir).expect("remove project dir");
    }

    #[tokio::test]
    async fn sync_services_unloads_changed_missing_path_and_reloads_after_recovery() {
        let sm = std::sync::Arc::new(parking_lot::Mutex::new(manager()));
        let rr = reactor_ref(&sm);
        let mut sync_state = ServiceSyncState::default();

        {
            let projects = vec![project("local", &existing_path())];
            let mut guard = sm.lock();
            let mut cx = rr.cx();
            sync_services(&projects, &mut sync_state, &mut guard, &mut cx);
        }

        {
            let mut missing = project("local", "/path/that/does/not/exist/okena");
            missing.data_replacement_epoch = 1;
            let mut guard = sm.lock();
            let mut cx = rr.cx();
            sync_services(&[missing], &mut sync_state, &mut guard, &mut cx);
        }
        assert!(!sync_state.known.contains_key("local"));
        assert!(sm.lock().project_path("local").is_none());

        {
            let mut recovered = project("local", &existing_path());
            recovered.data_replacement_epoch = 1;
            let mut guard = sm.lock();
            let mut cx = rr.cx();
            sync_services(&[recovered], &mut sync_state, &mut guard, &mut cx);
        }
        assert!(sync_state.known.contains_key("local"));
        let recovered_path = existing_path();
        assert_eq!(sm.lock().project_path("local"), Some(&recovered_path));
    }

    /// End-to-end-ish: spawn the observer tasks on a LocalSet, bump
    /// `workspace_tick`, and assert `state_version` advances. Exercises the
    /// `spawn_local`/LocalSet wiring and the tick→state_version bump.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn observers_advance_state_version_on_workspace_tick() {
        use okena_workspace::state::Workspace;

        let backend = Arc::new(StubBackend);
        let terminals = Arc::new(parking_lot::Mutex::new(Default::default()));
        let workspace = Workspace::new(empty_workspace_data());
        let reactor = Arc::new(DaemonReactor::new(
            workspace,
            backend,
            terminals,
            None,
            None,
            tokio::runtime::Handle::current(),
        ));

        let local = tokio::task::LocalSet::new();
        local
            .run_until(async move {
                reactor.spawn_observers();

                let mut sv_rx = reactor.state_version.subscribe();
                let before = *sv_rx.borrow_and_update();

                // Bump the workspace tick — the observer should react.
                reactor.workspace_tick.send_modify(|v| *v += 1);

                // Wait for state_version to advance (the workspace-tick task ran).
                sv_rx.changed().await.expect("state_version sender alive");
                let after = *sv_rx.borrow();
                assert!(
                    after > before,
                    "state_version should advance on workspace_tick"
                );
            })
            .await;
    }
}
