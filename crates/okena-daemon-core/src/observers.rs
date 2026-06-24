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

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use okena_services::manager::{ServiceCx, ServiceManager};
use okena_workspace::persistence;

use crate::reactor::DaemonReactor;
use crate::service_cx::ServiceReactorRef;
use crate::workspace_cx::DaemonWorkspaceCx;

/// Debounce window before an autosave is flushed to disk. Mirrors the GUI's
/// 500ms timer in `app/mod.rs`.
const AUTOSAVE_DEBOUNCE: Duration = Duration::from_millis(500);

/// Per-project snapshot taken under the workspace lock so the services diff can
/// run after the lock is dropped (the separate-lock-scope guard).
struct ProjectSnapshot {
    id: String,
    path: String,
    is_remote: bool,
    service_terminals: std::collections::HashMap<String, String>,
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
            self.hook_runner.clone(),
            self.hook_monitor.clone(),
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

/// The workspace-tick observer task: bump `state_version`, autosave, and run the
/// project→services load/unload diff on every `workspace_tick` change.
#[allow(clippy::too_many_arguments)]
async fn workspace_tick_task(
    mut tick_rx: tokio::sync::watch::Receiver<u64>,
    workspace: SharedWorkspace,
    service_manager: SharedServiceManager,
    state_version: tokio::sync::watch::Sender<u64>,
    service_tick: tokio::sync::watch::Sender<u64>,
    runtime: tokio::runtime::Handle,
    hook_runner: Option<okena_hooks::HookRunner>,
    hook_monitor: Option<okena_hooks::HookMonitor>,
) {
    // Tracks the `data_version` last persisted, so UI-only changes skip the
    // save — the daemon analogue of the GUI's `last_saved_version`.
    let last_saved_version = Arc::new(AtomicU64::new(0));
    // Projects already loaded into the service manager — the GUI's `known` set,
    // kept across passes to make the diff idempotent.
    let mut known: HashSet<String> = HashSet::new();

    // Mirror the GUI's initial load: run one diff pass before awaiting ticks so
    // persisted projects get their services loaded at startup.
    run_services_sync(&workspace, &service_manager, &runtime, &service_tick, &mut known);

    loop {
        if tick_rx.changed().await.is_err() {
            // All senders dropped — the reactor is gone; stop the task.
            return;
        }

        // Coarse "persistent state changed" tick.
        state_version.send_modify(|v| *v += 1);

        // ── Debounced autosave ───────────────────────────────────────────────
        autosave(&workspace, &runtime, &hook_runner, &hook_monitor, &last_saved_version).await;

        // ── project → services load/unload diff ─────────────────────────────
        run_services_sync(&workspace, &service_manager, &runtime, &service_tick, &mut known);
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
        let terminal_maps: Vec<(String, std::collections::HashMap<String, String>)> = {
            let sm = service_manager.lock();
            let project_ids: HashSet<String> =
                sm.instances().keys().map(|(pid, _)| pid.clone()).collect();
            project_ids
                .into_iter()
                .map(|pid| {
                    let ids = sm.service_terminal_ids(&pid);
                    (pid, ids)
                })
                .collect()
        };

        // Lock scope 2: write the maps back under the workspace lock.
        // `sync_service_terminals` only notifies when a map actually changes, so
        // once converged this stops bumping `workspace_tick` and the cross-tick
        // storm terminates.
        {
            let mut ws = workspace.lock();
            let mut cx = DaemonWorkspaceCx::new(&workspace_tick, &hook_runner, &hook_monitor);
            for (project_id, terminals) in terminal_maps {
                ws.sync_service_terminals(&project_id, terminals, &mut cx);
            }
        }
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
    _hook_runner: &Option<okena_hooks::HookRunner>,
    _hook_monitor: &Option<okena_hooks::HookMonitor>,
    last_saved_version: &Arc<AtomicU64>,
) {
    // Skip UI-only changes: the persistent `data_version` is unchanged.
    let current_version = workspace.lock().data_version();
    if current_version == last_saved_version.load(Ordering::Relaxed) {
        return;
    }

    // Debounce: a burst of mutations collapses into one save after the window.
    tokio::time::sleep(AUTOSAVE_DEBOUNCE).await;

    // Re-snapshot after the sleep — the version may have moved again; take the
    // latest under a short lock and DROP it before the blocking I/O.
    let (data, version) = {
        let ws = workspace.lock();
        (ws.data().clone(), ws.data_version())
    };

    // Blocking fs I/O — offload onto the multi-thread runtime so it never stalls
    // the LocalSet thread (Windows AV / OneDrive can stall workspace.json saves).
    let save_result = runtime
        .spawn_blocking(move || persistence::save_workspace(&data))
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
fn run_services_sync(
    workspace: &SharedWorkspace,
    service_manager: &SharedServiceManager,
    runtime: &tokio::runtime::Handle,
    service_tick: &tokio::sync::watch::Sender<u64>,
    known: &mut HashSet<String>,
) {
    // Lock scope 1: snapshot the projects, then drop the workspace lock.
    let projects: Vec<ProjectSnapshot> = {
        let ws = workspace.lock();
        ws.data()
            .projects
            .iter()
            .map(|p| ProjectSnapshot {
                id: p.id.clone(),
                path: p.path.clone(),
                is_remote: p.is_remote,
                service_terminals: p.service_terminals.clone(),
            })
            .collect()
    };

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
    sync_services(&projects, known, &mut sm, &mut cx);
}

/// GPUI-free port of `okena-app`'s `app/mod.rs::sync_services`: diff the current
/// non-remote, on-disk project set against `known` and load/unload service
/// configs accordingly. Idempotent — a project already in `known` is skipped, so
/// repeated passes converge to no-ops (the re-entrancy guard).
fn sync_services(
    projects: &[ProjectSnapshot],
    known: &mut HashSet<String>,
    sm: &mut ServiceManager,
    cx: &mut impl ServiceCx,
) {
    let current_ids: HashSet<String> = projects
        .iter()
        .filter(|p| !p.is_remote)
        .map(|p| p.id.clone())
        .collect();

    for p in projects {
        if p.is_remote || known.contains(&p.id) {
            continue;
        }
        // Skip projects whose directory doesn't exist yet (deferred worktrees).
        if !std::path::Path::new(&p.path).exists() {
            continue;
        }
        sm.load_project_services(&p.id, &p.path, &p.service_terminals, cx);
        known.insert(p.id.clone());
    }

    let removed: Vec<String> = known.difference(&current_ids).cloned().collect();
    for id in &removed {
        sm.unload_project_services(id, cx);
        known.remove(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use okena_state::WorkspaceData;
    use okena_terminal::backend::TerminalBackend;
    use okena_terminal::shell_config::ShellType;
    use okena_terminal::terminal::TerminalTransport;

    /// No-op transport for the test backend. Never actually exercised by the
    /// `sync_services` tests (projects carry no saved terminal ids and the crate
    /// dir has no `okena.yaml` / docker-compose), but required to satisfy the
    /// `TerminalBackend::transport` return type.
    struct StubTransport;

    impl TerminalTransport for StubTransport {
        fn send_input(&self, _terminal_id: &str, _data: &[u8]) {}
        fn resize(&self, _terminal_id: &str, _cols: u16, _rows: u16) {}
        fn uses_mouse_backend(&self) -> bool {
            false
        }
    }

    /// Minimal `TerminalBackend` for constructing a `ServiceManager` in tests.
    /// The `sync_services` test path (no `okena.yaml`, no docker-compose, empty
    /// `service_terminals`) never reaches terminal creation, so these methods are
    /// no-ops / errors.
    struct StubBackend;

    impl TerminalBackend for StubBackend {
        fn transport(&self) -> Arc<dyn TerminalTransport> {
            Arc::new(StubTransport)
        }
        fn create_terminal(&self, _cwd: &str, _shell: Option<&ShellType>) -> anyhow::Result<String> {
            anyhow::bail!("stub backend: create_terminal not supported")
        }
        fn reconnect_terminal(
            &self,
            _terminal_id: &str,
            _cwd: &str,
            _shell: Option<&ShellType>,
        ) -> anyhow::Result<String> {
            anyhow::bail!("stub backend: reconnect_terminal not supported")
        }
        fn kill(&self, _terminal_id: &str) {}
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
    fn project(id: &str, path: &str, is_remote: bool) -> ProjectSnapshot {
        ProjectSnapshot {
            id: id.to_string(),
            path: path.to_string(),
            is_remote,
            service_terminals: Default::default(),
        }
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

    /// An empty `WorkspaceData` for the integration-style observer test
    /// (`WorkspaceData` has no `Default`).
    fn empty_workspace_data() -> WorkspaceData {
        WorkspaceData {
            version: 1,
            projects: Vec::new(),
            project_order: Vec::new(),
            folders: Vec::new(),
            service_panel_heights: Default::default(),
            hook_panel_heights: Default::default(),
            main_window: Default::default(),
            extra_windows: Vec::new(),
        }
    }

    /// Build a top-level `DaemonServiceCx` over a throwaway reactor for tests
    /// that need to pass a `cx` into `sync_services`. The notify just bumps a
    /// detached watch channel.
    fn reactor_ref(manager: &std::sync::Arc<parking_lot::Mutex<ServiceManager>>) -> ServiceReactorRef {
        let (tick, _rx) = tokio::sync::watch::channel(0u64);
        ServiceReactorRef::new(manager.clone(), tokio::runtime::Handle::current(), tick)
    }

    #[tokio::test]
    async fn sync_services_loads_new_local_projects_and_tracks_them() {
        let sm = std::sync::Arc::new(parking_lot::Mutex::new(manager()));
        let rr = reactor_ref(&sm);

        let projects = vec![
            project("local", &existing_path(), false),
            project("remote", &existing_path(), true),
        ];
        let mut known = HashSet::new();

        {
            let mut guard = sm.lock();
            let mut cx = rr.cx();
            sync_services(&projects, &mut known, &mut guard, &mut cx);
        }

        // Non-remote, on-disk project is tracked; remote project is skipped.
        assert!(known.contains("local"));
        assert!(!known.contains("remote"));
    }

    #[tokio::test]
    async fn sync_services_skips_nonexistent_paths() {
        let sm = std::sync::Arc::new(parking_lot::Mutex::new(manager()));
        let rr = reactor_ref(&sm);

        let projects = vec![project("ghost", "/path/that/does/not/exist/okena", false)];
        let mut known = HashSet::new();

        {
            let mut guard = sm.lock();
            let mut cx = rr.cx();
            sync_services(&projects, &mut known, &mut guard, &mut cx);
        }

        // Deferred worktree (missing dir) is NOT tracked, so a later pass retries.
        assert!(!known.contains("ghost"));
    }

    #[tokio::test]
    async fn sync_services_unloads_removed_projects() {
        let sm = std::sync::Arc::new(parking_lot::Mutex::new(manager()));
        let rr = reactor_ref(&sm);

        // Pass 1: load a local project.
        let mut known = HashSet::new();
        {
            let projects = vec![project("local", &existing_path(), false)];
            let mut guard = sm.lock();
            let mut cx = rr.cx();
            sync_services(&projects, &mut known, &mut guard, &mut cx);
        }
        assert!(known.contains("local"));

        // Pass 2: the project is gone from the workspace → it is unloaded.
        {
            let projects: Vec<ProjectSnapshot> = vec![];
            let mut guard = sm.lock();
            let mut cx = rr.cx();
            sync_services(&projects, &mut known, &mut guard, &mut cx);
        }
        assert!(!known.contains("local"));
    }

    #[tokio::test]
    async fn sync_services_is_idempotent_when_converged() {
        let sm = std::sync::Arc::new(parking_lot::Mutex::new(manager()));
        let rr = reactor_ref(&sm);

        let projects = vec![project("local", &existing_path(), false)];
        let mut known = HashSet::new();

        // First pass loads and tracks.
        {
            let mut guard = sm.lock();
            let mut cx = rr.cx();
            sync_services(&projects, &mut known, &mut guard, &mut cx);
        }
        let known_after_first: HashSet<String> = known.clone();

        // Second pass with the same project set is a no-op (already in `known`).
        {
            let mut guard = sm.lock();
            let mut cx = rr.cx();
            sync_services(&projects, &mut known, &mut guard, &mut cx);
        }
        assert_eq!(known, known_after_first);
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
                assert!(after > before, "state_version should advance on workspace_tick");
            })
            .await;
    }
}
