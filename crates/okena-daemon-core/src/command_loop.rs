//! GPUI-free remote command loop: the headless daemon's faithful port of the
//! GUI's `remote_command_loop` (in `okena-app`'s `app/remote_commands.rs`).
//!
//! The GUI version runs on the GPUI main thread and dispatches each
//! [`RemoteCommand`] into `Entity<Workspace>` / `Entity<ServiceManager>` via
//! `cx.update(|cx| …)` / `entity.read(cx)` / `entity.update(cx, …)`. The daemon
//! has no entity graph: it holds the same state behind
//! `Arc<parking_lot::Mutex<…>>` and drives the identical
//! `okena-app-core` / `okena-services` code paths against the daemon reactor cx
//! types (see [`crate::workspace_cx`] / [`crate::service_cx`]).
//!
//! Each arm reproduces the GUI behavior arm-for-arm:
//!
//! * **Service actions** lock the [`ServiceManager`], mint a
//!   [`DaemonServiceCx`](crate::service_cx::DaemonServiceCx) from the shared
//!   [`ServiceReactorRef`], and call the same method with the same project-path
//!   lookup + "project not found" error as the GUI.
//! * **App-scoped settings/theme** delegate to [`DaemonConfig`] (the GUI's
//!   `remote_config` counterpart).
//! * **Command palette** is unavailable in the daemon (no GUI action registry):
//!   `ListActions` returns an empty list, `InvokeAction` returns an error.
//! * **Workspace-scoped actions** run through
//!   [`execute_action`](okena_app_core::workspace::actions::execute::execute_action)
//!   against [`WindowId::Main`] (the daemon serves a single synthetic main
//!   window, mirroring headless mode).
//! * **`GetState`** builds the [`StateResponse`](okena_core::api::StateResponse)
//!   the same way the GUI does, with the single synthetic `"main"` window.
//!
//! ## Lock discipline
//!
//! Every arm is fully synchronous: it never `.await`s while a state guard is
//! held, so each guard drops at the arm's end before the loop's next
//! `recv().await`. This mirrors the established daemon pattern in
//! [`crate::pty_loop::handle_exits`]. The single `GetState`/service-action arms
//! that touch both the workspace and service-manager locks take the workspace
//! lock first, then the service-manager lock (consistent order), and both drop
//! before looping.

use std::collections::HashMap;
use std::sync::Arc;

use okena_app_core::workspace::actions::execute::{
    ensure_terminal, execute_action, spawn_uninitialized_terminals,
};
use okena_app_core::remote_snapshot::build_state_response;
use okena_core::api::{
    ActionRequest, ApiGitStatus, ApiServiceInfo, ApiWindow, CommandResult,
};
use okena_core::git_poll::{git_poll_trigger_for_action, GitPollTrigger};
use okena_remote_server::bridge::{BridgeMessage, BridgeReceiver, RemoteCommand};
use okena_services::manager::ServiceManager;
use okena_terminal::backend::TerminalBackend;
use okena_terminal::TerminalsRegistry;
use okena_workspace::actions::soft_close::{
    begin_soft_close_flow, close_now_flow, probe_busy, undo_soft_close_flow,
};
use okena_workspace::focus::FocusManager;
use okena_workspace::persistence::AppSettings;
use okena_workspace::state::{WindowId, Workspace};
use parking_lot::Mutex;
use tokio::sync::watch;

use crate::daemon_config::{get_settings_schema, DaemonConfig};
use crate::service_cx::ServiceReactorRef;
use crate::soft_close::SoftCloseDeadlines;
use crate::workspace_cx::DaemonWorkspaceCx;

/// Parse a wire-format window id into a [`WindowId`].
///
/// GPUI-free copy of the GUI's `remote_commands::parse_window_id`. `"main"`
/// maps to [`WindowId::Main`]; any other string is parsed as a UUID and, on
/// success, wrapped in [`WindowId::Extra`]. A malformed UUID returns `None` so
/// the caller can reject the action with an "invalid window id" error rather
/// than silently routing it to the wrong window.
fn parse_window_id(s: &str) -> Option<WindowId> {
    if s == "main" {
        Some(WindowId::Main)
    } else {
        uuid::Uuid::parse_str(s).ok().map(WindowId::Extra)
    }
}

fn claim_input_resize_owner(action: &ActionRequest, owner_id: &str) {
    let terminal_id = match action {
        ActionRequest::SendText { terminal_id, .. }
        | ActionRequest::SendBytes { terminal_id, .. }
        | ActionRequest::RunCommand { terminal_id, .. }
        | ActionRequest::SendSpecialKey { terminal_id, .. } => Some(terminal_id.as_str()),
        _ => None,
    };

    if let Some(terminal_id) = terminal_id {
        okena_terminal::terminal::claim_resize_authority_remote_owner(terminal_id, owner_id);
    }
}

fn send_git_poll_trigger_after_success(
    result: &CommandResult,
    trigger: Option<GitPollTrigger>,
    tx: &tokio::sync::mpsc::UnboundedSender<GitPollTrigger>,
) {
    if matches!(result, CommandResult::Ok(_))
        && let Some(trigger) = trigger
    {
        let _ = tx.send(trigger);
    }
}

fn publish_config_change_after_success(
    result: &CommandResult,
    state_version: &watch::Sender<u64>,
) {
    if matches!(result, CommandResult::Ok(_)) {
        state_version.send_modify(|version| *version = version.wrapping_add(1));
    }
}

