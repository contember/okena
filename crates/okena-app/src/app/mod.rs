mod detached_overlays;
mod detached_terminals;
mod extras;
mod notifications;

pub use detached_overlays::open_detached_overlay;

use crate::remote_client::manager::{RemoteConnectionManager, RemoteManagerEvent};
use crate::views::window::{TerminalsRegistry, WindowView};
use crate::workspace::state::{GlobalWorkspace, WindowId, Workspace, WorkspaceData};
use gpui::*;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

/// Identity guard for [`kill_process_by_pid`]: OS pids recycle, so a pid taken
/// from a possibly-stale `remote.json` may now belong to an unrelated process.
/// Only a process whose name or executable file name starts with "okena" (the
/// `okena`/`okena-daemon` binaries) may be killed. Pure so it's unit-testable.
fn is_okena_process(name: Option<&str>, exe_file_name: Option<&str>) -> bool {
    let is_okena = |s: &str| s.starts_with("okena");
    name.is_some_and(is_okena) || exe_file_name.is_some_and(is_okena)
}

/// Best-effort kill a process by pid — SIGKILL on Unix, `TerminateProcess` on
/// Windows, matching `std::process::Child::kill`. Used by the UI-owned daemon
/// lifecycle to reap a daemon we own but hold no `Child` for: a restart spawns a
/// *detached* successor, known to us only by the pid it advertises in
/// `remote.json`. A pid of 0 (unknown) or an already-dead process is a no-op.
/// Refuses (warn + skip) when the process at that pid doesn't look like an okena
/// binary — see [`is_okena_process`].
fn kill_process_by_pid(pid: u32) {
    if pid == 0 {
        return;
    }
    use sysinfo::{Pid, ProcessesToUpdate, System};
    let spid = Pid::from_u32(pid);
    let mut sys = System::new();
    sys.refresh_processes(ProcessesToUpdate::Some(&[spid]), true);
    if let Some(proc) = sys.process(spid) {
        let name = proc.name().to_str();
        let exe_file_name = proc
            .exe()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str());
        if !is_okena_process(name, exe_file_name) {
            log::warn!(
                "Refusing to kill pid {pid}: process {name:?} (exe {exe_file_name:?}) is not an okena binary — the pid was likely recycled"
            );
            return;
        }
        proc.kill();
    }
}

