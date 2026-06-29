mod detached_overlays;
mod detached_terminals;
mod extras;
pub mod headless;
mod notifications;
mod remote_commands;
mod remote_config;

pub use detached_overlays::open_detached_overlay;

use crate::remote_client::manager::{RemoteConnectionManager, RemoteManagerEvent};
use crate::services::manager::ServiceManager;
use crate::views::window::{TerminalsRegistry, WindowView};
use crate::workspace::state::{GlobalWorkspace, WindowId, Workspace, WorkspaceData};
use gpui::*;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

/// Best-effort kill a process by pid — SIGKILL on Unix, `TerminateProcess` on
/// Windows, matching `std::process::Child::kill`. Used by the UI-owned daemon
/// lifecycle to reap a daemon we own but hold no `Child` for: a restart spawns a
/// *detached* successor, known to us only by the pid it advertises in
/// `remote.json`. A pid of 0 (unknown) or an already-dead process is a no-op.
fn kill_process_by_pid(pid: u32) {
    if pid == 0 {
        return;
    }
    use sysinfo::{Pid, ProcessesToUpdate, System};
    let spid = Pid::from_u32(pid);
    let mut sys = System::new();
    sys.refresh_processes(ProcessesToUpdate::Some(&[spid]), true);
    if let Some(proc) = sys.process(spid) {
        proc.kill();
    }
}

/// Set up an observer that loads/unloads service configs when projects change.
/// Handles deferred worktrees by skipping projects whose directory doesn't exist yet.
///
/// Used by the headless daemon (`HeadlessApp`), which is the real service owner.
/// The GUI is a thin client and never runs services in-process.
pub(crate) fn observe_project_services<T: 'static>(
    workspace: &Entity<Workspace>,
    service_manager: &Entity<ServiceManager>,
    cx: &mut Context<T>,
) {
    let service_manager = service_manager.clone();
    let known: Arc<parking_lot::Mutex<HashSet<String>>> =
        Arc::new(parking_lot::Mutex::new(HashSet::new()));

    // Initial load
    {
        let data = workspace.read(cx).data().clone();
        sync_services(&data, &mut known.lock(), &service_manager, cx);
    }

    let known_for_observer = known.clone();
    cx.observe(workspace, move |_this, workspace: Entity<Workspace>, cx| {
        let data = workspace.read(cx).data().clone();
        sync_services(&data, &mut known_for_observer.lock(), &service_manager, cx);
    })
    .detach();
}

fn sync_services(
    data: &WorkspaceData,
    known: &mut HashSet<String>,
    service_manager: &Entity<ServiceManager>,
    cx: &mut impl AppContext,
) {
    let current_ids: HashSet<String> = data.projects.iter()
        .filter(|p| !p.is_remote)
        .map(|p| p.id.clone())
        .collect();

    for p in &data.projects {
        if p.is_remote || known.contains(&p.id) {
            continue;
        }
        // Skip projects whose directory doesn't exist yet (deferred worktrees).
        if !std::path::Path::new(&p.path).exists() {
            continue;
        }
        service_manager.update(cx, |sm, cx| {
            sm.load_project_services(&p.id, &p.path, &p.service_terminals, cx);
        });
        known.insert(p.id.clone());
    }

    let removed: Vec<String> = known.difference(&current_ids).cloned().collect();
    for id in &removed {
        service_manager.update(cx, |sm, cx| {
            sm.unload_project_services(id, cx);
        });
        known.remove(id);
    }
}