/// GPUI-free remote command loop for the headless daemon.
///
/// Processes [`RemoteCommand`]s off the [`BridgeReceiver`] until every bridge
/// sender is dropped (server shutdown), replying via each message's `oneshot`
/// when present. The single dormant `FocusManager` is owned by the loop (which
/// is single-threaded), mirroring the GUI's per-window focus-manager but with no
/// view to drive.
// Bridge loop: each param is a distinct channel / shared-state dependency.
#[allow(clippy::too_many_arguments)]
pub async fn daemon_command_loop(
    bridge_rx: BridgeReceiver,
    backend: Arc<dyn TerminalBackend>,
    workspace: Arc<Mutex<Workspace>>,
    workspace_tick: watch::Sender<u64>,
    hook_runner: Option<okena_hooks::HookRunner>,
    hook_monitor: Option<okena_hooks::HookMonitor>,
    terminals: TerminalsRegistry,
    state_version: Arc<watch::Sender<u64>>,
    git_status_tx: Arc<watch::Sender<HashMap<String, ApiGitStatus>>>,
    service_manager: Arc<Mutex<ServiceManager>>,
    service_tick: watch::Sender<u64>,
    runtime: tokio::runtime::Handle,
    settings: Arc<Mutex<AppSettings>>,
    mut daemon_config: DaemonConfig,
    deadlines: SoftCloseDeadlines,
    git_poll_trigger_tx: tokio::sync::mpsc::UnboundedSender<GitPollTrigger>,
) {
    // Single dormant "main" FocusManager. The loop is single-threaded, so it
    // owns the FM directly instead of resolving a per-window entity like the
    // GUI. Focus state never drives a render here, so it is effectively dormant.
    let mut focus_manager = FocusManager::new();

    // Shared service reactor: built once, `cx()` re-borrowed per service arm.
    // It re-locks `service_manager` internally on reentry, so the loop locks the
    // manager itself only while the cx is alive — never across an await.
    let service_reactor =
        ServiceReactorRef::new(service_manager.clone(), runtime.clone(), service_tick.clone());

    loop {
        let msg: BridgeMessage = match bridge_rx.recv().await {
            Ok(m) => m,
            Err(_) => break,
        };

        let command = match msg.command {
            // Identityless actions (HTTP /v1/actions: CLI, agents) do NOT
            // touch resize authority — nulling the owner here handed the next
            // arriving resize to a random client. Only input from an
            // identified WS connection ("someone typed at that window")
            // transfers ownership.
            RemoteCommand::ActionFromConnection {
                action,
                connection_id,
            } => {
                claim_input_resize_owner(&action, &connection_id);
                RemoteCommand::Action(action)
            }
            command => command,
        };

        let result: CommandResult = match command {
            RemoteCommand::Action(action) => match action {
                // ── Service actions ──────────────────────────────────────────
                ActionRequest::StartService { project_id, service_name } => {
                    let mut sm = service_manager.lock();
                    let mut cx = service_reactor.cx();
                    sm.start_service_action(&project_id, &service_name, &mut cx)
                }
                ActionRequest::StopService { project_id, service_name } => {
                    let mut sm = service_manager.lock();
                    let mut cx = service_reactor.cx();
                    sm.stop_service_action(&project_id, &service_name, &mut cx)
                }
                ActionRequest::RestartService { project_id, service_name } => {
                    let mut sm = service_manager.lock();
                    let mut cx = service_reactor.cx();
                    sm.restart_service_action(&project_id, &service_name, &mut cx)
                }
                ActionRequest::StartAllServices { project_id } => {
                    let mut sm = service_manager.lock();
                    let mut cx = service_reactor.cx();
                    sm.start_all_action(&project_id, &mut cx)
                }
                ActionRequest::StopAllServices { project_id } => {
                    let mut sm = service_manager.lock();
                    let mut cx = service_reactor.cx();
                    sm.stop_all_action(&project_id, &mut cx)
                }
                ActionRequest::ReloadServices { project_id } => {
                    let mut sm = service_manager.lock();
                    let mut cx = service_reactor.cx();
                    sm.reload_services_action(&project_id, &mut cx)
                }

                // ── App-scoped: settings / theme ─────────────────────────────
                ActionRequest::GetSettings => daemon_config.get_settings(),
                ActionRequest::GetSettingsSchema => get_settings_schema(),
                ActionRequest::SetSettings { patch } => {
                    let result = daemon_config.set_settings(patch);
                    publish_config_change_after_success(&result, &state_version);
                    result
                }
                ActionRequest::GetThemes => daemon_config.get_themes(),
                ActionRequest::GetTheme { id } => daemon_config.get_theme(id),
                ActionRequest::SetTheme { id } => {
                    let result = daemon_config.set_theme(id);
                    publish_config_change_after_success(&result, &state_version);
                    result
                }
                ActionRequest::SaveCustomTheme { id, config, activate } => {
                    let result = daemon_config.save_custom_theme(id, config, activate);
                    publish_config_change_after_success(&result, &state_version);
                    result
                }

                // ── Command palette ──────────────────────────────────────────
                // The daemon has no GUI action registry, so there are no
                // invokable commands to list or dispatch (the agreed parity
                // decision; the GUI's headless mode rejects these too).
                ActionRequest::ListActions => {
                    CommandResult::Ok(Some(serde_json::json!({ "actions": [] })))
                }
                ActionRequest::InvokeAction { .. } => {
                    CommandResult::Err("command palette unavailable in daemon mode".to_string())
                }

                // ── Soft-close: undo (restore the ejected pane) ──────────────
                ActionRequest::UndoSoftClose { terminal_id } => {
                    let mut cx = DaemonWorkspaceCx::new(&workspace_tick, &hook_runner, &hook_monitor);
                    let mut ws = workspace.lock();
                    undo_soft_close_flow(
                        &deadlines,
                        &mut ws,
                        &mut focus_manager,
                        &terminals,
                        &terminal_id,
                        &mut cx,
                    );
                    CommandResult::Ok(None)
                }

                // ── Soft-close: finalize now ("Close now") ───────────────────
                ActionRequest::CloseTerminalNow { terminal_id } => {
                    let mut cx = DaemonWorkspaceCx::new(&workspace_tick, &hook_runner, &hook_monitor);
                    let mut ws = workspace.lock();
                    close_now_flow(
                        &deadlines,
                        &mut ws,
                        &*backend,
                        &terminals,
                        &terminal_id,
                        &mut cx,
                    );
                    CommandResult::Ok(None)
                }

                // ── Close terminal: grace-aware soft close ───────────────────
                // Faithful daemon-side port of the GUI's optimistic close. A
                // busy terminal is ejected from the layout (mirrors to clients)
                // but its PTY is kept alive for the grace period; the finalizer
                // loop ([`crate::soft_close::run_soft_close_poll`]) kills it on
                // expiry. Idle terminals and `grace == 0` keep the immediate
                // close. The Undo / Close-now toast buttons are built here but
                // are inert until the client wires their actions.
                ActionRequest::CloseTerminal { project_id, terminal_id } => {
                    let grace = settings.lock().terminal_close_grace_secs;

                    if grace == 0 {
                        // Feature off → immediate close (unchanged behavior).
                        // Snapshot settings BEFORE locking the workspace.
                        let app_settings = settings.lock().clone();
                        let mut ws = workspace.lock();
                        run_main_workspace_action(
                            ActionRequest::CloseTerminal { project_id, terminal_id },
                            &mut ws,
                            &mut focus_manager,
                            &backend,
                            &terminals,
                            &app_settings,
                            &workspace_tick,
                            &hook_runner,
                            &hook_monitor,
                        )
                    } else {
                        // Probe busy-ness OFF the loop thread (forks
                        // tmux/lsof/pgrep). Hold NO locks across the await. Also
                        // grab the foreground command for the toast label.
                        let probe = {
                            let backend = backend.clone();
                            let tid = terminal_id.clone();
                            runtime.spawn_blocking(move || probe_busy(&*backend, &tid))
                        };
                        let (busy, command) = probe.await.unwrap_or((false, None));

                        if !busy {
                            // Idle → immediate close.
                            let app_settings = settings.lock().clone();
                            let mut ws = workspace.lock();
                            run_main_workspace_action(
                                ActionRequest::CloseTerminal { project_id, terminal_id },
                                &mut ws,
                                &mut focus_manager,
                                &backend,
                                &terminals,
                                &app_settings,
                                &workspace_tick,
                                &hook_runner,
                                &hook_monitor,
                            )
                        } else {
                            // Soft close: eject the pane (mirrors back), keep the
                            // PTY, surface an Undo/Close-now toast, and arm the
                            // grace deadline for the finalizer loop. `None` from
                            // the flow means the terminal wasn't in the layout —
                            // fall back to an immediate close.
                            let toast = {
                                let mut cx = DaemonWorkspaceCx::new(
                                    &workspace_tick,
                                    &hook_runner,
                                    &hook_monitor,
                                );
                                let mut ws = workspace.lock();
                                begin_soft_close_flow(
                                    &deadlines,
                                    &mut ws,
                                    &mut focus_manager,
                                    &terminals,
                                    &project_id,
                                    &terminal_id,
                                    grace,
                                    command,
                                    &mut cx,
                                )
                            };
                            match toast {
                                Some(toast) => {
                                    if let Some(hm) = &hook_monitor {
                                        hm.push_toast(toast);
                                    }
                                    CommandResult::Ok(None)
                                }
                                None => {
                                    // Not in the layout — immediate close.
                                    let app_settings = settings.lock().clone();
                                    let mut ws = workspace.lock();
                                    run_main_workspace_action(
                                        ActionRequest::CloseTerminal {
                                            project_id,
                                            terminal_id,
                                        },
                                        &mut ws,
                                        &mut focus_manager,
                                        &backend,
                                        &terminals,
                                        &app_settings,
                                        &workspace_tick,
                                        &hook_runner,
                                        &hook_monitor,
                                    )
                                }
                            }
                        }
                    }
                }

                // ── Create worktree: run the blocking git off the reactor ────
                // `git fetch` + `git worktree add` are network/disk-heavy (up to
                // seconds on a cold fetch). Routing them through the synchronous
                // `execute_action` path holds the workspace lock the whole time,
                // stalling EVERY other daemon action. Split it (mirroring the
                // `CloseTerminal` busy-probe): resolve paths under a brief lock,
                // run the git on a blocking thread with NO lock held, then do the
                // fast workspace mutation (register project + fire on_worktree_create
                // + spawn PTYs) under the lock.
                ActionRequest::CreateWorktree { project_id, branch, create_branch } => {
                    // Phase 0: resolve paths. Read settings first, then the
                    // workspace (settings-before-workspace lock order), and drop
                    // both before the blocking git runs.
                    let template = settings.lock().worktree.path_template.clone();
                    let prepared = {
                        let ws = workspace.lock();
                        ws.project(&project_id).map(|p| {
                            let (git_root, subdir) = okena_git::resolve_git_root_and_subdir(
                                std::path::Path::new(&p.path),
                            );
                            let (worktree_path, wt_project_path) =
                                okena_git::compute_target_paths(&git_root, &subdir, &template, &branch);
                            (git_root, worktree_path, wt_project_path)
                        })
                    };

                    match prepared {
                        None => CommandResult::Err(format!("project not found: {project_id}")),
                        Some((git_root, worktree_path, wt_project_path)) => {
                            // Phase 1: create the worktree OFF the command-loop
                            // thread (no workspace lock held). For a NEW branch, base
                            // it on the LOCAL `origin/<default>` — NO blocking network
                            // fetch — so the window appears immediately, and return
                            // the default branch so phase 3 can freshen it to the true
                            // remote tip in the background.
                            let git = {
                                let git_root = git_root.clone();
                                let branch = branch.clone();
                                let target = std::path::PathBuf::from(&worktree_path);
                                runtime
                                    .spawn_blocking(move || {
                                        if create_branch {
                                            let default = okena_git::get_default_branch(&git_root);
                                            okena_git::create_worktree_with_start_point(
                                                &git_root,
                                                &branch,
                                                &target,
                                                default.as_deref(),
                                            )
                                            .map(|()| default)
                                        } else {
                                            okena_git::create_worktree(&git_root, &branch, &target, false)
                                                .map(|()| None)
                                        }
                                    })
                                    .await
                            };

                            match git {
                                Err(join) => {
                                    CommandResult::Err(format!("worktree creation task failed: {join}"))
                                }
                                Ok(Err(e)) => CommandResult::Err(match &e {
                                    okena_git::GitError::WorktreeExists { path } => format!(
                                        "Directory '{}' is already an active worktree",
                                        path.display()
                                    ),
                                    other => other.to_string(),
                                }),
                                Ok(Ok(default_branch)) => {
                                    // Phase 2: fast workspace mutation UNDER the lock
                                    // (register + fire hooks + spawn the initial PTY).
                                    let app_settings = settings.lock().clone();
                                    let mut cx = DaemonWorkspaceCx::new(
                                        &workspace_tick,
                                        &hook_runner,
                                        &hook_monitor,
                                    );
                                    let mut ws = workspace.lock();
                                    match ws.register_worktree_project(
                                        &project_id,
                                        &branch,
                                        &git_root,
                                        &worktree_path,
                                        &wt_project_path,
                                        &app_settings.hooks,
                                        WindowId::Main,
                                        &mut cx,
                                    ) {
                                        Ok(new_id) => {
                                            let _ = spawn_uninitialized_terminals(
                                                &mut ws,
                                                &new_id,
                                                &*backend,
                                                &terminals,
                                                &app_settings,
                                                None,
                                                &mut cx,
                                            );
                                            drop(ws);
                                            // Phase 3: freshen in the background — fetch
                                            // origin/<default> and fast-forward the new
                                            // branch to the true remote tip (ff-only,
                                            // never clobbers). The window is already up;
                                            // this runs off the reactor.
                                            if let Some(default_branch) = default_branch {
                                                let git_root = git_root.clone();
                                                let worktree_path = worktree_path.clone();
                                                tokio::task::spawn_local(async move {
                                                    let _ = tokio::task::spawn_blocking(move || {
                                                        okena_git::fetch_and_fast_forward(
                                                            &git_root,
                                                            std::path::Path::new(&worktree_path),
                                                            &default_branch,
                                                        )
                                                    })
                                                    .await;
                                                });
                                            }
                                            CommandResult::Ok(Some(serde_json::json!({
                                                "project_id": new_id,
                                                "path": wt_project_path,
                                            })))
                                        }
                                        Err(e) => CommandResult::Err(e),
                                    }
                                }
                            }
                        }
                    }
                }

                // ── Close worktree: run the blocking git removal off the reactor ─
                // A plain (non-merge) close of a worktree with NO before_remove
                // hook does a bare `git worktree remove`, whose expensive status
                // checks + directory delete can block for SECONDS on a busy
                // worktree (Docker holding files, a large tree), freezing the whole
                // UI. Run that git off the command-loop thread: snapshot inputs +
                // fire on_worktree_close under a brief lock, remove the git worktree
                // on spawn_blocking with NO lock held, then finalize state under the
                // lock. Merge closes and before_remove-hook closes (which defer
                // removal to the PTY-exit handler) keep the existing sync path.
                ActionRequest::CloseWorktree { project_id, merge, stash, fetch, push, delete_branch } => {
                    let global_hooks = settings.lock().hooks.clone();
                    let plan = {
                        let mut cx = DaemonWorkspaceCx::new(&workspace_tick, &hook_runner, &hook_monitor);
                        let mut ws = workspace.lock();
                        let fast = !merge
                            && ws.project(&project_id).is_some_and(|p| {
                                p.worktree_info.is_some()
                                    && p.hooks.worktree.before_remove.is_none()
                                    && global_hooks.worktree.before_remove.is_none()
                            });
                        if fast {
                            Some(ws.begin_worktree_removal(&project_id, &global_hooks, &mut cx))
                        } else {
                            None
                        }
                    };
                    match plan {
                        // Not the fast path — run the full close pipeline synchronously.
                        None => {
                            let app_settings = settings.lock().clone();
                            let mut ws = workspace.lock();
                            run_main_workspace_action(
                                ActionRequest::CloseWorktree { project_id, merge, stash, fetch, push, delete_branch },
                                &mut ws,
                                &mut focus_manager,
                                &backend,
                                &terminals,
                                &app_settings,
                                &workspace_tick,
                                &hook_runner,
                                &hook_monitor,
                            )
                        }
                        Some(Err(e)) => CommandResult::Err(e),
                        Some(Ok(plan)) => {
                            // Off-reactor `git worktree remove` (force: the user
                            // confirmed the close). NO lock held during the git.
                            let git = {
                                let worktree_path = plan.worktree_path.clone();
                                runtime
                                    .spawn_blocking(move || okena_git::remove_worktree(&worktree_path, true))
                                    .await
                            };
                            match git {
                                Err(join) => {
                                    CommandResult::Err(format!("worktree removal task failed: {join}"))
                                }
                                Ok(Err(e)) => CommandResult::Err(e.to_string()),
                                Ok(Ok(())) => {
                                    let mut cx = DaemonWorkspaceCx::new(&workspace_tick, &hook_runner, &hook_monitor);
                                    let mut ws = workspace.lock();
                                    ws.finish_worktree_removal(&mut focus_manager, &plan, &global_hooks, &mut cx);
                                    CommandResult::Ok(None)
                                }
                            }
                        }
                    }
                }

                // ── Default: workspace-scoped action ─────────────────────────
                action => {
                    let git_poll_trigger = git_poll_trigger_for_action(&action);
                    // Resolve the action's explicit target window (if any)
                    // BEFORE moving `action` into `execute_action`. The daemon
                    // serves only the synthetic main window: `None` and
                    // `Some("main")` are accepted; any other (valid) window id is
                    // "not found"; a malformed id is "invalid".
                    let parsed_target = match action.target_window() {
                        None => Ok(None),
                        Some(s) => match parse_window_id(s) {
                            Some(wid) => Ok(Some(wid)),
                            None => Err(s.to_string()),
                        },
                    };
                    match parsed_target {
                        Err(bad) => {
                            // Malformed window id: rejected up front.
                            CommandResult::Err(format!("invalid window id: {bad}"))
                        }
                        Ok(None) | Ok(Some(WindowId::Main)) => {
                            // Snapshot app settings to thread into the gpui-free
                            // `execute_action` (hooks / worktree template /
                            // default shell). Read before locking the workspace.
                            // The daemon always targets `WindowId::Main`; the
                            // mutators notify via `cx` themselves, so there is no
                            // separate `cx.notify()` like the GUI's view-refresh.
                            let app_settings = settings.lock().clone();
                            let mut ws = workspace.lock();
                            let result = run_main_workspace_action(
                                action,
                                &mut ws,
                                &mut focus_manager,
                                &backend,
                                &terminals,
                                &app_settings,
                                &workspace_tick,
                                &hook_runner,
                                &hook_monitor,
                            );
                            send_git_poll_trigger_after_success(
                                &result,
                                git_poll_trigger,
                                &git_poll_trigger_tx,
                            );
                            result
                        }
                        Ok(Some(WindowId::Extra(uuid))) => {
                            // The daemon has only the synthetic main window.
                            CommandResult::Err(format!("window not found: {uuid}"))
                        }
                    }
                }
            },

            RemoteCommand::ResizeFromConnection {
                terminal_id,
                cols,
                rows,
                connection_id,
            } => {
                if !okena_terminal::terminal::claim_remote_resize_if_allowed(
                    &terminal_id,
                    &connection_id,
                ) {
                    // Denied: reply with the authoritative size so the stream
                    // handler can correct the client's optimistically-resized
                    // grid and make it cede (server_owns), instead of leaving
                    // it silently diverged from the PTY.
                    let denied_size = terminals
                        .lock()
                        .get(&terminal_id)
                        .map(|term| term.resize_state.lock().size);
                    match denied_size {
                        Some(size) => CommandResult::Ok(Some(serde_json::json!({
                            "denied": true,
                            "cols": size.cols,
                            "rows": size.rows,
                        }))),
                        None => CommandResult::Ok(None),
                    }
                } else {
                    let app_settings = settings.lock().clone();
                    let mut ws = workspace.lock();
                    run_main_workspace_action(
                        ActionRequest::Resize {
                            terminal_id,
                            cols,
                            rows,
                        },
                        &mut ws,
                        &mut focus_manager,
                        &backend,
                        &terminals,
                        &app_settings,
                        &workspace_tick,
                        &hook_runner,
                        &hook_monitor,
                    )
                }
            }
            RemoteCommand::ActionFromConnection { .. } => {
                CommandResult::Err("internal action normalization error".to_string())
            }

            // ── GetState: full workspace snapshot ────────────────────────────
            RemoteCommand::GetState => {
                // Lock order: workspace first, then service manager (kept
                // consistent across the loop). The whole arm is synchronous, so
                // both guards drop before the next `recv().await`.
                let ws = workspace.lock();
                let sm = service_manager.lock();
                let sv = *state_version.borrow();
                let git_statuses = git_status_tx.borrow().clone();
                let data = ws.data();

                // Build terminal size map from the registry.
                let size_map: HashMap<String, (u16, u16)> = {
                    let registry = terminals.lock();
                    registry
                        .iter()
                        .map(|(id, term)| {
                            let size = term.resize_state.lock().size;
                            (id.clone(), (size.cols, size.rows))
                        })
                        .collect()
                };

                // Source of truth for runtime visibility (per-window viewport).
                let hidden_project_ids = &data.main_window.hidden_project_ids;

                // Pre-build the per-project wire service lists from THIS caller's
                // `ServiceManager` (keeps the `okena-services` dependency in the
                // daemon; the shared builder in `okena-app-core` never sees it).
                // The `ServiceInstance -> ApiServiceInfo` mapping is
                // `ServiceInstance::to_api`, shared with the GUI loop.
                let services_by_project: HashMap<String, Vec<ApiServiceInfo>> = data
                    .projects
                    .iter()
                    .map(|p| {
                        let services = sm
                            .services_for_project(&p.id)
                            .into_iter()
                            .map(|inst| inst.to_api())
                            .collect();
                        (p.id.clone(), services)
                    })
                    .collect();

                // The daemon serves a SINGLE synthetic main window (ported from
                // the former GUI-headless windows resolver). No GUI, so it's always
                // "active", has no per-window focus/fullscreen/bounds, and no
                // hidden set — every project in `project_order` is visible.
                let visible_project_ids: Vec<String> = ws
                    .visible_projects(WindowId::Main, None, false)
                    .iter()
                    .map(|p| p.id.clone())
                    .collect();
                let windows = vec![ApiWindow {
                    id: "main".into(),
                    kind: "main".into(),
                    active: true,
                    focused_project_id: None,
                    focused_terminal_id: None,
                    fullscreen: None,
                    visible_project_ids,
                    folder_filter: None,
                    bounds: None,
                    sidebar_open: None,
                }];

                // Hook execution history so thin clients can render the hook
                // log (the hooks run here on the daemon, not on the client).
                let hooks = hook_monitor
                    .as_ref()
                    .map(|m| m.history().iter().map(|e| e.to_api()).collect())
                    .unwrap_or_default();

                // Shared projection: ordered projects + folders + flat back-compat
                // fields → `StateResponse` (identical to the GUI loop).
                let resp = build_state_response(
                    sv,
                    data,
                    &git_statuses,
                    &services_by_project,
                    hidden_project_ids,
                    &size_map,
                    windows,
                    hooks,
                );

                // `match` (not `.expect`) so the daemon-core crate stays clean
                // under `clippy::expect_used` had it been enabled — the serialize
                // is unreachable-fail for a well-formed DTO.
                match serde_json::to_value(resp) {
                    Ok(v) => CommandResult::Ok(Some(v)),
                    Err(e) => CommandResult::Err(format!("failed to serialize state: {e}")),
                }
            }

            // ── GetTerminalSizes ─────────────────────────────────────────────
            RemoteCommand::GetTerminalSizes { terminal_ids } => {
                let terms = terminals.lock();
                let mut sizes: HashMap<String, (u16, u16)> = HashMap::new();
                for id in &terminal_ids {
                    if let Some(term) = terms.get(id) {
                        let s = term.resize_state.lock().size;
                        sizes.insert(id.clone(), (s.cols, s.rows));
                    }
                }
                match serde_json::to_value(sizes) {
                    Ok(v) => CommandResult::Ok(Some(v)),
                    Err(e) => CommandResult::Err(format!("failed to serialize sizes: {e}")),
                }
            }

            // ── RenderSnapshot ───────────────────────────────────────────────
            RemoteCommand::RenderSnapshot { terminal_id } => {
                let ws = workspace.lock();
                match ensure_terminal(&terminal_id, &terminals, &*backend, &ws) {
                    Some(term) => {
                        let (data, sequence) = term.render_snapshot_with_sequence();
                        CommandResult::OkSnapshot { data, sequence }
                    }
                    None => CommandResult::Err(format!("terminal not found: {terminal_id}")),
                }
            }

            // ── PastePath ────────────────────────────────────────────────────
            RemoteCommand::PastePath { terminal_id, text } => {
                let ws = workspace.lock();
                match ensure_terminal(&terminal_id, &terminals, &*backend, &ws) {
                    Some(term) => {
                        term.send_paste(&text);
                        CommandResult::Ok(None)
                    }
                    None => CommandResult::Err(format!("terminal not found: {terminal_id}")),
                }
            }
        };

        if let Some(reply) = msg.reply {
            let _ = reply.send(result);
        }
    }
}