fn hand_off_ui_owned_daemon(mut spawned_child: Option<std::process::Child>) {
    let Some(daemon) = okena_remote_server::local::running_daemon() else {
        if let Some(child) = spawned_child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
        return;
    };
    if !daemon.ui_owned {
        log::info!("Leaving standalone local daemon pid {} running", daemon.pid);
        return;
    }

    let owns_current_process = spawned_child
        .as_ref()
        .is_some_and(|child| child.id() == daemon.pid);
    match okena_remote_server::local::request_local_shutdown(&daemon) {
        Ok(outcome) if outcome.accepted && outcome.active_clients == 0 => {
            if !okena_remote_server::local::wait_for_pid_exit(
                daemon.pid,
                Duration::from_secs(3),
            ) {
                if let Some(child) = spawned_child.as_mut().filter(|_| owns_current_process) {
                    let _ = child.kill();
                } else {
                    // No owned `Child` handle — e.g. a detached post-restart
                    // successor we only know by the pid it advertises. Reap it by
                    // pid, guarded by the is_okena_process recycle check, so a
                    // UI-owned daemon we own the lifecycle of never lingers.
                    log::info!(
                        "UI-owned daemon pid {} did not exit gracefully; reaping by pid",
                        daemon.pid
                    );
                    kill_process_by_pid(daemon.pid);
                }
            }
            if let Some(child) = spawned_child.as_mut().filter(|_| owns_current_process) {
                let _ = child.wait();
            }
        }
        Ok(outcome) if outcome.accepted => {
            log::info!(
                "UI-owned daemon shutdown armed until {} other client(s) disconnect",
                outcome.active_clients
            );
        }
        Ok(_) => {
            log::info!("Local daemon declined UI lifecycle handoff");
        }
        Err(error) => {
            if let Some(child) = spawned_child.as_mut().filter(|_| owns_current_process) {
                log::warn!("Shutdown request failed ({error}); killing owned daemon child");
                let _ = child.kill();
                let _ = child.wait();
            } else {
                log::warn!(
                    "Shutdown request failed ({error}); refusing to kill daemon without a matching child handle"
                );
            }
        }
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
    /// `Entity<WindowView>` is registered immediately after `cx.open_window`
    /// succeeds (see `extras.rs`).
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
    /// Child this GUI spawned, retained for owner-checked recovery fallback.
    spawned_daemon: Option<std::process::Child>,
    /// Single-flight guard for the local-daemon recovery task: set while a
    /// recovery loop runs so repeat `LocalConnectionFailed` events don't stack
    /// up parallel recoveries (each would re-run `ensure_local_daemon`).
    recovering: Arc<AtomicBool>,
    /// Set at the start of the quit handler so an in-flight (or newly triggered)
    /// recovery bails instead of resurrecting the connection or spawning a
    /// daemon we'd immediately orphan. Guards the part-B quit path's
    /// `remove_connection` from being mistaken for a recoverable failure.
    /// Also set from the main window's close handler (see main.rs) so pending
    /// extra-window forgets never commit during app teardown.
    quitting: Arc<AtomicBool>,
    /// Deferred forgets for OS-closed extra windows — see
    /// `extras.rs::handle_extra_window_os_close` for the quit-vs-close story.
    pending_extra_forgets: extras::PendingExtraForgets,
}

impl Okena {
    pub fn new(
        workspace_data: WorkspaceData,
        client_project_layouts: HashMap<String, crate::workspace::state::LayoutNode>,
        local_daemon: okena_remote_server::local::EnsuredDaemon,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // Create workspace entity. The GUI is always a thin daemon client: the
        // daemon owns persistence + the instance lock and is the single writer,
        // so the GUI's `Workspace` is a pure mirror with no autosave.
        let workspace = cx.new(|_cx| {
            let mut workspace = Workspace::new(workspace_data);
            workspace.seed_client_project_layouts(client_project_layouts);
            workspace
        });
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
        // daemon so its projects mirror into the GUI. A spawned child handle is
        // retained for bounded fallback cleanup if graceful UI lifecycle handoff
        // fails; an attached daemon is never force-killed. The connection uses a
        // fixed id so it's recognizable and dedup-safe, and is never written to
        // settings — `add_connection` does not persist, and the only insertion
        // site (`OverlayManagerEvent::RemoteConnected`) is never fired for it.
        let spawned_daemon = {
            let ensured = local_daemon;
            let cfg = ensured.daemon.connection_config(ensured.token.clone());
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
            recovering: Arc::new(AtomicBool::new(false)),
            quitting: Arc::new(AtomicBool::new(false)),
            pending_extra_forgets: extras::PendingExtraForgets::default(),
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
        {
            let remote_manager = remote_manager.clone();
            let settings = crate::settings::settings_entity(cx);
            cx.subscribe(&settings, move |_this, _settings, event, cx| {
                let crate::settings::SettingsEvent::Changed(settings) = event;
                match serde_json::to_value(settings) {
                    Ok(mut patch) => {
                        if let Some(object) = patch.as_object_mut() {
                            object.remove("remote_connections");
                        }
                        remote_manager.update(cx, |manager, cx| {
                            manager.send_action(
                                okena_transport::client::LOCAL_DAEMON_CONNECTION_ID,
                                okena_core::api::ActionRequest::SetSettings { patch },
                                cx,
                            );
                        });
                    }
                    Err(error) => {
                        log::error!("Failed to encode settings update: {error}");
                    }
                }
            })
            .detach();
        }

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
                RemoteManagerEvent::TerminalFocusRequested {
                    project_id,
                    terminal_id,
                    window: _,
                } => {
                    this.jump_to_terminal(project_id, terminal_id, cx);
                }
                // Local daemon connection dead-ended — re-run discovery/ensure so
                // the GUI recovers instead of staying wedged on a dead socket.
                RemoteManagerEvent::LocalConnectionFailed => {
                    this.recover_local_daemon(cx);
                }
                RemoteManagerEvent::SettingsChanged(settings) => {
                    let settings = settings.as_ref().clone();
                    let mode = settings.theme_mode;
                    let custom_id = settings.custom_theme_id.clone();
                    crate::settings::settings_entity(cx).update(cx, |state, cx| {
                        state.replace_from_daemon(settings.clone(), cx);
                    });

                    if let Some(global_theme) = cx.try_global::<crate::theme::GlobalTheme>() {
                        let theme = global_theme.0.clone();
                        theme.update(cx, |theme, cx| {
                            if mode == crate::theme::ThemeMode::Custom
                                && let Some(custom_id) = custom_id.as_ref()
                                && let Some((_, colors)) = crate::theme::load_custom_themes()
                                    .into_iter()
                                    .find(|(info, _)| info.id == format!("custom:{custom_id}"))
                            {
                                theme.set_custom_colors(colors);
                                theme.set_mode(crate::theme::ThemeMode::Custom);
                            } else {
                                theme.set_mode(mode);
                            }
                            cx.notify();
                        });
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

        // Hand UI-owned lifecycle to the daemon on quit. It exits after the
        // final client disconnects; standalone daemons are left running.
        cx.on_app_quit(move |this: &mut Self, cx| {
            // Stop any recovery from resurrecting the connection or spawning a
            // daemon while we tear down (esp. the remove_connection just below).
            this.quitting.store(true, Ordering::SeqCst);
            let (final_layout, project_layouts) = {
                let workspace = this.workspace.read(cx);
                (workspace.data().clone(), workspace.client_project_layouts())
            };
            // Disconnect our own loopback connection before handing lifecycle
            // to the daemon, so its live-client count excludes this GUI.
            this.remote_manager.update(cx, |rm, cx| {
                rm.remove_connection(
                    okena_transport::client::LOCAL_DAEMON_CONNECTION_ID,
                    cx,
                );
            });
            hand_off_ui_owned_daemon(this.spawned_daemon.take());
            async move {
                if let Err(error) = smol::unblock(move || {
                    crate::workspace::persistence::save_window_layout(
                        &final_layout,
                        project_layouts,
                    )
                })
                .await
                {
                    log::error!("Failed to save final window layout: {error}");
                }
            }
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
                        let (data, project_layouts, version) = cx.update(|cx| {
                            let ws = workspace.read(cx);
                            (
                                ws.data().clone(),
                                ws.client_project_layouts(),
                                ws.data_version(),
                            )
                        });
                        let save_result = smol::unblock(move || {
                            crate::workspace::persistence::save_window_layout(
                                &data,
                                project_layouts,
                            )
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

        // Linux starts its platform event loop only after the application
        // startup callback returns. Use the foreground executor so restored
        // Wayland windows are created on the first live event-loop turn, while
        // retaining Okena strongly until the one-shot restore has completed.
        let okena = cx.entity();
        cx.spawn(async move |_, cx| {
            cx.update(|cx| {
                okena.update(cx, |this, cx| {
                    this.handle_extra_windows_changed(cx);
                });
            });
        })
        .detach();

        // Note: updater is now handled by the okena-ext-updater extension.
        // GlobalUpdateInfo is set in main.rs via okena_ext_updater::init().

        manager
    }
}

impl Okena {
    /// Mark the app as quitting before the platform loop stops. Called from
    /// the main window's close handler (main.rs) ahead of `cx.quit()` so
    /// deferred extra-window forgets and daemon recovery bail immediately —
    /// on_app_quit alone would set this only after close events for every
    /// window have already been processed.
    pub fn note_quitting(&self) {
        self.quitting.store(true, Ordering::SeqCst);
    }
}

impl Render for Okena {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.main_window.clone())
    }
}

/// After this many consecutive failed recovery attempts, surface one error
/// toast. We do NOT stop retrying afterwards: the local daemon backs the whole
/// GUI, so giving up would leave the app dead until a manual restart — exactly
/// the bug this heals. Instead we keep retrying at the 30s cap (see
/// [`recovery_backoff_delay`]) and toast only once, so we never spam.
const RECOVERY_TOAST_AFTER_ATTEMPTS: u32 = 5;

/// Attach patience for the recovery path's `ensure` calls. Shorter than the 30s
/// startup default so a live-but-unreachable daemon (which makes `ensure` error
/// only after the attach timeout) is escalated on sooner.
const RECOVERY_ATTACH_TIMEOUT: Duration = Duration::from_secs(8);

/// Spawn budget for the recovery path — the full startup patience, NOT the short
/// attach patience: daemon boot loads the workspace before the server binds and
/// can take many seconds under load, and `ensure` SIGKILLs its own child on this
/// deadline — a short one would kill every mid-boot respawn forever.
const RECOVERY_SPAWN_TIMEOUT: Duration = Duration::from_secs(30);

/// After this many consecutive failed recovery attempts, escalate: if a live but
/// unreachable local daemon is what's blocking us, kill it so the next attempt
/// takes the spawn path instead of forever re-hitting the attach timeout.
const RECOVERY_ESCALATE_AFTER_ATTEMPTS: u32 = 2;

/// Confirm-probe timeout before an escalation kill. Deliberately much longer
/// than the 300ms probe `ensure` uses internally: under a system-wide stall a
/// slow-but-healthy daemon must not be misread as dead and killed, while a truly
/// dead socket still fails the connect instantly, so the extra patience is free.
const RECOVERY_HEALTH_PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// Whether recovery should escalate to killing the local daemon. Escalate only
/// once we've failed enough consecutive times AND a live-but-unreachable daemon
/// is the thing blocking us (so we never kill a daemon that's merely absent, nor
/// one that's actually healthy). Pure so the decision is unit-testable.
fn should_escalate_recovery(
    failed_attempts: u32,
    live_unreachable_daemon: bool,
    owns_daemon: bool,
) -> bool {
    failed_attempts >= RECOVERY_ESCALATE_AFTER_ATTEMPTS
        && live_unreachable_daemon
        && owns_daemon
}

/// Backoff before the next local-daemon recovery attempt, given how many have
/// already failed. Ramps 1 → 2 → 5 → 10s then caps at 30s: quick enough to heal
/// a brief daemon gap (the common fast-restart race) yet without a spawn storm
/// when the daemon stays down. Pure so the schedule is unit-testable.
fn recovery_backoff_delay(failed_attempts: u32) -> Duration {
    let secs = match failed_attempts {
        0 | 1 => 1,
        2 => 2,
        3 => 5,
        4 => 10,
        _ => 30,
    };
    Duration::from_secs(secs)
}

impl Okena {
    /// Self-heal the implicit local-daemon loopback connection after it hit a
    /// terminal failure (both dead-end client paths surface `Error`). Re-runs
    /// `ensure_local_daemon` — attach to a live daemon or spawn a fresh one —
    /// then re-points the loopback connection at the new endpoint/token.
    /// Single-flight and quit-aware; mirrors `perform_restart_daemon`'s pattern
    /// of running the blocking remote-server call on the background pool.
    fn recover_local_daemon(&mut self, cx: &mut Context<Self>) {
        if self.quitting.load(Ordering::SeqCst) {
            return;
        }
        // Single-flight: a recovery already running will re-point the connection
        // when it succeeds, so further failure events until then are redundant.
        if self.recovering.swap(true, Ordering::SeqCst) {
            return;
        }

        let recovering = self.recovering.clone();
        let quitting = self.quitting.clone();
        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            let mut failed_attempts: u32 = 0;
            loop {
                if quitting.load(Ordering::SeqCst) {
                    break;
                }

                // `ensure_local_daemon_with_timeouts` blocks (short attach
                // patience, full spawn budget); run it on the blocking pool
                // like `perform_restart_daemon` does with restart.
                let outcome = cx
                    .background_executor()
                    .spawn(async move {
                        okena_remote_server::local::ensure_local_daemon_with_timeouts(
                            RECOVERY_ATTACH_TIMEOUT,
                            RECOVERY_SPAWN_TIMEOUT,
                        )
                    })
                    .await;

                match outcome {
                    Ok(ensured) => {
                        // Hold `ensured` outside the closure: if the entity is
                        // gone the closure never runs, and dropping a Child does
                        // not kill it — we'd orphan a daemon we just spawned.
                        let mut ensured = Some(ensured);
                        let applied = this.update(cx, |this, cx| {
                            if let Some(ensured) = ensured.take() {
                                this.apply_recovered_daemon(ensured, cx);
                            }
                        });
                        // Entity dropped mid-recovery: best-effort reap the child.
                        if applied.is_err()
                            && let Some(mut child) = ensured.and_then(|e| e.spawned)
                        {
                            let _ = child.kill();
                            let _ = child.wait();
                        }
                        break;
                    }
                    Err(msg) => {
                        failed_attempts += 1;
                        log::warn!("Local daemon recovery attempt {failed_attempts} failed: {msg}");

                        // Escalation: a live-but-unreachable daemon makes every
                        // `ensure` error at the attach timeout, forever. Once we've
                        // failed enough times, kill that wedged daemon so the next
                        // `ensure` takes the spawn path. Never while quitting; this
                        // loop only ever handles the local daemon.
                        if failed_attempts >= RECOVERY_ESCALATE_AFTER_ATTEMPTS
                            && !quitting.load(Ordering::SeqCst)
                        {
                            let stuck_daemon = cx
                                .background_executor()
                                .spawn(async {
                                    okena_remote_server::local::running_daemon().filter(|d| {
                                        !okena_remote_server::local::daemon_endpoint_responds(
                                            d,
                                            RECOVERY_HEALTH_PROBE_TIMEOUT,
                                        )
                                    })
                                })
                                .await;
                            if let Some(daemon) = stuck_daemon
                            {
                                let owns_daemon = this
                                    .update(cx, |this, _cx| {
                                        this.spawned_daemon
                                            .as_ref()
                                            .is_some_and(|child| child.id() == daemon.pid)
                                    })
                                    .unwrap_or(false);
                                if should_escalate_recovery(
                                    failed_attempts,
                                    true,
                                    owns_daemon,
                                ) {
                                    log::warn!(
                                        "Owned local daemon pid {} is live but unreachable after {failed_attempts} failed recovery attempts; killing it so the next attempt respawns",
                                        daemon.pid
                                    );
                                    kill_process_by_pid(daemon.pid);
                                    let _ = this.update(cx, |this, _cx| {
                                        if let Some(child) = this.spawned_daemon.as_mut()
                                            && child.id() == daemon.pid
                                        {
                                            let _ = child.wait();
                                            this.spawned_daemon = None;
                                        }
                                    });
                                } else {
                                    log::warn!(
                                        "Attached local daemon pid {} is unreachable; refusing to kill a process this GUI did not spawn",
                                        daemon.pid
                                    );
                                }
                            }
                        }

                        // Update every attempt so a dropped entity ends the loop
                        // (and the app isn't left spawning daemons post-quit).
                        let should_toast = failed_attempts == RECOVERY_TOAST_AFTER_ATTEMPTS;
                        let alive = this
                            .update(cx, |_this, cx| {
                                if should_toast {
                                    crate::workspace::toast::ToastManager::error(
                                        "Local daemon unreachable; still retrying in the background…"
                                            .to_string(),
                                        cx,
                                    );
                                }
                            })
                            .is_ok();
                        if !alive {
                            break;
                        }
                        cx.background_executor()
                            .timer(recovery_backoff_delay(failed_attempts))
                            .await;
                    }
                }
            }
            recovering.store(false, Ordering::SeqCst);
        })
        .detach();
    }

    /// Apply a freshly-ensured daemon on the GPUI thread: re-point the loopback
    /// connection at its endpoint/token and adopt any child we spawned. Bails
    /// (reaping a just-spawned daemon) if a quit began while we were ensuring.
    fn apply_recovered_daemon(
        &mut self,
        ensured: okena_remote_server::local::EnsuredDaemon,
        cx: &mut Context<Self>,
    ) {
        // Raced a quit: don't resurrect the connection, and don't orphan a daemon
        // we just spawned (dropping the Child doesn't kill it on Unix).
        if self.quitting.load(Ordering::SeqCst) {
            if let Some(mut child) = ensured.spawned {
                let _ = child.kill();
                let _ = child.wait();
            }
            return;
        }

        let cfg = ensured.daemon.connection_config(ensured.token.clone());
        self.remote_manager.update(cx, |rm, cx| {
            rm.redirect_and_reconnect(
                okena_transport::client::LOCAL_DAEMON_CONNECTION_ID,
                cfg,
                ensured.token.clone(),
                cx,
            );
        });

        // If we spawned a fresh daemon, we now own it. Reap the old (dead) child
        // handle first so we don't leak a zombie, then take over the new one.
        match ensured.spawned {
            Some(child) => {
                if let Some(mut old) = self.spawned_daemon.take() {
                    let _ = old.kill();
                    let _ = old.wait();
                }
                self.spawned_daemon = Some(child);
            }
            None => {
                // Attach path: reap our previous child only if it ALREADY exited
                // (e.g. after an escalation kill) so no zombie outlives recovery.
                // try_wait never touches a live child — the daemon we attached to
                // may well BE that child.
                if let Some(child) = self.spawned_daemon.as_mut()
                    && matches!(child.try_wait(), Ok(Some(_)))
                {
                    self.spawned_daemon = None;
                }
            }
        }

        crate::workspace::toast::ToastManager::info("Local daemon reconnected".to_string(), cx);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        is_okena_process, recovery_backoff_delay, should_escalate_recovery,
        RECOVERY_ESCALATE_AFTER_ATTEMPTS, RECOVERY_TOAST_AFTER_ATTEMPTS,
    };
    use std::time::Duration;

    #[test]
    fn okena_process_identity_guard() {
        // Our binaries — killable.
        assert!(is_okena_process(Some("okena"), None));
        assert!(is_okena_process(Some("okena-daemon"), None));
        assert!(is_okena_process(None, Some("okena-daemon")));
        // Exe name matches even when the reported name doesn't (truncation etc.).
        assert!(is_okena_process(Some("some-thread"), Some("okena")));
        // Recycled pid pointing at an unrelated process — never kill.
        assert!(!is_okena_process(Some("cargo"), Some("cargo")));
        assert!(!is_okena_process(Some("firefox"), None));
        assert!(!is_okena_process(None, None));
    }

    #[test]
    fn escalates_only_after_threshold_and_when_stuck() {
        // Below the threshold: never escalate, even if a daemon is stuck.
        assert!(!should_escalate_recovery(0, true, true));
        assert!(!should_escalate_recovery(
            RECOVERY_ESCALATE_AFTER_ATTEMPTS - 1,
            true,
            true,
        ));
        // At/after the threshold WITH a live-unreachable daemon: escalate.
        assert!(should_escalate_recovery(
            RECOVERY_ESCALATE_AFTER_ATTEMPTS,
            true,
            true,
        ));
        assert!(should_escalate_recovery(
            RECOVERY_ESCALATE_AFTER_ATTEMPTS + 5,
            true,
            true,
        ));
        // No live-unreachable daemon (absent or healthy): never kill.
        assert!(!should_escalate_recovery(
            RECOVERY_ESCALATE_AFTER_ATTEMPTS + 5,
            false,
            true,
        ));
        // A daemon this GUI merely attached to is never killable.
        assert!(!should_escalate_recovery(
            RECOVERY_ESCALATE_AFTER_ATTEMPTS + 5,
            true,
            false,
        ));
    }

    #[test]
    fn backoff_ramps_then_caps_at_30s() {
        assert_eq!(recovery_backoff_delay(0), Duration::from_secs(1));
        assert_eq!(recovery_backoff_delay(1), Duration::from_secs(1));
        assert_eq!(recovery_backoff_delay(2), Duration::from_secs(2));
        assert_eq!(recovery_backoff_delay(3), Duration::from_secs(5));
        assert_eq!(recovery_backoff_delay(4), Duration::from_secs(10));
        assert_eq!(recovery_backoff_delay(5), Duration::from_secs(30));
        assert_eq!(recovery_backoff_delay(50), Duration::from_secs(30));
    }

    #[test]
    fn backoff_is_monotonic_nondecreasing() {
        // Never speeds up as failures accumulate — guards against a spawn storm.
        let mut prev = Duration::ZERO;
        for n in 0..12 {
            let d = recovery_backoff_delay(n);
            assert!(d >= prev, "delay must not decrease at attempt {n}");
            prev = d;
        }
        // The one-shot give-up toast fires during the ramp, not only at the cap.
        const { assert!(RECOVERY_TOAST_AFTER_ATTEMPTS >= 4) };
    }
}