/// Main application state and view
pub struct Okena {
    /// The single, always-present main window. Closing it quits the app
    /// (per the multi-window PRD's main-is-special invariant).
    main_window: Entity<WindowView>,
    /// OS window handle of the main window. Captured from `window.window_handle()`
    /// in `Okena::new`'s `cx.open_window` build closure (see main.rs). Used by
    /// the remote-bridge command loop to resolve actions to the focused
    /// window's per-window `FocusManager` per PRD cri 13.
    pub(super) main_window_handle: AnyWindowHandle,
    /// Ephemeral extras spawned at runtime, keyed by `WindowId::Extra(uuid)`.
    /// Populated by the workspace observer in `handle_extra_windows_changed`
    /// when `WorkspaceData.extra_windows` gains a new entry; the matching
    /// `Entity<WindowView>` is created and inserted as part of the
    /// `cx.open_window` build closure (see `extras.rs`).
    extra_windows: HashMap<WindowId, Entity<WindowView>>,
    /// OS window handles for extras, keyed by `WindowId::Extra(uuid)`. Populated
    /// alongside `extra_windows` in `extras.rs::open_extra_window`. Same
    /// purpose as `main_window_handle` — focused-window resolution at the
    /// remote-bridge boundary (PRD cri 13).
    pub(super) extra_window_handles: HashMap<WindowId, AnyWindowHandle>,
    pub(crate) workspace: Entity<Workspace>,
    pub(crate) terminals: TerminalsRegistry,
    /// Track which detached windows we've already opened
    pub(crate) opened_detached_windows: HashSet<String>,
    /// Remote connection manager. Held so extras spawned at runtime can
    /// be wired with the same singleton main was wired with at startup
    /// (`open_extra_window` calls `set_remote_manager` on the new view).
    remote_manager: Entity<RemoteConnectionManager>,
    /// Sender handed to desktop-notification threads. When a user clicks an
    /// XDG notification, the thread sends a `NotificationJump` here and the
    /// click loop focuses the originating pane. See `app/notifications.rs`.
    notification_jump_tx: async_channel::Sender<notifications::NotificationJump>,
    /// Child of a daemon WE spawned in `--daemon-client` mode; killed on app
    /// quit. `None` if we attached to an existing daemon or in classic mode.
    spawned_daemon: Option<std::process::Child>,
}