/// Materialize the PTYs for every restored project's uninitialized terminal
/// slots at daemon startup, then fire each restored project's `on_project_open`
/// lifecycle hook.
///
/// Persisted `workspace.json` layouts carry terminal slots with
/// `terminal_id: None` (the normal saved state). In daemon-client mode nobody
/// ever materializes them: the GUI client cannot self-spawn over a remote
/// backend, and the daemon only calls
/// [`spawn_uninitialized_terminals`](okena_app_core::workspace::actions::execute::spawn_uninitialized_terminals)
/// from the `CreateTerminal` / `SplitTerminal` / `AddProject` action arms — not
/// on boot. A restored slot therefore never gets a PTY and renders blank
/// forever.
///
/// This is the daemon's boot-time analogue of the GUI's
/// `spawn_terminals_for_project` (fired on project creation): it walks EVERY
/// loaded project and assigns ids + creates PTYs for its uninitialized slots,
/// so `/v1/state` serves real ids and the snapshot/live-PTY path works.
///
/// All projects (not just the visible ones): the prior in-process GUI eagerly
/// spawned terminals when a project column was created, regardless of overview
/// visibility, and `hidden_project_ids` is a per-window viewport concern, not a
/// "don't run this project" signal. Spawning everything keeps behavior simple
/// and correct; project counts are small (one column per project), so this is
/// not too heavy.
///
/// Runs on the LocalSet thread (mirroring the command loop's `execute_action`):
/// PTY spawning and hook execution may reach the reactor, and the
/// `WorkspaceCx::notify` bumps the `workspace_tick` whose observer task bumps
/// `state_version`. The freshly-assigned ids bump `data_version`, so the
/// existing autosave observer persists them — this introduces NO second writer.
///
/// Must be invoked AFTER `spawn_observers` (so the tick reaches them) and BEFORE
/// the command loop starts serving clients (so `/v1/state` never exposes the
/// transient null slots).
pub fn materialize_uninitialized_terminals(
    backend: &dyn TerminalBackend,
    workspace: &Arc<Mutex<Workspace>>,
    workspace_tick: &watch::Sender<u64>,
    hook_runner: &Option<okena_hooks::HookRunner>,
    hook_monitor: &Option<okena_hooks::HookMonitor>,
    terminals: &TerminalsRegistry,
    settings: &Arc<Mutex<AppSettings>>,
) {
    // Snapshot the project ids under a short lock, then drop it before spawning
    // (each `spawn_uninitialized_terminals` call re-locks the workspace itself).
    let project_ids: Vec<String> = {
        let ws = workspace.lock();
        ws.data().projects.iter().map(|p| p.id.clone()).collect()
    };

    // Snapshot settings once, mirroring the command loop's `execute_action` arm.
    let app_settings = settings.lock().clone();

    for project_id in &project_ids {
        let mut cx = DaemonWorkspaceCx::new(workspace_tick, hook_runner, hook_monitor);
        let mut ws = workspace.lock();
        match spawn_uninitialized_terminals(
            &mut ws,
            project_id,
            backend,
            terminals,
            &app_settings,
            None,
            &mut cx,
        ) {
            okena_app_core::workspace::actions::execute::ActionResult::Err(e) => {
                log::error!(
                    "startup: failed to materialize terminals for project {project_id}: {e}"
                );
            }
            okena_app_core::workspace::actions::execute::ActionResult::Ok(_) => {}
        }
    }

    // Fire `on_project_open` for every restored project. `add_project` fires it
    // for NEW projects, but the daemon restores existing projects via
    // `Workspace::new` (never `add_project`), so without this their lifecycle
    // open hook — global or per-project — never runs on restart. Runs AFTER
    // terminal materialization so the hook sees a settled registry; the hook's
    // own PTY (if any) is registered via `register_hook_results`.
    for project_id in &project_ids {
        let mut cx = DaemonWorkspaceCx::new(workspace_tick, hook_runner, hook_monitor);
        let mut ws = workspace.lock();
        ws.fire_project_open_hooks(project_id, &app_settings.hooks, &mut cx);
    }
}

