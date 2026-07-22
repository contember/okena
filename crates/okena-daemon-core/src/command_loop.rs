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

use okena_app_core::remote_snapshot::build_state_response;
use okena_app_core::workspace::actions::execute::{
    apply_imported_workspace, apply_loaded_session, ensure_terminal,
    ensure_workspace_replacement_allowed, execute_action, import_workspace_data,
    load_session_data_for_shell, spawn_uninitialized_terminals,
};
use okena_core::api::{ActionRequest, ApiGitStatus, ApiServiceInfo, ApiWindow, CommandResult};
use okena_core::git_poll::{GitPollTrigger, git_poll_trigger_for_action};
use okena_remote_server::bridge::{BridgeMessage, BridgeReceiver, RemoteCommand};
use okena_services::manager::ServiceManager;
use okena_terminal::TerminalsRegistry;
use okena_terminal::backend::TerminalBackend;
use okena_workspace::actions::soft_close::{
    begin_soft_close_flow, close_now_flow, probe_busy, undo_soft_close_flow,
};
use okena_workspace::actions::worktree::WorktreeRemovalPlan;
use okena_workspace::context::WorkspaceCx;
use okena_workspace::focus::FocusManager;
use okena_workspace::persistence::AppSettings;
use okena_workspace::state::{WindowId, Workspace};
use parking_lot::Mutex;
use tokio::sync::watch;

use crate::daemon_config::{DaemonConfig, get_settings_schema};
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

fn publish_config_change_after_success(result: &CommandResult, state_version: &watch::Sender<u64>) {
    if matches!(result, CommandResult::Ok(_)) {
        state_version.send_modify(|version| *version = version.wrapping_add(1));
    }
}

/// Run destructive cleanup only while no current project occupies the physical
/// worktree root or one of its subdirectories. The workspace guard fences
/// replacement registration against the check-to-delete window.
fn with_unclaimed_worktree_root<R>(
    workspace: &Arc<Mutex<Workspace>>,
    worktree_path: &std::path::Path,
    cleanup: impl FnOnce() -> R,
) -> Option<R> {
    let workspace = workspace.lock();
    let physical_root = Workspace::physical_path_identity(worktree_path);
    if workspace
        .projects()
        .iter()
        .filter(|project| !project.is_remote)
        .map(|project| Workspace::physical_path_identity(std::path::Path::new(&project.path)))
        .any(|project_path| project_path.starts_with(&physical_root))
    {
        return None;
    }
    Some(cleanup())
}

fn cleanup_created_worktree_if_unclaimed(
    workspace: &Arc<Mutex<Workspace>>,
    worktree_path: &std::path::Path,
    git_root: &std::path::Path,
) {
    let result = with_unclaimed_worktree_root(workspace, worktree_path, || {
        okena_git::verify_linked_worktree_fresh(git_root, worktree_path)
            .and_then(|verified| okena_git::remove_worktree_fast(&verified))
    });
    match result {
        Some(Err(error)) => log::warn!(
            "worktree-create: failed to clean stale checkout at {}: {error}",
            worktree_path.display()
        ),
        None => log::info!(
            "worktree-create: retained checkout now claimed at or below {}",
            worktree_path.display()
        ),
        Some(Ok(())) => {}
    }
}

fn unload_project_services(
    project_id: &str,
    service_manager: &Arc<Mutex<ServiceManager>>,
    service_tick: &watch::Sender<u64>,
    runtime: &tokio::runtime::Handle,
) {
    let reactor_ref = ServiceReactorRef::new(
        service_manager.clone(),
        runtime.clone(),
        service_tick.clone(),
    );
    let mut manager = service_manager.lock();
    let mut cx = reactor_ref.cx();
    manager.unload_project_services(project_id, &mut cx);
}

fn recover_project_services(
    project_id: &str,
    workspace: &Arc<Mutex<Workspace>>,
    service_manager: &Arc<Mutex<ServiceManager>>,
    service_tick: &watch::Sender<u64>,
    runtime: &tokio::runtime::Handle,
) {
    let project_owner = {
        let workspace = workspace.lock();
        workspace
            .project(project_id)
            .map(|project| (project.path.clone(), workspace.data_replacement_epoch()))
    };
    let Some((project_path, data_replacement_epoch)) = project_owner else {
        return;
    };
    if !std::path::Path::new(&project_path).exists() {
        return;
    }
    let reactor_ref = ServiceReactorRef::new(
        service_manager.clone(),
        runtime.clone(),
        service_tick.clone(),
    );
    let mut manager = service_manager.lock();
    let mut cx = reactor_ref.cx();
    manager.set_project_writeback_owner(project_id, &project_path, data_replacement_epoch);
    // The old persistent sessions were intentionally killed before removal;
    // reconnecting their ids here would race their asynchronous teardown.
    manager.load_project_services(project_id, &project_path, &HashMap::new(), &mut cx);
}

fn apply_deferred_hook_actions(
    ws: &mut Workspace,
    project_id: &str,
    outcome: okena_hooks::HookActionOutcome,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
    settings: &AppSettings,
    cx: &mut impl WorkspaceCx,
) -> okena_app_core::workspace::actions::execute::ActionResult {
    let (terminal_actions, hook_results) = outcome;
    let needs_materialization = !terminal_actions.is_empty();
    ws.register_hook_results(hook_results, cx);
    for (command, env) in terminal_actions {
        ws.add_terminal_with_command(project_id, &command, &env, cx);
    }
    if needs_materialization {
        spawn_uninitialized_terminals(ws, project_id, backend, terminals, settings, None, cx)
    } else {
        okena_app_core::workspace::actions::execute::ActionResult::Ok(None)
    }
}

/// Run physical worktree removal off the reactor while the daemon keeps the
/// authoritative project row in `is_closing` state. State is deleted only after
/// Git confirms the checkout is gone; failures restore normal terminal slots.
#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_background_worktree_removal(
    plan: WorktreeRemovalPlan,
    operation_epoch: u64,
    did_stash: bool,
    global_hooks: &okena_workspace::persistence::HooksConfig,
    workspace: &Arc<Mutex<Workspace>>,
    workspace_tick: &watch::Sender<u64>,
    hook_runner: &Option<okena_hooks::HookRunner>,
    hook_monitor: &Option<okena_hooks::HookMonitor>,
    backend: &Arc<dyn TerminalBackend>,
    terminals: &TerminalsRegistry,
    settings: &Arc<Mutex<AppSettings>>,
    service_manager: &Arc<Mutex<ServiceManager>>,
    service_tick: &watch::Sender<u64>,
    runtime: &tokio::runtime::Handle,
) -> CommandResult {
    let terminal_ids = {
        let mut cx = DaemonWorkspaceCx::new(workspace_tick, hook_runner, hook_monitor);
        let mut ws = workspace.lock();
        match ws.prepare_background_worktree_removal(&plan.project_id, &mut cx) {
            Ok(ids) => ids,
            Err(error) => return CommandResult::Err(error),
        }
    };
    // ServiceManager owns restart-on-crash. Unload before killing project PTYs
    // so their exit events cannot schedule replacement services mid-removal.
    unload_project_services(&plan.project_id, service_manager, service_tick, runtime);
    for id in terminal_ids {
        backend.kill(&id);
        terminals.lock().remove(&id);
    }

    let workspace = workspace.clone();
    let workspace_tick = workspace_tick.clone();
    let hook_runner = hook_runner.clone();
    let hook_monitor = hook_monitor.clone();
    let global_hooks = global_hooks.clone();
    let backend = backend.clone();
    let terminals = terminals.clone();
    let settings = settings.clone();
    let service_manager = service_manager.clone();
    let service_tick = service_tick.clone();
    let runtime = runtime.clone();
    tokio::task::spawn_local(async move {
        let task_project_id = plan.project_id.clone();
        let global_hooks_blocking = global_hooks.clone();
        let monitor = hook_monitor.clone();
        let teardown_backend = backend.clone();
        let outcome = tokio::task::spawn_blocking(move || {
            // `kill` is asynchronous for local PTYs. Wait off-reactor until the
            // queued handles and persistent sessions release their checkout CWD.
            teardown_backend.flush_teardown();
            let worktree_path = plan.worktree_path.clone();
            // force_remove = is_dirty && !did_stash — same condition the sync
            // close_worktree path uses to fire the dirty-close safety net. Runs
            // before close hooks and removal, matching the canonical sync flow.
            let dirty_hook = if !did_stash && okena_git::has_uncommitted_changes(&worktree_path) {
                Some(plan.fire_on_dirty_close_headless(&global_hooks_blocking, monitor.as_ref()))
            } else {
                None
            };
            plan.fire_close_hooks_headless(&global_hooks_blocking, monitor.as_ref());
            let removal = plan.remove_fast();
            (plan, removal, dirty_hook)
        })
        .await;
        match outcome {
            Ok((plan, removal, dirty_hook)) => {
                if let Some(Err(error)) = dirty_hook {
                    log::error!(
                        "worktree-close: dirty-close hook failed for {}: {error}",
                        plan.project_id
                    );
                }
                match removal {
                    Ok(()) => {
                        let mut cx =
                            DaemonWorkspaceCx::new(&workspace_tick, &hook_runner, &hook_monitor);
                        let mut ws = workspace.lock();
                        if ws.data_replacement_epoch() != operation_epoch {
                            log::info!(
                                "worktree-close: ignoring stale completion for {}",
                                plan.project_id
                            );
                            return;
                        }
                        let mut focus_manager = FocusManager::new();
                        ws.finish_worktree_removal(
                            &mut focus_manager,
                            &plan,
                            &global_hooks,
                            &mut cx,
                        );
                        for id in ws.drain_pending_terminal_kills() {
                            backend.kill(&id);
                            terminals.lock().remove(&id);
                        }
                    }
                    Err(e) => {
                        log::error!(
                            "worktree-close: git removal failed for {}: {e}",
                            plan.project_id
                        );
                        let app_settings = settings.lock().clone();
                        let mut cx =
                            DaemonWorkspaceCx::new(&workspace_tick, &hook_runner, &hook_monitor);
                        let mut ws = workspace.lock();
                        if ws.data_replacement_epoch() != operation_epoch {
                            log::info!(
                                "worktree-close: ignoring stale failure for {}",
                                plan.project_id
                            );
                            return;
                        }
                        ws.finish_closing_project(&plan.project_id);
                        cx.notify();
                        if let okena_app_core::workspace::actions::execute::ActionResult::Err(
                            spawn_error,
                        ) = spawn_uninitialized_terminals(
                            &mut ws,
                            &plan.project_id,
                            backend.as_ref(),
                            &terminals,
                            &app_settings,
                            None,
                            &mut cx,
                        ) {
                            log::error!(
                                "worktree-close: failed to restore terminals for {}: {spawn_error}",
                                plan.project_id
                            );
                        }
                        if let Some(hm) = &hook_monitor {
                            hm.push_toast(okena_state::Toast::error(format!(
                                "Worktree checkout could not be removed and remains open at {}: {e}",
                                plan.worktree_path.display()
                            )));
                        }
                        drop(ws);
                        recover_project_services(
                            &plan.project_id,
                            &workspace,
                            &service_manager,
                            &service_tick,
                            &runtime,
                        );
                    }
                }
            }
            Err(e) => {
                log::error!("worktree-close: removal task failed: {e}");
                let app_settings = settings.lock().clone();
                let mut cx = DaemonWorkspaceCx::new(&workspace_tick, &hook_runner, &hook_monitor);
                let mut ws = workspace.lock();
                if ws.data_replacement_epoch() != operation_epoch {
                    log::info!("worktree-close: ignoring stale task failure for {task_project_id}");
                    return;
                }
                ws.finish_closing_project(&task_project_id);
                cx.notify();
                if let okena_app_core::workspace::actions::execute::ActionResult::Err(spawn_error) =
                    spawn_uninitialized_terminals(
                        &mut ws,
                        &task_project_id,
                        backend.as_ref(),
                        &terminals,
                        &app_settings,
                        None,
                        &mut cx,
                    )
                {
                    log::error!(
                        "worktree-close: failed to restore terminals for {task_project_id}: {spawn_error}"
                    );
                }
                drop(ws);
                recover_project_services(
                    &task_project_id,
                    &workspace,
                    &service_manager,
                    &service_tick,
                    &runtime,
                );
            }
        }
    });

    CommandResult::Ok(Some(serde_json::json!({ "pending": true })))
}