impl Okena {
    pub fn new(
        workspace_data: WorkspaceData,
        local_daemon: okena_remote_server::local::EnsuredDaemon,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // Create workspace entity. The GUI is always a thin daemon client: the
        // daemon owns persistence + the instance lock and is the single writer,
        // so the GUI's `Workspace` is a pure mirror with no autosave.
        let workspace = cx.new(|_cx| Workspace::new(workspace_data));
        cx.set_global(GlobalWorkspace(workspace.clone()));

        // Shared terminals registry — one per Okena instance, threaded into
        // every WindowView (main + extras). Each TerminalPane looks up the
        // existing Arc<Terminal> for its terminal_id from this registry; if
        // each window had its own registry, an extra rendering a project
        // already shown in main would create a NEW Terminal model and PTY
        // bytes (which feed the original Arc<Terminal>) would never reach
        // the extra's content pane.
        let terminals: TerminalsRegistry = Arc::new(parking_lot::Mutex::new(std::collections::HashMap::new()));

        // Create the main window's per-window view, sharing the registry.
        let terminals_for_main = terminals.clone();
        let main_window = cx.new(|cx| {
            WindowView::new(WindowId::Main, workspace.clone(), terminals_for_main, window, cx)
        });

        // Listen for cross-window requests (e.g. "jump into a project's terminal"
        // from the Switch Project overlay). Okena is the only place that holds
        // every window's view + OS handle, so it executes these.
        cx.subscribe(&main_window, Self::handle_window_view_event).detach();

        // Create remote connection manager and wire to main window
        let remote_manager = cx.new(|cx| {
            RemoteConnectionManager::new(terminals.clone(), cx)
        });
        main_window.update(cx, |rv, cx| {
            rv.set_remote_manager(remote_manager.clone(), cx);
        });

        // Register the implicit, trusted loopback connection to our local
        // daemon so its projects mirror into the GUI. We own the spawned child
        // (if any) and kill it on quit; an attached daemon (`spawned == None`)
        // is left alone (§ risk: only the spawner kills). The connection uses a
        // fixed id so it's recognizable and dedup-safe, and is never written to
        // settings — `add_connection` does not persist, and the only insertion
        // site (`OverlayManagerEvent::RemoteConnected`) is never fired for it.
        let spawned_daemon = {
            let ensured = local_daemon;
            let cfg = okena_transport::client::RemoteConnectionConfig {
                id: okena_transport::client::LOCAL_DAEMON_CONNECTION_ID.to_string(),
                name: "Local".to_string(),
                host: ensured.daemon.host().to_string(),
                port: ensured.daemon.port,
                saved_token: Some(ensured.token.clone()),
                token_obtained_at: None,
                tls: false,
                pinned_cert_sha256: None,
                local_endpoint: ensured.daemon.local_endpoint.clone(),
            };
            if let Err(e) = remote_manager.update(cx, |rm, cx| rm.add_connection(cfg, cx)) {
                eprintln!("Failed to register local-daemon loopback connection: {e}");
                std::process::exit(1);
            }
            ensured.spawned
        };

        // Auto-connect to saved connections with valid tokens after the
        // reserved local-daemon connection is present. Saved user-managed
        // remotes that point at the same endpoint are skipped by the manager;
        // the implicit local connection is the authoritative one.
        remote_manager.update(cx, |rm, cx| {
            rm.auto_connect_all(cx);
            rm.start_token_refresh_task(cx);
        });

        // Observe window bounds changes to force re-render
        cx.observe_window_bounds(window, |_this, _window, cx| {
            cx.notify();
        })
        .detach();

        // Channel for clicked desktop notifications → "jump to that pane".
        let (notification_jump_tx, notification_jump_rx) = async_channel::unbounded();

        let main_window_handle = window.window_handle();

        let mut manager = Self {
            main_window,
            main_window_handle,
            extra_windows: HashMap::new(),
            extra_window_handles: HashMap::new(),
            workspace: workspace.clone(),
            terminals,
            opened_detached_windows: HashSet::new(),
            remote_manager: remote_manager.clone(),
            notification_jump_tx,
            spawned_daemon,
        };

        // Route clicked desktop notifications back to their originating pane.
        manager.start_notification_click_loop(notification_jump_rx, cx);

        // Fire OS notifications for remote (daemon-served) terminals. Their PTY
        // output never reaches the local PTY event loop above — it arrives over
        // the WS and is only parsed by the remote activity pump, which drains
        // each terminal's pending bytes (populating the OSC 9/777/99 + bell
        // queues) but doesn't fire OS bubbles. The pump emits the advanced
        // terminal ids here so we reuse the exact same focus-suppressed,
        // settings-gated notification path the local loop uses. Without this,
        // notifications from real (remote) terminals would be parsed and then
        // silently dropped in the daemon-client model.
        cx.subscribe(
            &remote_manager,
            |this, _rm, event, cx| match event {
                RemoteManagerEvent::TerminalActivity(terminal_ids) => {
                    if !terminal_ids.is_empty() {
                        this.process_terminal_notifications(terminal_ids, cx);
                        // Answer (or, when disabled, drop) OSC 52 clipboard *read*
                        // requests for remote terminals. The clipboard physically
                        // lives on this client machine, so the reply must be
                        // produced here and written back over the terminal's
                        // RemoteTransport to the daemon PTY. Without this the dead
                        // local PTY loop's clipboard-read handling no longer runs,
                        // leaving remote OSC 52 reads unanswered.
                        this.process_clipboard_reads(terminal_ids, cx);
                    }
                }
            },
        )
        .detach();

        // Kill orphaned terminals when projects are deleted
        cx.observe(&workspace, move |this, workspace, cx| {
            let kills = workspace.update(cx, |ws, _| ws.drain_pending_terminal_kills());
            if !kills.is_empty() {
                let mut reg = this.terminals.lock();
                for tid in &kills {
                    reg.remove(tid);
                }
            }
        })
        .detach();

        // Flush soft-closed terminals on quit. Their grace timer can't fire once
        // the app is gone, so tear the PTYs down here — otherwise a terminal
        // closed seconds before quitting would leak its persistent (dtach/tmux)
        // session. on_app_quit fires for every exit path.
        cx.on_app_quit(move |this: &mut Self, cx| {
            let ids = this
                .workspace
                .update(cx, |ws, _| ws.drain_pending_closes());
            if !ids.is_empty() {
                let mut reg = this.terminals.lock();
                for tid in &ids {
                    reg.remove(tid);
                }
            }
            async {}
        })
        .detach();

        // UI-owned daemon lifecycle: kill the daemon WE spawned in
        // `--daemon-client` mode when the app quits. A daemon we merely attached
        // to (`spawned_daemon == None`) is left running for any other UIs.
        //
        // A UI-triggered restart (`perform_restart_daemon`) replaces our daemon
        // with a *detached* successor we hold no `Child` for — killing only the
        // original `Child` would orphan it. So when we own the lifecycle, also
        // reap the CURRENT daemon discovered from `remote.json`, whose pid
        // reflects any restarts. No-op when no restart happened (the discovered
        // process is the child we just killed, now dead) or when we attached.
        cx.on_app_quit(move |this: &mut Self, _cx| {
            let owned = this.spawned_daemon.is_some();
            if let Some(mut child) = this.spawned_daemon.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
            if owned
                && let Some(daemon) = okena_remote_server::local::running_daemon()
            {
                kill_process_by_pid(daemon.pid);
            }
            async {}
        })
        .detach();

        // Set up observer for detached terminals
        cx.observe(&workspace, move |this, workspace, cx| {
            this.handle_detached_terminals_changed(workspace, cx);
        })
        .detach();

        // Open an OS window per fresh `WorkspaceData.extra_windows` entry —
        // slice 05 keystone. The data-layer `Workspace::spawn_extra_window`
        // mutation push fires this observer; the diff against
        // `Okena.extra_windows` is the spawn signal.
        cx.observe(&workspace, |this, _workspace, cx| {
            this.handle_extra_windows_changed(cx);
        })
        .detach();

        // Client-owned window-layout autosave. The GUI (not the daemon) owns its
        // window PRESENTATION — which windows are open, their OS bounds, and
        // per-window viewport. The `observe_window_bounds → set_os_bounds` wiring
        // in `WindowView::new` and the spawn/close mutations all bump
        // `data_version`; this debounced observer persists the window layout to
        // window-layout.json (NEVER workspace.json — the daemon is its single
        // writer). Mirrors the daemon's workspace autosave. Without it, the
        // captured bounds + extra-window set are lost on exit and only one window
        // reopens next launch.
        {
            let save_pending = Arc::new(AtomicBool::new(false));
            let last_saved_version = Arc::new(AtomicU64::new(0));
            let workspace_for_save = workspace.clone();
            cx.observe(&workspace, move |_this, ws_entity, cx| {
                let current_version = ws_entity.read(cx).data_version();
                if current_version == last_saved_version.load(Ordering::Relaxed) {
                    return;
                }
                save_pending.store(true, Ordering::Relaxed);

                let save_pending = save_pending.clone();
                let last_saved = last_saved_version.clone();
                let workspace = workspace_for_save.clone();
                cx.spawn(async move |_, cx| {
                    smol::Timer::after(Duration::from_millis(500)).await;
                    if save_pending.swap(false, Ordering::Relaxed) {
                        let (data, version) = cx.update(|cx| {
                            let ws = workspace.read(cx);
                            (ws.data().clone(), ws.data_version())
                        });
                        let save_result = smol::unblock(move || {
                            crate::workspace::persistence::save_window_layout(&data)
                        })
                        .await;
                        match save_result {
                            Ok(()) => last_saved.store(version, Ordering::Relaxed),
                            Err(e) => log::error!("Failed to save window layout: {}", e),
                        }
                    }
                })
                .detach();
            })
            .detach();
        }

        // Scrub stale focus across every window's FocusManager on each
        // workspace change. Deleting a project from one window can leave
        // another window's focus pointing at a now-gone project; without
        // this, the orphaned window renders a ghost zoom of the deleted
        // project (or worse, panics on missing data).
        cx.observe(&workspace, |this, workspace, cx| {
            let valid_ids: HashSet<String> = workspace
                .read(cx)
                .projects()
                .iter()
                .map(|p| p.id.clone())
                .collect();
            let mut fms: Vec<Entity<crate::workspace::focus::FocusManager>> = Vec::with_capacity(1 + this.extra_windows.len());
            fms.push(this.main_window.read(cx).focus_manager());
            for view in this.extra_windows.values() {
                fms.push(view.read(cx).focus_manager());
            }
            for fm in fms {
                fm.update(cx, |fm, cx| {
                    if fm.clear_stale_focus(|id| valid_ids.contains(id)) {
                        cx.notify();
                    }
                });
            }
        })
        .detach();

        // Slice 07 cri 1: kick the extras observer once so persisted
        // `WorkspaceData.extra_windows` entries reopen at launch. The observer
        // above only fires when `workspace` notifies, but `Workspace::new` does
        // not notify on construction — without an explicit kick, persisted
        // extras would stay invisible until the user mutates the workspace.
        // Deferred via `cx.spawn` because `open_extra_window` captures
        // `cx.entity()` and calls `okena.update` inside `cx.open_window`'s
        // build closure; running synchronously inside `Okena::new` would touch
        // a half-constructed entity. By the time the spawned task body runs,
        // the entity is fully wrapped and `update` is safe.
        cx.spawn(async move |this: WeakEntity<Okena>, cx| {
            let _ = this.update(cx, |this, cx| {
                this.handle_extra_windows_changed(cx);
            });
        })
        .detach();

        // Note: updater is now handled by the okena-ext-updater extension.
        // GlobalUpdateInfo is set in main.rs via okena_ext_updater::init().

        manager
    }
}

impl Render for Okena {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.main_window.clone())
    }
}