/// Run a workspace-scoped action against the synthetic main window.
///
/// Shared by the generic default arm and the `CloseTerminal` immediate-close
/// fallbacks. The caller snapshots `app_settings` and locks the workspace
/// (passed as `&mut Workspace`) BEFORE invoking this, so no lock is held across
/// an `.await`. Mirrors the inline body the default arm uses for
/// `WindowId::Main`.
#[allow(clippy::too_many_arguments)]
fn run_main_workspace_action(
    action: ActionRequest,
    ws: &mut Workspace,
    focus_manager: &mut FocusManager,
    backend: &Arc<dyn TerminalBackend>,
    terminals: &TerminalsRegistry,
    app_settings: &AppSettings,
    workspace_tick: &watch::Sender<u64>,
    hook_runner: &Option<okena_hooks::HookRunner>,
    hook_monitor: &Option<okena_hooks::HookMonitor>,
) -> CommandResult {
    let mut cx = DaemonWorkspaceCx::new(workspace_tick, hook_runner, hook_monitor);
    let result = execute_action(
        action,
        ws,
        WindowId::Main,
        focus_manager,
        &**backend,
        terminals,
        app_settings,
        &mut cx,
    )
    .into_command_result();

    // Drain any terminal kills the action queued (delete_project,
    // remove_worktree_project, the grace==0 immediate close, …) and tear down
    // their PTYs + persistent session backends. The GUI client does this via a
    // `Workspace` observer; the daemon has no equivalent observer, so without
    // this a delete removes the project's state but leaks its PTY / dtach / tmux
    // processes. Mirrors the soft-close finalize paths (`CloseTerminalNow`, the
    // grace-expiry poll), which drain + kill the same way.
    for id in ws.drain_pending_terminal_kills() {
        backend.kill(&id);
        terminals.lock().remove(&id);
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{StubBackend, StubTransport, default_settings, empty_workspace_data};
    use okena_core::api::StateResponse;
    use std::collections::HashSet;

    use okena_remote_server::bridge::{BridgeReceiver, BridgeSender, bridge_channel};
    use okena_state::WorkspaceData;
    use okena_terminal::backend::TerminalBackend;
    use okena_terminal::shell_config::ShellType;
    use okena_terminal::terminal::TerminalTransport;
    use tokio::sync::oneshot;

    /// Bundle of the shared state + channels the loop needs, so each test can
    /// spawn the loop and keep handles to inspect afterwards.
    struct Harness {
        workspace: Arc<Mutex<Workspace>>,
        backend: Arc<dyn TerminalBackend>,
        workspace_tick: watch::Sender<u64>,
        terminals: TerminalsRegistry,
        state_version: Arc<watch::Sender<u64>>,
        git_status_tx: Arc<watch::Sender<HashMap<String, ApiGitStatus>>>,
        service_manager: Arc<Mutex<ServiceManager>>,
        service_tick: watch::Sender<u64>,
        settings: Arc<Mutex<AppSettings>>,
        daemon_config: DaemonConfig,
    }

    impl Harness {
        fn spawn_loop(self, bridge_rx: BridgeReceiver) -> tokio::task::JoinHandle<()> {
            tokio::task::spawn_local(daemon_command_loop(
                bridge_rx,
                self.backend,
                self.workspace,
                self.workspace_tick,
                None,
                None,
                self.terminals,
                self.state_version,
                self.git_status_tx,
                self.service_manager,
                self.service_tick,
                tokio::runtime::Handle::current(),
                self.settings,
                self.daemon_config,
                Arc::new(Mutex::new(HashMap::new())),
                tokio::sync::mpsc::unbounded_channel().0,
            ))
        }
    }

    fn harness() -> Harness {
        let workspace = Arc::new(Mutex::new(Workspace::new(empty_workspace_data())));
        let terminals: TerminalsRegistry = Arc::new(Mutex::new(Default::default()));
        let backend: Arc<dyn TerminalBackend> = Arc::new(StubBackend);
        let service_manager = Arc::new(Mutex::new(ServiceManager::new(
            backend.clone(),
            terminals.clone(),
        )));
        let settings = Arc::new(Mutex::new(default_settings()));
        let daemon_config = DaemonConfig::new(settings.clone());
        let (workspace_tick, _wtrx) = watch::channel(0u64);
        let (service_tick, _strx) = watch::channel(0u64);
        let (state_version, _svrx) = watch::channel(0u64);
        let (git_status_tx, _gsrx) = watch::channel(HashMap::new());
        Harness {
            workspace,
            backend,
            workspace_tick,
            terminals,
            state_version: Arc::new(state_version),
            git_status_tx: Arc::new(git_status_tx),
            service_manager,
            service_tick,
            settings,
            daemon_config,
        }
    }

    async fn request(
        bridge_tx: &BridgeSender,
        command: RemoteCommand,
        label: &str,
    ) -> CommandResult {
        let (reply_tx, reply_rx) = oneshot::channel();
        bridge_tx
            .send(BridgeMessage {
                command,
                reply: Some(reply_tx),
            })
            .await
            .unwrap_or_else(|_| panic!("send {label}"));
        reply_rx.await.unwrap_or_else(|_| panic!("{label} reply"))
    }

    // ── Pure unit tests ──────────────────────────────────────────────────────

    #[test]
    fn parse_window_id_main_maps_to_main() {
        assert_eq!(parse_window_id("main"), Some(WindowId::Main));
    }

    #[test]
    fn parse_window_id_valid_uuid_maps_to_extra() {
        let id = uuid::Uuid::new_v4();
        assert_eq!(parse_window_id(&id.to_string()), Some(WindowId::Extra(id)));
    }

    #[test]
    fn parse_window_id_garbage_returns_none() {
        assert_eq!(parse_window_id("garbage"), None);
        assert_eq!(parse_window_id(""), None);
        // A near-miss UUID (one char short) is still rejected.
        assert_eq!(parse_window_id("550e8400-e29b-41d4-a716-44665544000"), None);
    }

    #[test]
    fn api_project_visibility_reads_from_hidden_set() {
        use okena_app_core::remote_snapshot::api_project_visibility;
        let hidden: HashSet<String> = ["p1".to_string()].into_iter().collect();
        assert!(!api_project_visibility("p1", &hidden));
        assert!(api_project_visibility("p2", &hidden));
    }

    #[test]
    fn api_project_visibility_empty_hidden_set_is_visible() {
        use okena_app_core::remote_snapshot::api_project_visibility;
        let hidden: HashSet<String> = HashSet::new();
        assert!(api_project_visibility("p1", &hidden));
    }

    #[test]
    fn git_poll_trigger_is_sent_only_after_success() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

        send_git_poll_trigger_after_success(
            &CommandResult::Err("nope".to_string()),
            Some(GitPollTrigger::branch_change("p1".to_string())),
            &tx,
        );
        assert!(rx.try_recv().is_err());

        send_git_poll_trigger_after_success(
            &CommandResult::Ok(None),
            Some(GitPollTrigger::branch_change("p1".to_string())),
            &tx,
        );
        let trigger = rx.try_recv().expect("success sends trigger");
        assert_eq!(trigger.project_id.as_deref(), Some("p1"));
        assert!(trigger.poll_github);
        assert!(trigger.invalidate_github);
    }

    // ── Loop round-trip tests ─────────────────────────────────────────────────

    /// `GetState` returns `Ok(Some(v))` that deserializes into a `StateResponse`
    /// with the single synthetic `"main"` window.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn get_state_round_trip() {
        let h = harness();
        let (bridge_tx, bridge_rx) = bridge_channel();

        let local = tokio::task::LocalSet::new();
        local
            .run_until(async move {
                let handle = h.spawn_loop(bridge_rx);
                let result = request(&bridge_tx, RemoteCommand::GetState, "GetState").await;
                let value = match result {
                    CommandResult::Ok(Some(v)) => v,
                    other => panic!("expected Ok(Some), got {other:?}"),
                };
                let resp: StateResponse =
                    serde_json::from_value(value).expect("deserialize StateResponse");
                assert_eq!(resp.windows.len(), 1, "single synthetic window");
                assert_eq!(resp.windows[0].id, "main");
                assert_eq!(resp.windows[0].kind, "main");
                assert!(resp.windows[0].active);

                // Drop the sender so `recv` errors and the loop task joins.
                drop(bridge_tx);
                handle.await.expect("loop task joins");
            })
            .await;
    }

    /// App-scoped `GetSettingsSchema` returns `Ok(Some(_))` with settings keys.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn get_settings_schema_round_trip() {
        let h = harness();
        let (bridge_tx, bridge_rx) = bridge_channel();

        let local = tokio::task::LocalSet::new();
        local
            .run_until(async move {
                let handle = h.spawn_loop(bridge_rx);
                let result = request(
                    &bridge_tx,
                    RemoteCommand::Action(ActionRequest::GetSettingsSchema),
                    "GetSettingsSchema",
                )
                .await;
                match result {
                    CommandResult::Ok(Some(v)) => {
                        let obj = v.as_object().expect("schema is an object");
                        assert!(obj.contains_key("font_size"));
                        assert!(obj.contains_key("theme_mode"));
                    }
                    other => panic!("expected Ok(Some), got {other:?}"),
                }

                drop(bridge_tx);
                handle.await.expect("loop task joins");
            })
            .await;
    }

    /// A workspace-scoped action (`CreateFolder`) returns `Ok(_)` and mutates the
    /// shared workspace.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn create_folder_action_mutates_workspace() {
        let h = harness();
        let workspace_for_assert = h.workspace.clone();
        let (bridge_tx, bridge_rx) = bridge_channel();

        let local = tokio::task::LocalSet::new();
        local
            .run_until(async move {
                let handle = h.spawn_loop(bridge_rx);
                let result = request(
                    &bridge_tx,
                    RemoteCommand::Action(ActionRequest::CreateFolder { name: "f".into() }),
                    "CreateFolder",
                )
                .await;
                assert!(
                    matches!(result, CommandResult::Ok(_)),
                    "expected Ok, got {result:?}"
                );

                // The shared workspace now has the folder.
                {
                    let ws = workspace_for_assert.lock();
                    assert_eq!(ws.data().folders.len(), 1, "folder was created");
                    assert_eq!(ws.data().folders[0].name, "f");
                }

                drop(bridge_tx);
                handle.await.expect("loop task joins");
            })
            .await;
    }

    // ── Startup terminal materialization ──────────────────────────────────────

    /// A `WorkspaceData` carrying one project whose layout is a single
    /// uninitialized terminal slot (`terminal_id: None`) — the normal persisted
    /// state for a restored project. `path` is the project cwd the PTY spawns in.
    fn workspace_with_uninitialized_terminal(path: &str) -> WorkspaceData {
        use okena_state::{LayoutNode, ProjectData};
        let project = ProjectData {
            id: "p1".to_string(),
            name: "Project p1".to_string(),
            path: path.to_string(),
            layout: Some(LayoutNode::Terminal {
                terminal_id: None,
                minimized: false,
                detached: false,
                shell_type: ShellType::Default,
                zoom_level: 1.0,
            }),
            terminal_names: Default::default(),
            hidden_terminals: Default::default(),
            worktree_info: None,
            worktree_ids: Vec::new(),
            folder_color: Default::default(),
            hooks: Default::default(),
            is_remote: false,
            connection_id: None,
            service_terminals: Default::default(),
            default_shell: None,
            hook_terminals: Default::default(),
            pinned: false,
            last_activity_at: None,
        };
        WorkspaceData {
            version: 1,
            projects: vec![project],
            project_order: vec!["p1".to_string()],
            folders: Vec::new(),
            service_panel_heights: Default::default(),
            hook_panel_heights: Default::default(),
            main_window: Default::default(),
            extra_windows: Vec::new(),
        }
    }

    /// `materialize_uninitialized_terminals` assigns a real `terminal_id` to a
    /// restored `terminal_id: None` slot, creates the backing PTY (so it lands
    /// in the registry), bumps `data_version` (so the autosave observer persists
    /// the assigned id) and the `workspace_tick` (so the state-version observer
    /// fires). This is the boot fix for blank restored terminals in
    /// daemon-client mode.
    #[test]
    fn materialize_assigns_ids_and_spawns_ptys_for_restored_projects() {
        use okena_terminal::backend::LocalBackend;
        use okena_terminal::pty_manager::PtyManager;
        use okena_terminal::session_backend::SessionBackend;
        use okena_workspace::state::LayoutNode;

        // A real, existing cwd for the spawned shell.
        let tmp = std::env::temp_dir();
        let tmp_path = tmp.to_str().expect("temp dir is utf-8");

        let (pty_manager, _pty_events) = PtyManager::new(SessionBackend::None);
        let pty_manager = Arc::new(pty_manager);
        let backend: Arc<dyn TerminalBackend> = Arc::new(LocalBackend::new(pty_manager.clone()));
        let terminals: TerminalsRegistry = Arc::new(Mutex::new(Default::default()));

        let workspace = Arc::new(Mutex::new(Workspace::new(
            workspace_with_uninitialized_terminal(tmp_path),
        )));
        let settings = Arc::new(Mutex::new(default_settings()));
        let (workspace_tick, _wtrx) = watch::channel(0u64);

        // Preconditions: slot is uninitialized, registry empty, tick at 0.
        let version_before = workspace.lock().data_version();
        let tick_before = *workspace_tick.borrow();
        assert!(terminals.lock().is_empty(), "registry starts empty");

        materialize_uninitialized_terminals(
            &*backend,
            &workspace,
            &workspace_tick,
            &None,
            &None,
            &terminals,
            &settings,
        );

        // The slot now has an id, the PTY is in the registry, and both the
        // persistent data_version and the notify tick advanced.
        let ws = workspace.lock();
        let project = ws.project("p1").expect("project p1 exists");
        let assigned = match project.layout.as_ref().expect("layout present") {
            LayoutNode::Terminal { terminal_id, .. } => terminal_id.clone(),
            other => panic!("expected a Terminal layout node, got {other:?}"),
        };
        let assigned = assigned.expect("terminal slot got a real id");
        assert!(
            terminals.lock().contains_key(&assigned),
            "spawned PTY is registered under the assigned id"
        );
        assert!(
            ws.data_version() > version_before,
            "data_version advanced so autosave persists the assigned id"
        );
        assert!(
            *workspace_tick.borrow() > tick_before,
            "workspace_tick advanced so the state-version observer fires"
        );
    }

    /// On an empty workspace `materialize_uninitialized_terminals` is a no-op:
    /// no terminals spawned and the data_version is untouched.
    #[test]
    fn materialize_is_noop_for_empty_workspace() {
        let workspace = Arc::new(Mutex::new(Workspace::new(empty_workspace_data())));
        let backend: Arc<dyn TerminalBackend> = Arc::new(StubBackend);
        let terminals: TerminalsRegistry = Arc::new(Mutex::new(Default::default()));
        let settings = Arc::new(Mutex::new(default_settings()));
        let (workspace_tick, _wtrx) = watch::channel(0u64);

        let version_before = workspace.lock().data_version();
        materialize_uninitialized_terminals(
            &*backend,
            &workspace,
            &workspace_tick,
            &None,
            &None,
            &terminals,
            &settings,
        );

        assert!(terminals.lock().is_empty(), "no terminals spawned");
        assert_eq!(
            workspace.lock().data_version(),
            version_before,
            "data_version untouched on empty workspace"
        );
    }

    // ── PROOF: does a lifecycle hook actually execute in the daemon path? ─────
    //
    // These two tests are a matched pair driving the SAME real HookRunner +
    // HookMonitor + real LocalBackend/PtyManager the daemon builds at boot
    // (daemon.rs:199/211). The only variable is the entrypoint.
    //
    //  * `restore_boot_path_does_not_fire_on_project_open` drives the actual
    //    daemon-boot entrypoint (`materialize_uninitialized_terminals`, called
    //    from `daemon.run()` at command_loop.rs:663) against a RESTORED project
    //    that has `project.on_open` configured. Result: the monitor records
    //    ZERO executions -> the on_project_open hook never fires on restore.
    //
    //  * `add_project_fires_on_project_open` drives `ws.add_project` (the sole
    //    fire_on_project_open call site) with the SAME services. Result: the
    //    monitor records exactly one `on_project_open` execution -> the firing
    //    machinery works; only the restore entrypoint skips it.

    /// Build a restored project that BOTH has an uninitialized terminal slot
    /// AND a configured `project.on_open` hook, at a real cwd.
    fn workspace_restored_with_on_open(path: &str, on_open: &str) -> WorkspaceData {
        use okena_state::{HooksConfig, LayoutNode, ProjectData, ProjectHooks};
        let project = ProjectData {
            id: "p1".to_string(),
            name: "Project p1".to_string(),
            path: path.to_string(),
            layout: Some(LayoutNode::Terminal {
                terminal_id: None,
                minimized: false,
                detached: false,
                shell_type: ShellType::Default,
                zoom_level: 1.0,
            }),
            terminal_names: Default::default(),
            hidden_terminals: Default::default(),
            worktree_info: None,
            worktree_ids: Vec::new(),
            folder_color: Default::default(),
            hooks: HooksConfig {
                project: ProjectHooks {
                    on_open: Some(on_open.to_string()),
                    on_close: None,
                },
                ..Default::default()
            },
            is_remote: false,
            connection_id: None,
            service_terminals: Default::default(),
            default_shell: None,
            hook_terminals: Default::default(),
            pinned: false,
            last_activity_at: None,
        };
        WorkspaceData {
            version: 1,
            projects: vec![project],
            project_order: vec!["p1".to_string()],
            folders: Vec::new(),
            service_panel_heights: Default::default(),
            hook_panel_heights: Default::default(),
            main_window: Default::default(),
            extra_windows: Vec::new(),
        }
    }

    /// FIXED behavior: the daemon boot path materializes the restored project's
    /// terminals AND fires its configured `on_project_open` hook. Uses a real
    /// backend + real HookRunner/HookMonitor so the pass reflects genuine
    /// execution, not a stub. (This is the same project the pre-fix proof used
    /// to demonstrate the break — the assertion is flipped.)
    #[test]
    fn restore_boot_path_fires_on_project_open() {
        use okena_hooks::{HookMonitor, HookRunner};
        use okena_terminal::backend::LocalBackend;
        use okena_terminal::pty_manager::PtyManager;
        use okena_terminal::session_backend::SessionBackend;
        use okena_workspace::state::LayoutNode;

        let tmp = std::env::temp_dir();
        let tmp_path = tmp.to_str().expect("temp dir is utf-8");

        let (pty_manager, _pty_events) = PtyManager::new(SessionBackend::None);
        let pty_manager = Arc::new(pty_manager);
        let backend: Arc<dyn TerminalBackend> = Arc::new(LocalBackend::new(pty_manager.clone()));
        let terminals: TerminalsRegistry = Arc::new(Mutex::new(Default::default()));

        // The real services the daemon threads through (daemon.rs:211).
        let hook_runner = Some(HookRunner::new(backend.clone(), terminals.clone()));
        let hook_monitor = Some(HookMonitor::new());

        // Restored project carries a PER-PROJECT on_open (global settings empty),
        // proving the fire resolves per-project hooks reloaded from workspace.json.
        let workspace = Arc::new(Mutex::new(Workspace::new(
            workspace_restored_with_on_open(tmp_path, "echo HOOK_MARKER"),
        )));
        let settings = Arc::new(Mutex::new(default_settings()));
        let (workspace_tick, _wtrx) = watch::channel(0u64);

        materialize_uninitialized_terminals(
            &*backend,
            &workspace,
            &workspace_tick,
            &hook_runner,
            &hook_monitor,
            &terminals,
            &settings,
        );

        // The restored terminal slot was materialized (boot path ran)...
        let assigned = {
            let ws = workspace.lock();
            match ws.project("p1").expect("p1").layout.as_ref().expect("layout") {
                LayoutNode::Terminal { terminal_id, .. } => terminal_id.clone(),
                other => panic!("expected Terminal node, got {other:?}"),
            }
        };
        let assigned = assigned.expect("restored terminal slot got a real id");
        assert!(
            terminals.lock().contains_key(&assigned),
            "boot path spawned the layout terminal PTY"
        );

        // ...and the restored project's per-project on_project_open hook fired.
        let history = hook_monitor.as_ref().unwrap().history();
        assert_eq!(
            history.len(),
            1,
            "restore must fire on_project_open exactly once, got: {:?}",
            history.iter().map(|h| h.hook_type).collect::<Vec<_>>()
        );
        assert_eq!(history[0].hook_type, "on_project_open");

        // The fire registered a live hook terminal in the project's map.
        assert_eq!(
            workspace.lock().project("p1").expect("p1").hook_terminals.len(),
            1,
            "one live hook terminal registered after boot fire"
        );
    }

    /// Restored projects that carry NO `on_open` hook (and no global hook) must
    /// NOT fire anything at boot — no spurious hook executions.
    #[test]
    fn restore_boot_path_no_hook_does_not_fire() {
        use okena_hooks::{HookMonitor, HookRunner};
        use okena_terminal::backend::LocalBackend;
        use okena_terminal::pty_manager::PtyManager;
        use okena_terminal::session_backend::SessionBackend;

        let tmp = std::env::temp_dir();
        let tmp_path = tmp.to_str().expect("temp dir is utf-8");

        let (pty_manager, _pty_events) = PtyManager::new(SessionBackend::None);
        let pty_manager = Arc::new(pty_manager);
        let backend: Arc<dyn TerminalBackend> = Arc::new(LocalBackend::new(pty_manager.clone()));
        let terminals: TerminalsRegistry = Arc::new(Mutex::new(Default::default()));
        let hook_runner = Some(HookRunner::new(backend.clone(), terminals.clone()));
        let hook_monitor = Some(HookMonitor::new());

        // Restored project with an uninitialized terminal slot but EMPTY hooks.
        let workspace = Arc::new(Mutex::new(Workspace::new(
            workspace_with_uninitialized_terminal(tmp_path),
        )));
        let settings = Arc::new(Mutex::new(default_settings())); // global hooks empty
        let (workspace_tick, _wtrx) = watch::channel(0u64);

        materialize_uninitialized_terminals(
            &*backend,
            &workspace,
            &workspace_tick,
            &hook_runner,
            &hook_monitor,
            &terminals,
            &settings,
        );

        assert!(
            hook_monitor.as_ref().unwrap().history().is_empty(),
            "no hook configured → nothing fires on restore"
        );
        assert!(
            workspace.lock().project("p1").expect("p1").hook_terminals.is_empty(),
            "no hook terminals registered when no hook is configured"
        );
    }

    /// Stale `hook_terminals` restored from disk (dead PTYs from a prior session)
    /// are dropped on boot before the fresh fire registers a live entry — so the
    /// map does not accumulate phantoms across restarts.
    #[test]
    fn restore_boot_path_clears_stale_hook_terminals() {
        use okena_hooks::{HookMonitor, HookRunner};
        use okena_terminal::backend::LocalBackend;
        use okena_terminal::pty_manager::PtyManager;
        use okena_terminal::session_backend::SessionBackend;
        use okena_workspace::state::{HookTerminalEntry, HookTerminalStatus};

        let tmp = std::env::temp_dir();
        let tmp_path = tmp.to_str().expect("temp dir is utf-8");

        let (pty_manager, _pty_events) = PtyManager::new(SessionBackend::None);
        let pty_manager = Arc::new(pty_manager);
        let backend: Arc<dyn TerminalBackend> = Arc::new(LocalBackend::new(pty_manager.clone()));
        let terminals: TerminalsRegistry = Arc::new(Mutex::new(Default::default()));
        let hook_runner = Some(HookRunner::new(backend.clone(), terminals.clone()));
        let hook_monitor = Some(HookMonitor::new());

        // Restored project has BOTH a persisted (dead) hook terminal AND an
        // on_open hook to re-fire.
        let mut data = workspace_restored_with_on_open(tmp_path, "echo HOOK_MARKER");
        data.projects[0].hook_terminals.insert(
            "stale-dead-id".to_string(),
            HookTerminalEntry {
                label: "on_project_open".to_string(),
                status: HookTerminalStatus::Succeeded,
                hook_type: "on_project_open".to_string(),
                command: "echo old".to_string(),
                cwd: tmp_path.to_string(),
            },
        );
        let workspace = Arc::new(Mutex::new(Workspace::new(data)));
        let settings = Arc::new(Mutex::new(default_settings()));
        let (workspace_tick, _wtrx) = watch::channel(0u64);

        materialize_uninitialized_terminals(
            &*backend,
            &workspace,
            &workspace_tick,
            &hook_runner,
            &hook_monitor,
            &terminals,
            &settings,
        );

        let ws = workspace.lock();
        let hooks = &ws.project("p1").expect("p1").hook_terminals;
        assert!(
            !hooks.contains_key("stale-dead-id"),
            "stale persisted hook terminal must be dropped on boot"
        );
        assert_eq!(hooks.len(), 1, "exactly one live hook terminal after re-fire");
    }

    /// CONTRAST (machinery works): calling `add_project` with the SAME real
    /// services DOES fire `on_project_open` and records exactly one execution.
    /// Proves the failure above is the missing restore trigger, not a broken
    /// runner or gpui-gated code path.
    #[test]
    fn add_project_fires_on_project_open() {
        use okena_hooks::{HookMonitor, HookRunner};
        use okena_terminal::backend::LocalBackend;
        use okena_terminal::pty_manager::PtyManager;
        use okena_terminal::session_backend::SessionBackend;

        let tmp = std::env::temp_dir();
        let tmp_path = tmp.to_str().expect("temp dir is utf-8").to_string();

        let (pty_manager, _pty_events) = PtyManager::new(SessionBackend::None);
        let pty_manager = Arc::new(pty_manager);
        let backend: Arc<dyn TerminalBackend> = Arc::new(LocalBackend::new(pty_manager.clone()));
        let terminals: TerminalsRegistry = Arc::new(Mutex::new(Default::default()));

        let hook_runner = Some(HookRunner::new(backend.clone(), terminals.clone()));
        let hook_monitor = Some(HookMonitor::new());

        // Global settings carry the on_open hook (add_project builds the new
        // ProjectData with empty per-project hooks and resolves against global).
        let mut app_settings = default_settings();
        app_settings.hooks.project.on_open = Some("echo HOOK_MARKER".to_string());

        let mut workspace = Workspace::new(empty_workspace_data());
        let (workspace_tick, _wtrx) = watch::channel(0u64);
        let mut cx = DaemonWorkspaceCx::new(&workspace_tick, &hook_runner, &hook_monitor);

        workspace.add_project(
            "Test".to_string(),
            tmp_path,
            true,
            &app_settings.hooks,
            WindowId::Main,
            &mut cx,
        );

        let history = hook_monitor.as_ref().unwrap().history();
        assert_eq!(
            history.len(),
            1,
            "add_project must fire exactly one hook, got: {:?}",
            history.iter().map(|h| h.hook_type).collect::<Vec<_>>()
        );
        assert_eq!(history[0].hook_type, "on_project_open");
    }

    /// `TerminalBackend` that records every `kill`ed id so a test can assert a
    /// deleted project's PTYs are torn down.
    struct RecordingBackend {
        killed: Arc<Mutex<Vec<String>>>,
    }

    impl TerminalBackend for RecordingBackend {
        fn transport(&self) -> Arc<dyn TerminalTransport> {
            Arc::new(StubTransport)
        }
        fn create_terminal(&self, _cwd: &str, _shell: Option<&ShellType>) -> anyhow::Result<String> {
            anyhow::bail!("recording backend: create_terminal not supported")
        }
        fn reconnect_terminal(
            &self,
            _terminal_id: &str,
            _cwd: &str,
            _shell: Option<&ShellType>,
        ) -> anyhow::Result<String> {
            anyhow::bail!("recording backend: reconnect_terminal not supported")
        }
        fn kill(&self, terminal_id: &str) {
            self.killed.lock().push(terminal_id.to_string());
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

    /// A single-terminal project whose terminal already has a real id.
    fn workspace_with_initialized_terminal(terminal_id: &str) -> WorkspaceData {
        use okena_state::{LayoutNode, ProjectData};
        let project = ProjectData {
            id: "p1".to_string(),
            name: "Project p1".to_string(),
            path: "/tmp".to_string(),
            layout: Some(LayoutNode::Terminal {
                terminal_id: Some(terminal_id.to_string()),
                minimized: false,
                detached: false,
                shell_type: ShellType::Default,
                zoom_level: 1.0,
            }),
            terminal_names: Default::default(),
            hidden_terminals: Default::default(),
            worktree_info: None,
            worktree_ids: Vec::new(),
            folder_color: Default::default(),
            hooks: Default::default(),
            is_remote: false,
            connection_id: None,
            service_terminals: Default::default(),
            default_shell: None,
            hook_terminals: Default::default(),
            pinned: false,
            last_activity_at: None,
        };
        WorkspaceData {
            version: 1,
            projects: vec![project],
            project_order: vec!["p1".to_string()],
            folders: Vec::new(),
            service_panel_heights: Default::default(),
            hook_panel_heights: Default::default(),
            main_window: Default::default(),
            extra_windows: Vec::new(),
        }
    }

    /// Deleting a project through the generic daemon action path must tear down
    /// its terminals' PTYs. `run_main_workspace_action` drains the kill queue
    /// `delete_project` fills — the GUI does this via a `Workspace` observer, but
    /// the daemon has none, so without the drain the PTY/session would leak.
    #[test]
    fn delete_project_drains_and_kills_queued_terminals() {
        let killed = Arc::new(Mutex::new(Vec::new()));
        let backend: Arc<dyn TerminalBackend> =
            Arc::new(RecordingBackend { killed: killed.clone() });
        let terminals: TerminalsRegistry = Arc::new(Mutex::new(Default::default()));
        let mut workspace = Workspace::new(workspace_with_initialized_terminal("t1"));
        let mut focus_manager = FocusManager::new();
        let settings = default_settings();
        let (workspace_tick, _wtrx) = watch::channel(0u64);

        let result = run_main_workspace_action(
            ActionRequest::DeleteProject {
                project_id: "p1".to_string(),
            },
            &mut workspace,
            &mut focus_manager,
            &backend,
            &terminals,
            &settings,
            &workspace_tick,
            &None,
            &None,
        );

        assert!(
            matches!(result, CommandResult::Ok(_)),
            "delete should succeed: {result:?}"
        );
        assert!(workspace.project("p1").is_none(), "project removed from state");
        assert_eq!(
            &*killed.lock(),
            &vec!["t1".to_string()],
            "the deleted project's terminal PTY was killed, not leaked"
        );
    }

    #[test]
    fn successful_config_changes_advance_state_version() {
        let (state_version, receiver) = watch::channel(7);
        publish_config_change_after_success(&CommandResult::Ok(None), &state_version);
        assert_eq!(*receiver.borrow(), 8);

        publish_config_change_after_success(
            &CommandResult::Err("invalid settings".to_string()),
            &state_version,
        );
        assert_eq!(*receiver.borrow(), 8);
    }

    // ── PROOF: does the DAEMON side of quick-create-worktree work? ────────────
    //
    // Drives the REAL CreateWorktree action end-to-end through `execute_action`
    // (the exact arm the daemon command loop dispatches at command_loop.rs:737)
    // against a REAL temp git repo, a REAL LocalBackend over
    // PtyManager(SessionBackend::None), and REAL HookRunner + HookMonitor (the
    // same services daemon.rs:199/211 builds). The parent project HAS a layout
    // with a terminal AND a `worktree.on_create` hook configured — mirroring an
    // actively-used project the user quick-creates a worktree from.
    //
    // Asserts BOTH reported symptoms are daemon-side-clean:
    //   (a) the new worktree project's layout carries a Terminal node with a
    //       real (Some) terminal_id whose PTY is in the TerminalsRegistry;
    //   (b) the on_worktree_create hook recorded exactly one execution in the
    //       HookMonitor AND a live hook terminal was registered on the project.
    #[test]
    fn create_worktree_materializes_terminal_and_fires_on_worktree_create() {
        use okena_hooks::{HookMonitor, HookRunner};
        use okena_state::{HooksConfig, LayoutNode, ProjectData, WorktreeHooks};
        use okena_terminal::backend::LocalBackend;
        use okena_terminal::pty_manager::PtyManager;
        use okena_terminal::session_backend::SessionBackend;
        use std::process::Command;

        // A real temp git repo with one commit — `git worktree add` needs a base.
        let repo = std::env::temp_dir().join(format!(
            "okena-wt-proof-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&repo).expect("mk repo dir");
        let git = |args: &[&str]| {
            let ok = Command::new("git")
                .args(args)
                .current_dir(&repo)
                .output()
                .expect("run git");
            assert!(ok.status.success(), "git {:?} failed: {}", args, String::from_utf8_lossy(&ok.stderr));
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["config", "user.email", "proof@okena.test"]);
        git(&["config", "user.name", "Proof"]);
        std::fs::write(repo.join("README.md"), "seed\n").expect("seed file");
        git(&["add", "."]);
        git(&["commit", "-qm", "seed"]);

        // A bare origin remote, like a real user project — the daemon bases new
        // worktree branches on origin/{default}, so origin/main must exist.
        let origin = repo.with_extension("origin.git");
        std::fs::create_dir_all(&origin).expect("mk origin dir");
        assert!(Command::new("git")
            .args(["init", "-q", "--bare", origin.to_str().unwrap()])
            .status()
            .expect("git init bare")
            .success());
        git(&["remote", "add", "origin", origin.to_str().unwrap()]);
        git(&["push", "-q", "-u", "origin", "main"]);
        git(&["remote", "set-head", "origin", "main"]);

        let repo_path = repo.to_str().expect("repo path utf-8").to_string();

        // Parent project: real layout with a materialized terminal (simulating an
        // actively-used project) + a per-project worktree.on_create hook.
        let parent = ProjectData {
            id: "p1".to_string(),
            name: "Parent".to_string(),
            path: repo_path.clone(),
            layout: Some(LayoutNode::Terminal {
                terminal_id: Some("parent-term".to_string()),
                minimized: false,
                detached: false,
                shell_type: ShellType::Default,
                zoom_level: 1.0,
            }),
            terminal_names: Default::default(),
            hidden_terminals: Default::default(),
            worktree_info: None,
            worktree_ids: Vec::new(),
            folder_color: Default::default(),
            hooks: HooksConfig {
                worktree: WorktreeHooks {
                    on_create: Some("echo WT_HOOK_MARKER".to_string()),
                    ..Default::default()
                },
                ..Default::default()
            },
            is_remote: false,
            connection_id: None,
            service_terminals: Default::default(),
            default_shell: None,
            hook_terminals: Default::default(),
            pinned: false,
            last_activity_at: None,
        };
        let data = WorkspaceData {
            version: 1,
            projects: vec![parent],
            project_order: vec!["p1".to_string()],
            folders: Vec::new(),
            service_panel_heights: Default::default(),
            hook_panel_heights: Default::default(),
            main_window: Default::default(),
            extra_windows: Vec::new(),
        };

        // Real daemon services.
        let (pty_manager, _pty_events) = PtyManager::new(SessionBackend::None);
        let pty_manager = Arc::new(pty_manager);
        let backend: Arc<dyn TerminalBackend> = Arc::new(LocalBackend::new(pty_manager.clone()));
        let terminals: TerminalsRegistry = Arc::new(Mutex::new(Default::default()));
        let hook_runner = Some(HookRunner::new(backend.clone(), terminals.clone()));
        let hook_monitor = Some(HookMonitor::new());

        let mut workspace = Workspace::new(data);
        let mut focus_manager = FocusManager::new();
        let settings = default_settings(); // default worktree.path_template
        let (workspace_tick, _wtrx) = watch::channel(0u64);
        let mut cx = DaemonWorkspaceCx::new(&workspace_tick, &hook_runner, &hook_monitor);

        // Drive the REAL action the daemon dispatches for quick-create.
        let result = execute_action(
            ActionRequest::CreateWorktree {
                project_id: "p1".to_string(),
                branch: "neumie/tezky-medovnik".to_string(),
                create_branch: true,
            },
            &mut workspace,
            WindowId::Main,
            &mut focus_manager,
            &*backend,
            &terminals,
            &settings,
            &mut cx,
        );
        if let okena_app_core::workspace::actions::execute::ActionResult::Err(e) = &result {
            panic!("CreateWorktree action failed: {e}");
        }

        // Find the new worktree project (the non-parent one).
        let new_id = workspace
            .data()
            .projects
            .iter()
            .find(|p| p.id != "p1")
            .map(|p| p.id.clone())
            .expect("a new worktree project was created");

        // (a) INITIAL TERMINAL: layout has a Terminal node with a real id whose
        //     PTY is in the registry.
        let assigned = {
            let p = workspace.project(&new_id).expect("new project");
            match p.layout.as_ref().expect("worktree layout present") {
                LayoutNode::Terminal { terminal_id, .. } => terminal_id.clone(),
                other => panic!("expected Terminal layout node, got {other:?}"),
            }
        };
        let assigned = assigned.expect("worktree initial terminal got a real id");
        assert!(
            terminals.lock().contains_key(&assigned),
            "daemon spawned + registered the worktree's initial terminal PTY"
        );

        // (b) HOOK: on_worktree_create ran exactly once and a live hook terminal
        //     was registered on the new project.
        let history = hook_monitor.as_ref().unwrap().history();
        let wt_hooks: Vec<_> = history.iter().filter(|h| h.hook_type == "on_worktree_create").collect();
        assert_eq!(
            wt_hooks.len(),
            1,
            "on_worktree_create must fire exactly once, full history: {:?}",
            history.iter().map(|h| h.hook_type).collect::<Vec<_>>()
        );
        assert_eq!(
            workspace.project(&new_id).expect("new project").hook_terminals.len(),
            1,
            "one live on_worktree_create hook terminal registered on the worktree project"
        );

        // The hook PTY is a SEPARATE terminal from the initial shell (both live in
        // the registry) — proving the hook does NOT consume the initial slot.
        assert!(terminals.lock().len() >= 2, "initial terminal + hook terminal both in registry");

        // cleanup
        std::fs::remove_dir_all(&repo).ok();
        std::fs::remove_dir_all(&origin).ok();
        if let Some(parent) = repo.parent() {
            std::fs::remove_dir_all(parent.join(format!(
                "{}-wt",
                repo.file_name().unwrap().to_string_lossy()
            )))
            .ok();
        }
    }
}