fn abort_background_worktree_close(
    project_id: &str,
    operation_epoch: u64,
    error: String,
    workspace: &Arc<Mutex<Workspace>>,
    workspace_tick: &watch::Sender<u64>,
    hook_runner: &Option<okena_hooks::HookRunner>,
    hook_monitor: &Option<okena_hooks::HookMonitor>,
) {
    let project_name = {
        let mut cx = DaemonWorkspaceCx::new(workspace_tick, hook_runner, hook_monitor);
        let mut ws = workspace.lock();
        if ws.data_replacement_epoch() != operation_epoch {
            log::info!("worktree-close: ignoring stale abort for {project_id}");
            return;
        }
        let name = ws
            .project(project_id)
            .map(|project| project.name.clone())
            .unwrap_or_else(|| project_id.to_string());
        ws.finish_closing_project(project_id);
        cx.notify();
        name
    };
    log::error!("worktree-close: merge close failed for {project_id}: {error}");
    if let Some(monitor) = hook_monitor {
        monitor.push_toast(okena_state::Toast::error(format!(
            "\"{project_name}\" was not closed: {error}"
        )));
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn_merge_worktree_close(
    project_id: String,
    stash: bool,
    fetch: bool,
    push: bool,
    delete_branch: bool,
    global_hooks: okena_workspace::persistence::HooksConfig,
    workspace: &Arc<Mutex<Workspace>>,
    workspace_tick: &watch::Sender<u64>,
    hook_runner: &Option<okena_hooks::HookRunner>,
    hook_monitor: &Option<okena_hooks::HookMonitor>,
    runtime: &tokio::runtime::Handle,
    backend: &Arc<dyn TerminalBackend>,
    terminals: &TerminalsRegistry,
    settings: &Arc<Mutex<AppSettings>>,
    service_manager: &Arc<Mutex<ServiceManager>>,
    service_tick: &watch::Sender<u64>,
) -> CommandResult {
    let prep = {
        let mut cx = DaemonWorkspaceCx::new(workspace_tick, hook_runner, hook_monitor);
        let mut ws = workspace.lock();
        if ws.is_creating_project(&project_id) {
            return CommandResult::Err("worktree is still being created".to_string());
        }
        if ws.is_project_closing(&project_id) {
            return CommandResult::Err("worktree is already closing".to_string());
        }
        if let Err(error) = ws.ensure_worktree_removal_claim_allowed(&project_id) {
            return CommandResult::Err(error);
        }
        let Some(project) = ws
            .project(&project_id)
            .filter(|project| project.worktree_info.is_some())
        else {
            return CommandResult::Err(format!("not a worktree project: {project_id}"));
        };
        let project_name = project.name.clone();
        let project_path = project.path.clone();
        let project_hooks = project.hooks.clone();
        let main_repo_path = ws.worktree_parent_path(&project_id).unwrap_or_default();
        let folder = ws.folder_for_project_or_parent(&project_id);
        let folder_id = folder.map(|folder| folder.id.clone());
        let folder_name = folder.map(|folder| folder.name.clone());
        ws.mark_closing_project_authoritative(&project_id);
        cx.notify();
        (
            ws.data_replacement_epoch(),
            project_name,
            project_path,
            project_hooks,
            main_repo_path,
            folder_id,
            folder_name,
        )
    };

    let workspace = workspace.clone();
    let workspace_tick = workspace_tick.clone();
    let hook_runner = hook_runner.clone();
    let hook_monitor = hook_monitor.clone();
    let runtime = runtime.clone();
    let backend = backend.clone();
    let terminals = terminals.clone();
    let settings = settings.clone();
    let service_manager = service_manager.clone();
    let service_tick = service_tick.clone();
    tokio::task::spawn_local(async move {
        use okena_workspace::actions::worktree::{
            CloseWorktreeGitOutcome, close_worktree_merge_git,
        };
        let (
            operation_epoch,
            project_name,
            project_path,
            project_hooks,
            main_repo_path,
            folder_id,
            folder_name,
        ) = prep;
        let blocking_project_id = project_id.clone();
        let blocking_global_hooks = global_hooks.clone();
        let blocking_monitor = hook_monitor.clone();
        let outcome = runtime
            .spawn_blocking(move || {
                use std::path::Path;
                let branch =
                    okena_git::get_current_branch(Path::new(&project_path)).unwrap_or_default();
                let default_branch =
                    okena_git::get_default_branch(Path::new(&main_repo_path)).unwrap_or_default();
                let is_dirty = okena_git::has_uncommitted_changes(Path::new(&project_path));
                let merge_enabled =
                    (!is_dirty || stash) && !branch.is_empty() && !default_branch.is_empty();
                if merge_enabled {
                    close_worktree_merge_git(
                        stash && is_dirty,
                        fetch,
                        push,
                        delete_branch,
                        &blocking_project_id,
                        &project_name,
                        &project_path,
                        &branch,
                        &default_branch,
                        &main_repo_path,
                        &project_hooks,
                        &blocking_global_hooks,
                        folder_id.as_deref(),
                        folder_name.as_deref(),
                        blocking_monitor.as_ref(),
                    )
                } else {
                    CloseWorktreeGitOutcome::Ok { did_stash: false }
                }
            })
            .await;

        if workspace.lock().data_replacement_epoch() != operation_epoch {
            log::info!("worktree-close: ignoring stale merge completion for {project_id}");
            return;
        }

        let did_stash = match outcome {
            Err(error) => {
                abort_background_worktree_close(
                    &project_id,
                    operation_epoch,
                    format!("worktree close task failed: {error}"),
                    &workspace,
                    &workspace_tick,
                    &hook_runner,
                    &hook_monitor,
                );
                return;
            }
            Ok(CloseWorktreeGitOutcome::Err(error)) => {
                abort_background_worktree_close(
                    &project_id,
                    operation_epoch,
                    error,
                    &workspace,
                    &workspace_tick,
                    &hook_runner,
                    &hook_monitor,
                );
                return;
            }
            Ok(CloseWorktreeGitOutcome::RebaseConflict { error, hook_plan }) => {
                if let Some(hook_plan) = hook_plan {
                    let outcome = okena_hooks::execute_hook_action_plan(
                        hook_plan,
                        hook_monitor.as_ref(),
                        hook_runner.as_ref(),
                    );
                    let app_settings = settings.lock().clone();
                    let mut cx =
                        DaemonWorkspaceCx::new(&workspace_tick, &hook_runner, &hook_monitor);
                    let mut ws = workspace.lock();
                    if let okena_app_core::workspace::actions::execute::ActionResult::Err(
                        spawn_error,
                    ) = apply_deferred_hook_actions(
                        &mut ws,
                        &project_id,
                        outcome,
                        backend.as_ref(),
                        &terminals,
                        &app_settings,
                        &mut cx,
                    ) {
                        log::error!(
                            "worktree-close: failed to materialize rebase hook terminal for {project_id}: {spawn_error}"
                        );
                    }
                }
                abort_background_worktree_close(
                    &project_id,
                    operation_epoch,
                    error,
                    &workspace,
                    &workspace_tick,
                    &hook_runner,
                    &hook_monitor,
                );
                return;
            }
            Ok(CloseWorktreeGitOutcome::Ok { did_stash }) => did_stash,
        };

        let plan = {
            let mut cx = DaemonWorkspaceCx::new(&workspace_tick, &hook_runner, &hook_monitor);
            let mut ws = workspace.lock();
            let has_before_remove = ws.project(&project_id).is_some_and(|project| {
                project.hooks.worktree.before_remove.is_some()
                    || global_hooks.worktree.before_remove.is_some()
            });
            if has_before_remove {
                None
            } else {
                Some(ws.begin_worktree_removal(&project_id, &global_hooks, &mut cx))
            }
        };

        if let Some(plan) = plan {
            match plan {
                Ok(plan) => {
                    let _ = spawn_background_worktree_removal(
                        plan,
                        operation_epoch,
                        did_stash,
                        &global_hooks,
                        &workspace,
                        &workspace_tick,
                        &hook_runner,
                        &hook_monitor,
                        &backend,
                        &terminals,
                        &settings,
                        &service_manager,
                        &service_tick,
                        &runtime,
                    );
                }
                Err(error) => abort_background_worktree_close(
                    &project_id,
                    operation_epoch,
                    error,
                    &workspace,
                    &workspace_tick,
                    &hook_runner,
                    &hook_monitor,
                ),
            }
            return;
        }

        let result = {
            let app_settings = settings.lock().clone();
            let mut cx = DaemonWorkspaceCx::new(&workspace_tick, &hook_runner, &hook_monitor);
            let mut ws = workspace.lock();
            ws.finish_closing_project(&project_id);
            cx.notify();
            let mut focus_manager = FocusManager::new();
            run_main_workspace_action(
                ActionRequest::CloseWorktree {
                    project_id: project_id.clone(),
                    merge: false,
                    stash: false,
                    fetch: false,
                    push: false,
                    delete_branch: false,
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
        };
        if let CommandResult::Err(error) = result {
            abort_background_worktree_close(
                &project_id,
                operation_epoch,
                error,
                &workspace,
                &workspace_tick,
                &hook_runner,
                &hook_monitor,
            );
        }
    });

    CommandResult::Ok(Some(serde_json::json!({ "pending": true })))
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
    let service_reactor = ServiceReactorRef::new(
        service_manager.clone(),
        runtime.clone(),
        service_tick.clone(),
    );

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
            RemoteCommand::Action(action) => {
                match action {
                    // ── Service actions ──────────────────────────────────────────
                    ActionRequest::StartService {
                        project_id,
                        service_name,
                    } => {
                        let mut sm = service_manager.lock();
                        let mut cx = service_reactor.cx();
                        sm.start_service_action(&project_id, &service_name, &mut cx)
                    }
                    ActionRequest::StopService {
                        project_id,
                        service_name,
                    } => {
                        let mut sm = service_manager.lock();
                        let mut cx = service_reactor.cx();
                        sm.stop_service_action(&project_id, &service_name, &mut cx)
                    }
                    ActionRequest::RestartService {
                        project_id,
                        service_name,
                    } => {
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
                    ActionRequest::SaveCustomTheme {
                        id,
                        config,
                        activate,
                    } => {
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

                    // Session parsing, migration, and worktree validation touch
                    // disk and Git. Keep both the workspace lock and LocalSet
                    // reactor free until the prepared data is ready to swap.
                    ActionRequest::LoadSession { name } => {
                        let app_settings = settings.lock().clone();
                        let session_backend = app_settings.session_backend;
                        let default_shell = app_settings.default_shell.clone();
                        load_workspace_off_reactor(
                            &workspace,
                            &runtime,
                            move || {
                                load_session_data_for_shell(&name, session_backend, &default_shell)
                            },
                            |ws, loaded| {
                                let mut cx = DaemonWorkspaceCx::new(
                                    &workspace_tick,
                                    &hook_runner,
                                    &hook_monitor,
                                );
                                apply_loaded_session(
                                    ws,
                                    &mut focus_manager,
                                    loaded,
                                    &*backend,
                                    &terminals,
                                    &app_settings,
                                    &mut cx,
                                )
                                .into_command_result()
                            },
                        )
                        .await
                    }
                    ActionRequest::ImportWorkspace { path } => {
                        let app_settings = settings.lock().clone();
                        load_workspace_off_reactor(
                            &workspace,
                            &runtime,
                            move || import_workspace_data(&path),
                            |ws, data| {
                                let mut cx = DaemonWorkspaceCx::new(
                                    &workspace_tick,
                                    &hook_runner,
                                    &hook_monitor,
                                );
                                apply_imported_workspace(
                                    ws,
                                    &mut focus_manager,
                                    data,
                                    &*backend,
                                    &terminals,
                                    &app_settings,
                                    &mut cx,
                                )
                                .into_command_result()
                            },
                        )
                        .await
                    }

                    // ── Soft-close: undo (restore the ejected pane) ──────────────
                    ActionRequest::UndoSoftClose { terminal_id } => {
                        let mut cx =
                            DaemonWorkspaceCx::new(&workspace_tick, &hook_runner, &hook_monitor);
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
                        let mut cx =
                            DaemonWorkspaceCx::new(&workspace_tick, &hook_runner, &hook_monitor);
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
                    ActionRequest::CloseTerminal {
                        project_id,
                        terminal_id,
                    } => {
                        let grace = settings.lock().terminal_close_grace_secs;

                        if grace == 0 {
                            // Feature off → immediate close (unchanged behavior).
                            // Snapshot settings BEFORE locking the workspace.
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
                    ActionRequest::CreateWorktree {
                        project_id,
                        branch,
                        create_branch,
                    } => {
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
                                    okena_git::compute_target_paths(
                                        &git_root, &subdir, &template, &branch,
                                    );
                                (git_root, worktree_path, wt_project_path)
                            })
                        };

                        match prepared {
                            None => CommandResult::Err(format!("project not found: {project_id}")),
                            Some((git_root, worktree_path, wt_project_path)) => {
                                // OPTIMISTIC CREATE (symmetric with the optimistic close):
                                // register the worktree row NOW — deferred hooks, no
                                // terminals, layout stays None so the client renders the
                                // "Setting up worktree…" placeholder — then return Ok and
                                // run the slow `git worktree add` checkout in the
                                // BACKGROUND. Previously the checkout was awaited before the
                                // row was even created, so its (repo-scaling) duration WAS
                                // the perceived latency. When the checkout finishes we seed
                                // the layout + spawn the PTY + fire on_worktree_create; on
                                // failure we roll the row back + toast.
                                let app_settings = settings.lock().clone();
                                let new_id = {
                                    let mut cx = DaemonWorkspaceCx::new(
                                        &workspace_tick,
                                        &hook_runner,
                                        &hook_monitor,
                                    );
                                    let mut ws = workspace.lock();
                                    let registered = ws.register_worktree_project_deferred_hooks(
                                        &project_id,
                                        &branch,
                                        &git_root,
                                        &worktree_path,
                                        &wt_project_path,
                                        &app_settings.hooks,
                                        WindowId::Main,
                                        &mut cx,
                                    );
                                    // Mark creating only on success; propagate the
                                    // registration error (parent-missing OR the
                                    // same-branch/path dedupe) to the caller instead of
                                    // masking it as "project not found".
                                    if let Ok(id) = &registered {
                                        ws.mark_creating_project(id);
                                    }
                                    let operation_epoch = ws.data_replacement_epoch();
                                    registered.map(|id| (id, operation_epoch))
                                };
                                match new_id {
                                    Err(e) => CommandResult::Err(e),
                                    Ok((new_id, operation_epoch)) => {
                                        let workspace = workspace.clone();
                                        let workspace_tick = workspace_tick.clone();
                                        let hook_runner = hook_runner.clone();
                                        let hook_monitor = hook_monitor.clone();
                                        let backend = backend.clone();
                                        let terminals = terminals.clone();
                                        let app_settings = app_settings.clone();
                                        let git_root = git_root.clone();
                                        let branch = branch.clone();
                                        let worktree_path = worktree_path.clone();
                                        let new_id_task = new_id.clone();
                                        tokio::task::spawn_local(async move {
                                            let git = {
                                                let git_root = git_root.clone();
                                                let branch = branch.clone();
                                                let target =
                                                    std::path::PathBuf::from(&worktree_path);
                                                tokio::task::spawn_blocking(move || {
                                                let (result, default_branch) = if create_branch {
                                                    let default = okena_git::get_default_branch(&git_root);
                                                    (
                                                        okena_git::create_worktree_with_start_point(
                                                            &git_root, &branch, &target, default.as_deref(),
                                                        ),
                                                        default,
                                                    )
                                                } else {
                                                    (
                                                        okena_git::create_worktree(&git_root, &branch, &target, false),
                                                        None,
                                                    )
                                                };
                                                if result.is_ok()
                                                    && let Some(default_branch) = default_branch
                                                {
                                                    okena_git::fetch_and_fast_forward(
                                                        &git_root,
                                                        &target,
                                                        &default_branch,
                                                    );
                                                }
                                                result
                                            })
                                            .await
                                            };
                                            let stale = workspace.lock().data_replacement_epoch()
                                                != operation_epoch;
                                            if stale {
                                                log::info!(
                                                    "worktree-create: ignoring stale completion for {new_id_task}"
                                                );
                                                if matches!(&git, Ok(Ok(()))) {
                                                    let _ =
                                                        tokio::task::spawn_blocking(move || {
                                                            cleanup_created_worktree_if_unclaimed(
                                                                &workspace,
                                                                std::path::Path::new(
                                                                    &worktree_path,
                                                                ),
                                                                &git_root,
                                                            );
                                                        })
                                                        .await;
                                                }
                                                return;
                                            }
                                            match git {
                                                Ok(Ok(())) => {
                                                    {
                                                        let mut cx = DaemonWorkspaceCx::new(
                                                            &workspace_tick,
                                                            &hook_runner,
                                                            &hook_monitor,
                                                        );
                                                        let mut ws = workspace.lock();
                                                        // Seeds the layout from the parent, then fires on_worktree_create.
                                                        ws.fire_worktree_hooks(
                                                            &new_id_task,
                                                            &app_settings.hooks,
                                                            &mut cx,
                                                        );
                                                        // Clear creating BEFORE spawning — spawn_uninitialized_terminals
                                                        // no-ops while is_creating (guards against spawning into a
                                                        // not-yet-checked-out worktree). The checkout is done here, so the
                                                        // dir exists and the PTYs must actually spawn.
                                                        ws.finish_creating_project(&new_id_task);
                                                        let _ = spawn_uninitialized_terminals(
                                                            &mut ws,
                                                            &new_id_task,
                                                            &*backend,
                                                            &terminals,
                                                            &app_settings,
                                                            None,
                                                            &mut cx,
                                                        );
                                                        ws.notify_data(&mut cx);
                                                    }
                                                }
                                                result => {
                                                    let msg = match result {
                                                        Ok(Err(
                                                            okena_git::GitError::WorktreeExists {
                                                                path,
                                                            },
                                                        )) => format!(
                                                            "Directory '{}' already exists",
                                                            path.display()
                                                        ),
                                                        Ok(Err(e)) => e.to_string(),
                                                        Err(join) => format!(
                                                            "worktree creation task failed: {join}"
                                                        ),
                                                        Ok(Ok(())) => {
                                                            unreachable!("success handled above")
                                                        }
                                                    };
                                                    // Roll the optimistic row back. Clear creating
                                                    // FIRST — remove_stale_worktree skips creating
                                                    // projects.
                                                    {
                                                        let mut cx = DaemonWorkspaceCx::new(
                                                            &workspace_tick,
                                                            &hook_runner,
                                                            &hook_monitor,
                                                        );
                                                        let mut ws = workspace.lock();
                                                        ws.finish_creating_project(&new_id_task);
                                                        ws.remove_stale_worktree(&new_id_task);
                                                        ws.notify_data(&mut cx);
                                                    }
                                                    log::error!(
                                                        "worktree-create: {branch} failed: {msg}"
                                                    );
                                                    if let Some(hm) = &hook_monitor {
                                                        hm.push_toast(okena_state::Toast::error(
                                                            msg,
                                                        ));
                                                    }
                                                    // A failed git command does not prove ownership of
                                                    // anything left at the target, so cleanup is manual.
                                                }
                                            }
                                        });
                                        // OPTIMISTIC reply: the row exists but the
                                        // checkout is still running in the background,
                                        // so `path` does NOT exist on disk yet.
                                        // `pending: true` is the machine-readable signal
                                        // that callers (REST/CLI/agents) must not treat
                                        // this path as ready — it materializes when the
                                        // background checkout finishes, or the row is
                                        // removed from state (+ a toast) on failure. Old
                                        // clients that ignore unknown fields keep working.
                                        CommandResult::Ok(Some(serde_json::json!({
                                            "project_id": new_id,
                                            "path": wt_project_path,
                                            "pending": true,
                                        })))
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
                    // lock. Merge closes run their whole Git pipeline in a detached
                    // task; before_remove-hook closes finish from the PTY-exit loop.
                    ActionRequest::CloseWorktree {
                        project_id,
                        merge,
                        stash,
                        fetch,
                        push,
                        delete_branch,
                    } => {
                        let global_hooks = settings.lock().hooks.clone();
                        if merge {
                            spawn_merge_worktree_close(
                                project_id,
                                stash,
                                fetch,
                                push,
                                delete_branch,
                                global_hooks,
                                &workspace,
                                &workspace_tick,
                                &hook_runner,
                                &hook_monitor,
                                &runtime,
                                &backend,
                                &terminals,
                                &settings,
                                &service_manager,
                                &service_tick,
                            )
                        } else if workspace.lock().is_project_closing(&project_id) {
                            CommandResult::Err("worktree is already closing".to_string())
                        } else {
                            let plan = {
                                let mut cx = DaemonWorkspaceCx::new(
                                    &workspace_tick,
                                    &hook_runner,
                                    &hook_monitor,
                                );
                                let mut ws = workspace.lock();
                                let fast = ws.project(&project_id).is_some_and(|project| {
                                    project.worktree_info.is_some()
                                        && project.hooks.worktree.before_remove.is_none()
                                        && global_hooks.worktree.before_remove.is_none()
                                });
                                if fast {
                                    Some((
                                        ws.data_replacement_epoch(),
                                        ws.begin_worktree_removal(
                                            &project_id,
                                            &global_hooks,
                                            &mut cx,
                                        ),
                                    ))
                                } else {
                                    None
                                }
                            };
                            match plan {
                                None => {
                                    let app_settings = settings.lock().clone();
                                    let mut ws = workspace.lock();
                                    run_main_workspace_action(
                                        ActionRequest::CloseWorktree {
                                            project_id,
                                            merge: false,
                                            stash,
                                            fetch,
                                            push,
                                            delete_branch,
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
                                Some((_, Err(error))) => CommandResult::Err(error),
                                Some((operation_epoch, Ok(plan))) => {
                                    spawn_background_worktree_removal(
                                        plan,
                                        operation_epoch,
                                        false,
                                        &global_hooks,
                                        &workspace,
                                        &workspace_tick,
                                        &hook_runner,
                                        &hook_monitor,
                                        &backend,
                                        &terminals,
                                        &settings,
                                        &service_manager,
                                        &service_tick,
                                        &runtime,
                                    )
                                }
                            }
                        }
                    }

                    // ── Default: workspace-scoped action ─────────────────────────
                    action => {
                        let git_poll_trigger = git_poll_trigger_for_action(&action);
                        let presentation_only_window =
                            matches!(&action, ActionRequest::FocusTerminal { .. });
                        // Resolve the action's explicit target window (if any)
                        // BEFORE moving `action` into `execute_action`. The daemon
                        // serves only the synthetic main window. FocusTerminal may
                        // carry an extra UI window through daemon-side validation.
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
                            Ok(Some(WindowId::Extra(uuid))) if !presentation_only_window => {
                                CommandResult::Err(format!("window not found: {uuid}"))
                            }
                            Ok(_) => {
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
                        }
                    }
                }
            }

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
                let app_settings = settings.lock().clone();
                let ws = workspace.lock();
                match ensure_terminal(&terminal_id, &terminals, &*backend, &ws, &app_settings) {
                    Some(term) => {
                        let (data, sequence) = term.render_snapshot_with_sequence();
                        CommandResult::OkSnapshot { data, sequence }
                    }
                    None => CommandResult::Err(format!("terminal not found: {terminal_id}")),
                }
            }

            // ── PastePath ────────────────────────────────────────────────────
            RemoteCommand::PastePath { terminal_id, text } => {
                let app_settings = settings.lock().clone();
                let ws = workspace.lock();
                match ensure_terminal(&terminal_id, &terminals, &*backend, &ws, &app_settings) {
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

/// Load replacement data on the blocking pool, then atomically apply it.
async fn load_workspace_off_reactor<T, Load, Apply>(
    workspace: &Arc<Mutex<Workspace>>,
    runtime: &tokio::runtime::Handle,
    loader: Load,
    apply: Apply,
) -> CommandResult
where
    T: Send + 'static,
    Load: FnOnce() -> Result<T, String> + Send + 'static,
    Apply: FnOnce(&mut Workspace, T) -> CommandResult,
{
    if let Err(error) = ensure_workspace_replacement_allowed(&workspace.lock()) {
        return CommandResult::Err(error);
    }

    let loaded = match runtime.spawn_blocking(loader).await {
        Ok(Ok(loaded)) => loaded,
        Ok(Err(error)) => return CommandResult::Err(error),
        Err(error) => {
            return CommandResult::Err(format!("workspace loader task failed: {error}"));
        }
    };

    // `apply` repeats the conflict check while this guard is held, closing the
    // race with worktree operations that started while loading was in flight.
    apply(&mut workspace.lock(), loaded)
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

    // Hook panels are persisted, but their PTYs are not reconnected. Tear down
    // persistent backend sessions before dropping the only ids that own them.
    for project_id in &project_ids {
        let stale_ids = {
            let mut cx = DaemonWorkspaceCx::new(workspace_tick, hook_runner, hook_monitor);
            workspace
                .lock()
                .clear_stale_hook_terminals(project_id, &mut cx)
        };
        for terminal_id in stale_ids {
            backend.kill(&terminal_id);
            terminals.lock().remove(&terminal_id);
        }
    }

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
    use std::sync::atomic::{AtomicBool, Ordering};

    use okena_remote_server::bridge::{BridgeReceiver, BridgeSender, bridge_channel};
    use okena_state::{LayoutNode, WorkspaceData};
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

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn failed_close_service_recovery_rearms_current_writeback_owner() {
        let project_dir =
            std::env::temp_dir().join(format!("okena-service-recovery-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&project_dir).expect("create project fixture");
        std::fs::write(project_dir.join("okena.yaml"), "services: []\n")
            .expect("write project services");

        let project_path = project_dir.to_string_lossy().into_owned();
        let mut data = workspace_with_uninitialized_terminal(&project_path);
        data.projects[0]
            .service_terminals
            .insert("stale".to_string(), "old-session".to_string());
        let (workspace_tick, _workspace_rx) = watch::channel(0u64);
        let no_hook_runner = None;
        let no_hook_monitor = None;
        let mut workspace_value = Workspace::new(data);
        let replacement = workspace_value.data().clone();
        let mut workspace_cx =
            DaemonWorkspaceCx::new(&workspace_tick, &no_hook_runner, &no_hook_monitor);
        workspace_value.replace_data(&mut FocusManager::new(), replacement, &mut workspace_cx);
        let current_epoch = workspace_value.data_replacement_epoch();
        let workspace = Arc::new(Mutex::new(workspace_value));

        let terminals: TerminalsRegistry = Arc::new(Mutex::new(HashMap::new()));
        let backend: Arc<dyn TerminalBackend> = Arc::new(StubBackend);
        let service_manager = Arc::new(Mutex::new(ServiceManager::new(backend, terminals)));
        service_manager.lock().set_project_writeback_owner(
            "p1",
            "/stale/path",
            current_epoch.wrapping_sub(1),
        );
        let (service_tick, _service_rx) = watch::channel(0u64);
        let runtime = tokio::runtime::Handle::current();

        unload_project_services("p1", &service_manager, &service_tick, &runtime);
        assert!(
            service_manager
                .lock()
                .service_terminal_writebacks()
                .is_empty()
        );

        recover_project_services("p1", &workspace, &service_manager, &service_tick, &runtime);

        let writebacks = service_manager.lock().service_terminal_writebacks();
        assert_eq!(writebacks.len(), 1);
        let writeback = &writebacks[0];
        assert_eq!(writeback.project_id, "p1");
        assert_eq!(writeback.project_path, project_path);
        assert_eq!(writeback.data_replacement_epoch, current_epoch);
        assert!(writeback.terminal_ids.is_empty());

        let mut workspace = workspace.lock();
        assert_eq!(
            workspace.data_replacement_epoch(),
            writeback.data_replacement_epoch
        );
        assert_eq!(
            workspace.project("p1").map(|project| project.path.as_str()),
            Some(project_path.as_str())
        );
        workspace.sync_service_terminals("p1", writeback.terminal_ids.clone(), &mut workspace_cx);
        assert!(
            workspace
                .project("p1")
                .expect("project")
                .service_terminals
                .is_empty()
        );
        drop(workspace);

        std::fs::remove_dir_all(project_dir).expect("remove project fixture");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn blocking_workspace_loader_does_not_stall_local_reactor() {
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let workspace = Arc::new(Mutex::new(Workspace::new(empty_workspace_data())));
                let runtime = tokio::runtime::Handle::current();
                let (started_tx, started_rx) = oneshot::channel();
                let (release_tx, release_rx) = std::sync::mpsc::channel();
                let task_workspace = workspace.clone();
                let load = tokio::task::spawn_local(async move {
                    load_workspace_off_reactor(
                        &task_workspace,
                        &runtime,
                        move || {
                            let _ = started_tx.send(());
                            release_rx.recv().map_err(|error| error.to_string())?;
                            Ok(empty_workspace_data())
                        },
                        |_workspace, _data| CommandResult::Ok(None),
                    )
                    .await
                });

                started_rx.await.expect("blocking loader started");
                let reactor_progressed = Arc::new(AtomicBool::new(false));
                let progressed = reactor_progressed.clone();
                tokio::task::spawn_local(async move {
                    tokio::task::yield_now().await;
                    progressed.store(true, Ordering::Release);
                })
                .await
                .expect("reactor task completed");
                assert!(
                    reactor_progressed.load(Ordering::Acquire),
                    "LocalSet tasks must progress while the loader is blocked"
                );

                release_tx.send(()).expect("release blocking loader");
                assert!(matches!(
                    load.await.expect("workspace loader task joined"),
                    CommandResult::Ok(_)
                ));
            })
            .await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn workspace_conflict_started_during_load_rejects_atomic_swap() {
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let workspace = Arc::new(Mutex::new(Workspace::new(
                    workspace_with_worktree_child(),
                )));
                let runtime = tokio::runtime::Handle::current();
                let (workspace_tick, _workspace_rx) = watch::channel(0u64);
                let backend: Arc<dyn TerminalBackend> = Arc::new(StubBackend);
                let terminals: TerminalsRegistry = Arc::new(Mutex::new(Default::default()));
                let settings = default_settings();
                let (started_tx, started_rx) = oneshot::channel();
                let (release_tx, release_rx) = std::sync::mpsc::channel();
                let task_workspace = workspace.clone();
                let task_backend = backend.clone();
                let task_terminals = terminals.clone();
                let load = tokio::task::spawn_local(async move {
                    let mut focus_manager = FocusManager::new();
                    load_workspace_off_reactor(
                        &task_workspace,
                        &runtime,
                        move || {
                            let _ = started_tx.send(());
                            release_rx.recv().map_err(|error| error.to_string())?;
                            Ok(okena_workspace::persistence::LoadedWorkspace {
                                data: empty_workspace_data(),
                                stale_terminal_ids: Vec::new(),
                            })
                        },
                        |ws, loaded| {
                            let mut cx = DaemonWorkspaceCx::new(
                                &workspace_tick,
                                &None,
                                &None,
                            );
                            apply_loaded_session(
                                ws,
                                &mut focus_manager,
                                loaded,
                                &*task_backend,
                                &task_terminals,
                                &settings,
                                &mut cx,
                            )
                            .into_command_result()
                        },
                    )
                    .await
                });

                started_rx.await.expect("blocking loader started");
                let protected_epoch = {
                    let mut ws = workspace.lock();
                    ws.mark_creating_project("wt1");
                    ws.data_replacement_epoch()
                };
                release_tx.send(()).expect("release blocking loader");

                let result = load.await.expect("workspace loader task joined");
                assert!(matches!(
                    result,
                    CommandResult::Err(ref error)
                        if error == "cannot replace workspace while worktree 'Project wt1' is being created"
                ));
                let ws = workspace.lock();
                assert!(ws.project("p1").is_some());
                assert!(ws.project("wt1").is_some());
                assert!(ws.is_creating_project("wt1"));
                assert_eq!(ws.data_replacement_epoch(), protected_epoch);
            })
            .await;
    }

    #[test]
    fn deferred_hook_terminal_actions_are_materialized_immediately() {
        let mut data = workspace_with_worktree_child();
        data.projects[1].layout = None;
        let mut workspace = Workspace::new(data);
        let backend = RestoringBackend;
        let terminals: TerminalsRegistry = Arc::new(Mutex::new(Default::default()));
        let (workspace_tick, _receiver) = watch::channel(0u64);
        let hook_runner = None;
        let hook_monitor = None;
        let mut cx = DaemonWorkspaceCx::new(&workspace_tick, &hook_runner, &hook_monitor);

        let result = apply_deferred_hook_actions(
            &mut workspace,
            "wt1",
            (vec![("git status".to_string(), HashMap::new())], Vec::new()),
            &backend,
            &terminals,
            &default_settings(),
            &mut cx,
        );

        assert!(matches!(
            result,
            okena_app_core::workspace::actions::execute::ActionResult::Ok(_)
        ));
        assert!(matches!(
            workspace.project("wt1").and_then(|project| project.layout.as_ref()),
            Some(LayoutNode::Terminal {
                terminal_id: Some(id),
                ..
            }) if id == "restored-terminal"
        ));
        assert!(terminals.lock().contains_key("restored-terminal"));
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

    #[test]
    fn stale_create_cleanup_skips_root_claimed_by_replacement_project() {
        let claimed_root = std::env::temp_dir().join("replacement-worktree");
        let mut data = workspace_with_worktree_child();
        data.projects[1].path = claimed_root.to_string_lossy().into_owned();
        let workspace = Arc::new(Mutex::new(Workspace::new(data)));
        let unnormalized_root = claimed_root.join("subdir").join("..");

        let result = with_unclaimed_worktree_root(&workspace, &unnormalized_root, || {
            panic!("claimed replacement root must not be cleaned")
        });

        assert!(result.is_none());
    }

    #[test]
    fn stale_create_cleanup_skips_descendant_claimed_by_replacement_project() {
        let worktree_root = std::env::temp_dir().join("replacement-worktree");
        let claimed_project = worktree_root.join("packages").join("app");
        let mut data = workspace_with_worktree_child();
        data.projects[1].path = claimed_project.to_string_lossy().into_owned();
        let workspace = Arc::new(Mutex::new(Workspace::new(data)));

        let result = with_unclaimed_worktree_root(&workspace, &worktree_root, || {
            panic!("replacement project below the checkout root must prevent cleanup")
        });

        assert!(result.is_none());
    }

    #[test]
    fn stale_create_cleanup_holds_ownership_guard_through_delete() {
        let workspace = Arc::new(Mutex::new(Workspace::new(empty_workspace_data())));
        let worktree_root = std::env::temp_dir().join("unclaimed-worktree");

        let result = with_unclaimed_worktree_root(&workspace, &worktree_root, || {
            assert!(
                workspace.try_lock().is_none(),
                "replacement registration must not interleave after the ownership check"
            );
            "cleaned"
        });

        assert_eq!(result, Some("cleaned"));
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

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn merge_close_rejects_worktree_still_being_created() {
        let h = harness();
        {
            let mut workspace = h.workspace.lock();
            *workspace = Workspace::new(workspace_with_worktree_child());
            workspace.mark_creating_project("wt1");
        }
        let workspace_for_assert = h.workspace.clone();
        let (bridge_tx, bridge_rx) = bridge_channel();

        let local = tokio::task::LocalSet::new();
        local
            .run_until(async move {
                let handle = h.spawn_loop(bridge_rx);
                let result = request(
                    &bridge_tx,
                    RemoteCommand::Action(ActionRequest::CloseWorktree {
                        project_id: "wt1".into(),
                        merge: true,
                        stash: false,
                        fetch: false,
                        push: false,
                        delete_branch: false,
                    }),
                    "CloseWorktree",
                )
                .await;

                assert!(
                    matches!(&result, CommandResult::Err(error) if error == "worktree is still being created"),
                    "mid-create merge close must be rejected: {result:?}"
                );
                {
                    let workspace = workspace_for_assert.lock();
                    assert!(workspace.project("wt1").is_some());
                    assert!(workspace.is_creating_project("wt1"));
                    assert!(!workspace.is_project_closing("wt1"));
                }

                drop(bridge_tx);
                handle.await.expect("loop task joins");
            })
            .await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn non_merge_close_rejects_worktree_already_closing() {
        let h = harness();
        {
            let mut workspace = h.workspace.lock();
            *workspace = Workspace::new(workspace_with_worktree_child());
            workspace.mark_closing_project_authoritative("wt1");
        }
        let workspace_for_assert = h.workspace.clone();
        let (bridge_tx, bridge_rx) = bridge_channel();

        let local = tokio::task::LocalSet::new();
        local
            .run_until(async move {
                let handle = h.spawn_loop(bridge_rx);
                let result = request(
                    &bridge_tx,
                    RemoteCommand::Action(ActionRequest::CloseWorktree {
                        project_id: "wt1".into(),
                        merge: false,
                        stash: false,
                        fetch: false,
                        push: false,
                        delete_branch: false,
                    }),
                    "CloseWorktree",
                )
                .await;

                assert!(matches!(
                    &result,
                    CommandResult::Err(error) if error == "worktree is already closing"
                ));
                let workspace = workspace_for_assert.lock();
                assert!(workspace.project("wt1").is_some());
                assert!(workspace.is_project_closing("wt1"));
                drop(workspace);

                drop(bridge_tx);
                handle.await.expect("loop task joins");
            })
            .await;
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn merge_close_keeps_command_loop_responsive_during_hook() {
        use std::process::Command;
        use std::time::Duration;

        let repo = std::env::temp_dir().join(format!(
            "okena-close-responsive-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let worktree = repo.with_extension("worktree");
        std::fs::create_dir_all(&repo).expect("create repository directory");
        let git = |cwd: &std::path::Path, args: &[&str]| {
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
        };
        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["config", "user.email", "test@okena.local"]);
        git(&repo, &["config", "user.name", "Okena Test"]);
        std::fs::write(repo.join("file.txt"), "base\n").expect("write fixture");
        git(&repo, &["add", "file.txt"]);
        git(&repo, &["commit", "-q", "-m", "base"]);
        git(
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

        let h = harness();
        {
            let mut data = workspace_with_worktree_child();
            data.projects[0].path = repo.to_string_lossy().into_owned();
            data.projects[1].path = worktree.to_string_lossy().into_owned();
            let metadata = data.projects[1]
                .worktree_info
                .as_mut()
                .expect("worktree metadata");
            metadata.main_repo_path = repo.to_string_lossy().into_owned();
            metadata.worktree_path = worktree.to_string_lossy().into_owned();
            metadata.branch_name = "feature".into();
            *h.workspace.lock() = Workspace::new(data);
            h.settings.lock().hooks.worktree.pre_merge = Some("sleep 1".into());
        }
        let workspace_for_assert = h.workspace.clone();
        let (bridge_tx, bridge_rx) = bridge_channel();

        let local = tokio::task::LocalSet::new();
        local
            .run_until(async move {
                let handle = h.spawn_loop(bridge_rx);
                let close = tokio::time::timeout(
                    Duration::from_millis(500),
                    request(
                        &bridge_tx,
                        RemoteCommand::Action(ActionRequest::CloseWorktree {
                            project_id: "wt1".into(),
                            merge: true,
                            stash: false,
                            fetch: false,
                            push: false,
                            delete_branch: false,
                        }),
                        "CloseWorktree",
                    ),
                )
                .await
                .expect("close is accepted before the slow hook finishes");
                assert!(
                    matches!(close, CommandResult::Ok(Some(ref value)) if value["pending"] == true),
                    "background close must report pending: {close:?}"
                );

                tokio::time::timeout(
                    Duration::from_millis(500),
                    request(&bridge_tx, RemoteCommand::GetState, "GetState during close"),
                )
                .await
                .expect("command loop remains responsive while pre_merge runs");
                assert!(workspace_for_assert.lock().is_project_closing("wt1"));

                tokio::time::timeout(Duration::from_secs(5), async {
                    loop {
                        if workspace_for_assert.lock().project("wt1").is_none() {
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(25)).await;
                    }
                })
                .await
                .expect("background close completes");

                drop(bridge_tx);
                handle.await.expect("loop task joins");
            })
            .await;

        std::fs::remove_dir_all(&repo).ok();
        std::fs::remove_dir_all(&worktree).ok();
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
            is_creating: false,
            is_closing: false,
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
    //  * `restore_boot_path_fires_on_project_open` drives the actual daemon-boot
    //    entrypoint (`materialize_uninitialized_terminals`, called from
    //    `daemon.run()`) against a RESTORED project that has `project.on_open`
    //    configured. Result: the monitor records exactly one execution -> the
    //    on_project_open hook fires on restore, via `fire_project_open_hooks`
    //    (okena-workspace actions/project.rs) — the daemon restores projects
    //    through `Workspace::new`, never `add_project`, so the boot path needs
    //    its own fire.
    //
    //  * `add_project_fires_on_project_open` drives `ws.add_project` (the OTHER
    //    fire_on_project_open call site) with the SAME services. Result: the
    //    monitor records exactly one `on_project_open` execution -> the firing
    //    machinery works from both entrypoints.

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
            is_creating: false,
            is_closing: false,
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
        let workspace = Arc::new(Mutex::new(Workspace::new(workspace_restored_with_on_open(
            tmp_path,
            "echo HOOK_MARKER",
        ))));
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
            match ws
                .project("p1")
                .expect("p1")
                .layout
                .as_ref()
                .expect("layout")
            {
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
            workspace
                .lock()
                .project("p1")
                .expect("p1")
                .hook_terminals
                .len(),
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
            workspace
                .lock()
                .project("p1")
                .expect("p1")
                .hook_terminals
                .is_empty(),
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
        assert_eq!(
            hooks.len(),
            1,
            "exactly one live hook terminal after re-fire"
        );
    }

    #[test]
    fn restore_boot_path_kills_persistent_stale_hook_session() {
        struct RecordingKillBackend {
            killed: Arc<Mutex<Vec<String>>>,
        }

        impl TerminalBackend for RecordingKillBackend {
            fn transport(&self) -> Arc<dyn TerminalTransport> {
                Arc::new(StubTransport)
            }

            fn create_terminal(
                &self,
                _cwd: &str,
                _shell: Option<&ShellType>,
            ) -> anyhow::Result<String> {
                anyhow::bail!("not used")
            }

            fn reconnect_terminal(
                &self,
                _terminal_id: &str,
                _cwd: &str,
                _shell: Option<&ShellType>,
            ) -> anyhow::Result<String> {
                anyhow::bail!("not used")
            }

            fn kill(&self, terminal_id: &str) {
                self.killed.lock().push(terminal_id.to_string());
            }

            fn supports_buffer_capture(&self) -> bool {
                false
            }

            fn capture_buffer(&self, _terminal_id: &str) -> Option<std::path::PathBuf> {
                None
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

        use okena_workspace::state::{HookTerminalEntry, HookTerminalStatus};

        let mut data = workspace_restored_with_on_open("/tmp", "");
        data.projects[0].layout = None;
        data.projects[0].hooks = Default::default();
        data.projects[0].hook_terminals.insert(
            "persistent-stale-hook".to_string(),
            HookTerminalEntry {
                label: "on_project_open".to_string(),
                status: HookTerminalStatus::Succeeded,
                hook_type: "on_project_open".to_string(),
                command: "echo old".to_string(),
                cwd: "/tmp".to_string(),
            },
        );
        let workspace = Arc::new(Mutex::new(Workspace::new(data)));
        let killed = Arc::new(Mutex::new(Vec::new()));
        let backend: Arc<dyn TerminalBackend> = Arc::new(RecordingKillBackend {
            killed: killed.clone(),
        });
        let terminals: TerminalsRegistry = Arc::new(Mutex::new(Default::default()));
        let settings = Arc::new(Mutex::new(default_settings()));
        let (workspace_tick, _wtrx) = watch::channel(0u64);

        materialize_uninitialized_terminals(
            &*backend,
            &workspace,
            &workspace_tick,
            &None,
            &None,
            &terminals,
            &settings,
        );

        assert_eq!(killed.lock().as_slice(), &["persistent-stale-hook"]);
        assert!(
            workspace
                .lock()
                .project("p1")
                .unwrap()
                .hook_terminals
                .is_empty()
        );
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

        workspace
            .add_project(
                "Test".to_string(),
                tmp_path,
                true,
                &app_settings.hooks,
                WindowId::Main,
                &mut cx,
            )
            .expect("add project");

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
        fn create_terminal(
            &self,
            _cwd: &str,
            _shell: Option<&ShellType>,
        ) -> anyhow::Result<String> {
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

    struct RestoringBackend;

    impl TerminalBackend for RestoringBackend {
        fn transport(&self) -> Arc<dyn TerminalTransport> {
            Arc::new(StubTransport)
        }
        fn create_terminal(
            &self,
            _cwd: &str,
            _shell: Option<&ShellType>,
        ) -> anyhow::Result<String> {
            Ok("restored-terminal".to_string())
        }
        fn reconnect_terminal(
            &self,
            _terminal_id: &str,
            _cwd: &str,
            _shell: Option<&ShellType>,
        ) -> anyhow::Result<String> {
            anyhow::bail!("restoring backend: reconnect not supported")
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

    struct RemovalBarrierBackend {
        killed: std::sync::atomic::AtomicBool,
        flush_started: std::sync::atomic::AtomicBool,
        release: Mutex<std::sync::mpsc::Receiver<()>>,
    }

    impl TerminalBackend for RemovalBarrierBackend {
        fn transport(&self) -> Arc<dyn TerminalTransport> {
            Arc::new(StubTransport)
        }

        fn create_terminal(
            &self,
            _cwd: &str,
            _shell: Option<&ShellType>,
        ) -> anyhow::Result<String> {
            anyhow::bail!("removal barrier backend does not create terminals")
        }

        fn reconnect_terminal(
            &self,
            _terminal_id: &str,
            _cwd: &str,
            _shell: Option<&ShellType>,
        ) -> anyhow::Result<String> {
            anyhow::bail!("removal barrier backend does not reconnect terminals")
        }

        fn kill(&self, _terminal_id: &str) {
            self.killed.store(true, std::sync::atomic::Ordering::SeqCst);
        }

        fn flush_teardown(&self) {
            assert!(
                self.killed.load(std::sync::atomic::Ordering::SeqCst),
                "project PTYs must be killed before the teardown barrier"
            );
            self.flush_started
                .store(true, std::sync::atomic::Ordering::SeqCst);
            self.release
                .lock()
                .recv_timeout(std::time::Duration::from_secs(2))
                .expect("test releases teardown barrier");
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
            is_creating: false,
            is_closing: false,
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
        let backend: Arc<dyn TerminalBackend> = Arc::new(RecordingBackend {
            killed: killed.clone(),
        });
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
        assert!(
            workspace.project("p1").is_none(),
            "project removed from state"
        );
        assert_eq!(
            &*killed.lock(),
            &vec!["t1".to_string()],
            "the deleted project's terminal PTY was killed, not leaked"
        );
    }

    /// A parent project plus a worktree child whose row is present but whose
    /// checkout is still being created (marked via `mark_creating_project` by
    /// the caller). Mirrors the optimistic-create window where the row exists
    /// but `git worktree add` hasn't finished.
    fn workspace_with_worktree_child() -> WorkspaceData {
        use okena_state::{LayoutNode, ProjectData, WorktreeMetadata};
        let mk = |id: &str, worktree_info: Option<WorktreeMetadata>, worktree_ids: Vec<String>| {
            ProjectData {
                id: id.to_string(),
                name: format!("Project {id}"),
                path: "/tmp".to_string(),
                layout: Some(LayoutNode::Terminal {
                    terminal_id: None,
                    minimized: false,
                    detached: false,
                    shell_type: ShellType::Default,
                    zoom_level: 1.0,
                }),
                terminal_names: Default::default(),
                hidden_terminals: Default::default(),
                worktree_info,
                worktree_ids,
                folder_color: Default::default(),
                hooks: Default::default(),
                is_remote: false,
                connection_id: None,
                service_terminals: Default::default(),
                default_shell: None,
                hook_terminals: Default::default(),
                pinned: false,
                last_activity_at: None,
                is_creating: false,
                is_closing: false,
            }
        };
        let parent = mk("p1", None, vec!["wt1".to_string()]);
        let child = mk(
            "wt1",
            Some(WorktreeMetadata {
                parent_project_id: "p1".to_string(),
                color_override: None,
                main_repo_path: "/tmp".to_string(),
                worktree_path: "/tmp/worktrees/wt1".to_string(),
                branch_name: String::new(),
            }),
            Vec::new(),
        );
        WorkspaceData {
            version: 1,
            projects: vec![parent, child],
            project_order: vec!["p1".to_string()],
            folders: Vec::new(),
            service_panel_heights: Default::default(),
            hook_panel_heights: Default::default(),
            main_window: Default::default(),
            extra_windows: Vec::new(),
        }
    }

    /// A worktree row whose optimistic create is still in flight (`is_creating`)
    /// must reject a generic `DeleteProject` action rather than dropping the row
    /// mid-checkout — otherwise the delete races the background `git worktree
    /// add` and strands an orphaned, git-registered worktree with no row. The
    /// guard lives in the generic `delete_project` execute wrapper, so it must
    /// hold on this daemon path too.
    #[test]
    fn delete_project_rejected_while_worktree_creating() {
        let backend: Arc<dyn TerminalBackend> = Arc::new(StubBackend);
        let terminals: TerminalsRegistry = Arc::new(Mutex::new(Default::default()));
        let mut workspace = Workspace::new(workspace_with_worktree_child());
        // Seed the transient creating state the way the daemon does — a freshly
        // constructed `Workspace` starts with an empty lifecycle tracker, so the
        // persisted `is_creating` mirror alone would not trip the guard.
        workspace.mark_creating_project("wt1");
        let mut focus_manager = FocusManager::new();
        let settings = default_settings();
        let (workspace_tick, _wtrx) = watch::channel(0u64);

        let result = run_main_workspace_action(
            ActionRequest::DeleteProject {
                project_id: "wt1".to_string(),
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
            matches!(&result, CommandResult::Err(e) if e == "worktree is still being created"),
            "mid-create delete must be rejected: {result:?}"
        );
        assert!(
            workspace.project("wt1").is_some(),
            "the worktree row survives the rejected delete"
        );
        assert!(
            workspace.is_creating_project("wt1"),
            "creating flag untouched by the rejected delete"
        );
    }

    #[test]
    fn stale_background_close_abort_cannot_mutate_reused_project_id() {
        let workspace = Arc::new(Mutex::new(Workspace::new(workspace_with_worktree_child())));
        let hook_monitor = Some(okena_hooks::HookMonitor::new());
        let hook_runner = None;
        let (workspace_tick, _receiver) = watch::channel(0u64);

        {
            let mut ws = workspace.lock();
            ws.mark_closing_project_authoritative("wt1");
            let mut replacement = workspace_with_worktree_child();
            replacement.projects[1].name = "Replacement project".to_string();
            let mut focus_manager = FocusManager::new();
            let mut cx = DaemonWorkspaceCx::new(&workspace_tick, &hook_runner, &hook_monitor);
            ws.replace_data(&mut focus_manager, replacement, &mut cx);
        }

        abort_background_worktree_close(
            "wt1",
            0,
            "old operation failed".to_string(),
            &workspace,
            &workspace_tick,
            &hook_runner,
            &hook_monitor,
        );

        let ws = workspace.lock();
        let project = ws.project("wt1").expect("replacement project retained");
        assert_eq!(project.name, "Replacement project");
        assert!(!project.is_closing);
        assert!(
            hook_monitor
                .as_ref()
                .expect("monitor")
                .drain_pending_toasts()
                .is_empty()
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn background_removal_failure_restores_authoritative_project() {
        use std::process::Command;
        use std::time::Duration;

        let fixture = std::env::temp_dir().join(format!(
            "okena-remove-failure-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let repo = fixture.join("main");
        let invalid_worktree = fixture.join("worktree");
        let git = |cwd: &std::path::Path, args: &[&str]| {
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
        };
        std::fs::create_dir_all(&repo).expect("create repository");
        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["config", "user.email", "test@okena.local"]);
        git(&repo, &["config", "user.name", "Okena Test"]);
        std::fs::write(repo.join("base.txt"), "base\n").expect("write base");
        git(&repo, &["add", "base.txt"]);
        git(&repo, &["commit", "-q", "-m", "base"]);
        git(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "feature",
                invalid_worktree.to_str().expect("utf-8 worktree path"),
            ],
        );

        let mut data = workspace_with_worktree_child();
        data.projects[0].path = repo.to_string_lossy().into_owned();
        data.projects[1].path = invalid_worktree.to_string_lossy().into_owned();
        let metadata = data.projects[1]
            .worktree_info
            .as_mut()
            .expect("worktree metadata");
        metadata.worktree_path = invalid_worktree.to_string_lossy().into_owned();
        metadata.main_repo_path = repo.to_string_lossy().into_owned();
        metadata.branch_name = "feature".to_string();
        if let Some(LayoutNode::Terminal { terminal_id, .. }) = data.projects[1].layout.as_mut() {
            *terminal_id = Some("terminal-1".into());
        }
        let workspace = Arc::new(Mutex::new(Workspace::new(data)));
        let backend: Arc<dyn TerminalBackend> = Arc::new(RestoringBackend);
        let terminals: TerminalsRegistry = Arc::new(Mutex::new(Default::default()));
        let settings = Arc::new(Mutex::new(default_settings()));
        let hook_monitor_service = okena_hooks::HookMonitor::new();
        let hook_monitor = Some(hook_monitor_service.clone());
        let hook_runner = None;
        let (workspace_tick, _receiver) = watch::channel(0u64);
        let service_manager = Arc::new(Mutex::new(ServiceManager::new(
            backend.clone(),
            terminals.clone(),
        )));
        let (service_tick, _service_receiver) = watch::channel(0u64);
        let runtime = tokio::runtime::Handle::current();
        let (operation_epoch, plan) = {
            let mut cx = DaemonWorkspaceCx::new(&workspace_tick, &hook_runner, &hook_monitor);
            let mut ws = workspace.lock();
            let operation_epoch = ws.data_replacement_epoch();
            let plan = ws
                .begin_worktree_removal("wt1", &Default::default(), &mut cx)
                .expect("build removal plan");
            (operation_epoch, plan)
        };

        // Provenance is valid when the plan is built. Replace the checkout
        // afterwards; the unrelated directory must never be recursively removed.
        std::fs::remove_dir_all(&invalid_worktree).expect("remove checkout directory");
        std::fs::create_dir(&invalid_worktree).expect("create unrelated replacement");
        let sentinel = invalid_worktree.join("must-survive.txt");
        std::fs::write(&sentinel, "unrelated data").expect("write sentinel");

        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let result = spawn_background_worktree_removal(
                    plan,
                    operation_epoch,
                    false,
                    &Default::default(),
                    &workspace,
                    &workspace_tick,
                    &hook_runner,
                    &hook_monitor,
                    &backend,
                    &terminals,
                    &settings,
                    &service_manager,
                    &service_tick,
                    &runtime,
                );
                assert!(
                    matches!(result, CommandResult::Ok(Some(ref value)) if value["pending"] == true)
                );

                tokio::time::timeout(Duration::from_secs(2), async {
                    loop {
                        if !workspace.lock().is_project_closing("wt1") {
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                })
                .await
                .expect("failed removal rolls back");
            })
            .await;

        let workspace_guard = workspace.lock();
        let project = workspace_guard
            .project("wt1")
            .expect("project row retained");
        assert!(!project.is_closing);
        assert!(matches!(
            project.layout,
            Some(LayoutNode::Terminal {
                terminal_id: Some(ref id),
                ..
            }) if id == "restored-terminal"
        ));
        drop(workspace_guard);
        let expected_service_path = invalid_worktree.to_string_lossy().into_owned();
        assert_eq!(
            service_manager.lock().project_path("wt1"),
            Some(&expected_service_path),
            "failed removal restores service ownership"
        );
        assert!(terminals.lock().contains_key("restored-terminal"));
        assert_eq!(
            std::fs::read_to_string(&sentinel).expect("replacement survives"),
            "unrelated data"
        );
        assert_eq!(
            hook_monitor_service.drain_pending_toasts().len(),
            1,
            "failure is surfaced to clients"
        );
        std::fs::remove_dir_all(fixture).ok();
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn background_removal_waits_for_teardown_then_runs_hooks_in_order() {
        use std::process::Command;
        use std::time::Duration;

        let repo = std::env::temp_dir().join(format!(
            "okena-close-hook-order-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let worktree = repo.with_extension("worktree");
        let marker = repo.with_extension("hooks.log");
        std::fs::create_dir_all(&repo).expect("create repository directory");
        let git = |cwd: &std::path::Path, args: &[&str]| {
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
        };
        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["config", "user.email", "test@okena.local"]);
        git(&repo, &["config", "user.name", "Okena Test"]);
        std::fs::write(repo.join("file.txt"), "base\n").expect("write fixture");
        git(&repo, &["add", "file.txt"]);
        git(&repo, &["commit", "-q", "-m", "base"]);
        git(
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
        std::fs::write(worktree.join("file.txt"), "dirty\n").expect("dirty worktree");

        let mut data = workspace_with_worktree_child();
        data.projects[0].path = repo.to_string_lossy().into_owned();
        data.projects[1].path = worktree.to_string_lossy().into_owned();
        let metadata = data.projects[1]
            .worktree_info
            .as_mut()
            .expect("worktree metadata");
        metadata.main_repo_path = repo.to_string_lossy().into_owned();
        metadata.worktree_path = worktree.to_string_lossy().into_owned();
        metadata.branch_name = "feature".into();
        if let Some(LayoutNode::Terminal { terminal_id, .. }) = data.projects[1].layout.as_mut() {
            *terminal_id = Some("terminal-1".to_string());
        }
        let workspace = Arc::new(Mutex::new(Workspace::new(data)));
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let barrier_backend = Arc::new(RemovalBarrierBackend {
            killed: std::sync::atomic::AtomicBool::new(false),
            flush_started: std::sync::atomic::AtomicBool::new(false),
            release: Mutex::new(release_rx),
        });
        let backend: Arc<dyn TerminalBackend> = barrier_backend.clone();
        let terminals: TerminalsRegistry = Arc::new(Mutex::new(Default::default()));
        let mut settings_value = default_settings();
        settings_value.hooks.worktree.on_dirty_close = Some(format!(
            "printf 'dirty\\n' >> '{}'",
            marker.to_string_lossy()
        ));
        settings_value.hooks.worktree.on_close = Some(format!(
            "printf 'close\\n' >> '{}'",
            marker.to_string_lossy()
        ));
        let global_hooks = settings_value.hooks.clone();
        let settings = Arc::new(Mutex::new(settings_value));
        let hook_runner = None;
        let hook_monitor = None;
        let (workspace_tick, _receiver) = watch::channel(0u64);
        let service_manager = Arc::new(Mutex::new(ServiceManager::new(
            backend.clone(),
            terminals.clone(),
        )));
        let (service_tick, _service_receiver) = watch::channel(0u64);
        let runtime = tokio::runtime::Handle::current();
        let (operation_epoch, plan) = {
            let mut cx = DaemonWorkspaceCx::new(&workspace_tick, &hook_runner, &hook_monitor);
            let mut ws = workspace.lock();
            let operation_epoch = ws.data_replacement_epoch();
            let plan = ws
                .begin_worktree_removal("wt1", &global_hooks, &mut cx)
                .expect("build removal plan");
            (operation_epoch, plan)
        };

        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let result = spawn_background_worktree_removal(
                    plan,
                    operation_epoch,
                    false,
                    &global_hooks,
                    &workspace,
                    &workspace_tick,
                    &hook_runner,
                    &hook_monitor,
                    &backend,
                    &terminals,
                    &settings,
                    &service_manager,
                    &service_tick,
                    &runtime,
                );
                assert!(matches!(
                    result,
                    CommandResult::Ok(Some(ref value)) if value["pending"] == true
                ));

                tokio::time::timeout(Duration::from_secs(1), async {
                    while !barrier_backend
                        .flush_started
                        .load(std::sync::atomic::Ordering::SeqCst)
                    {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                })
                .await
                .expect("removal reaches teardown barrier");
                assert!(
                    worktree.exists(),
                    "checkout survives while teardown is pending"
                );
                assert!(
                    !marker.exists(),
                    "close hooks wait behind terminal teardown"
                );
                let registration_error = {
                    let mut cx =
                        DaemonWorkspaceCx::new(&workspace_tick, &hook_runner, &hook_monitor);
                    workspace
                        .lock()
                        .add_project(
                            "late claimant".to_string(),
                            worktree
                                .join("packages/late")
                                .to_string_lossy()
                                .into_owned(),
                            false,
                            &global_hooks,
                            WindowId::Main,
                            &mut cx,
                        )
                        .unwrap_err()
                };
                assert!(
                    registration_error.contains("active worktree operation"),
                    "closing lease must reject registration at the teardown barrier"
                );
                release_tx.send(()).expect("release teardown barrier");

                tokio::time::timeout(Duration::from_secs(3), async {
                    loop {
                        if workspace.lock().project("wt1").is_none() {
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                })
                .await
                .expect("background removal completes");
            })
            .await;

        assert_eq!(
            std::fs::read_to_string(&marker).expect("read hook order"),
            "dirty\nclose\n"
        );
        std::fs::remove_dir_all(&repo).ok();
        std::fs::remove_file(&marker).ok();
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
            assert!(
                ok.status.success(),
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&ok.stderr)
            );
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
        assert!(
            Command::new("git")
                .args(["init", "-q", "--bare", origin.to_str().unwrap()])
                .status()
                .expect("git init bare")
                .success()
        );
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
            is_creating: false,
            is_closing: false,
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
        let wt_hooks: Vec<_> = history
            .iter()
            .filter(|h| h.hook_type == "on_worktree_create")
            .collect();
        assert_eq!(
            wt_hooks.len(),
            1,
            "on_worktree_create must fire exactly once, full history: {:?}",
            history.iter().map(|h| h.hook_type).collect::<Vec<_>>()
        );
        assert_eq!(
            workspace
                .project(&new_id)
                .expect("new project")
                .hook_terminals
                .len(),
            1,
            "one live on_worktree_create hook terminal registered on the worktree project"
        );

        // The hook PTY is a SEPARATE terminal from the initial shell (both live in
        // the registry) — proving the hook does NOT consume the initial slot.
        assert!(
            terminals.lock().len() >= 2,
            "initial terminal + hook terminal both in registry"
        );

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
