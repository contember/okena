// The `.expect("BUG: ... must serialize")` sites in this file serialize
// internal DTOs whose Serialize impls cannot fail in practice.
#![allow(clippy::expect_used)]

use crate::remote::bridge::{BridgeMessage, BridgeReceiver, CommandResult, RemoteCommand};
use crate::remote::types::{ActionRequest, ApiFolder, ApiFullscreen, ApiProject, ApiServiceInfo, ApiWindow, StateResponse};
use crate::services::manager::{ServiceManager, ServiceStatus};
use crate::terminal::backend::TerminalBackend;
use crate::views::window::TerminalsRegistry;
use crate::workspace::actions::execute::{ensure_terminal, execute_action};
use crate::workspace::state::{WindowId, Workspace};
use gpui::*;
use okena_core::api::ApiGitStatus;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::watch as tokio_watch;
use uuid::Uuid;

use super::Okena;

/// Parse a wire-format window id into a [`WindowId`].
///
/// `"main"` maps to [`WindowId::Main`]; any other string is parsed as a UUID
/// and, on success, wrapped in [`WindowId::Extra`]. A malformed UUID returns
/// `None` so the caller can reject the action with an "invalid window id"
/// error rather than silently routing it to the wrong window.
pub(crate) fn parse_window_id(s: &str) -> Option<WindowId> {
    if s == "main" {
        Some(WindowId::Main)
    } else {
        Uuid::parse_str(s).ok().map(WindowId::Extra)
    }
}

/// Resolver returning a window's `(WindowId, FocusManager)` for a
/// remote-bridge action.
///
/// The `Option<WindowId>` argument selects the target:
/// - `None` → the focused/active window. In GUI mode the resolver consults
///   `cx.active_window()` and yields the focused window's per-window
///   `WindowId` + `FocusManager` (PRD cri 13), always returning `Some`
///   (falling back to main). In headless mode it returns
///   `Some((WindowId::Main, dormant FocusManager))` since there are no
///   windows to consult.
/// - `Some(id)` → that specific window's `(WindowId, FocusManager)`, or
///   `None` if no such window exists (so the caller can report "window not
///   found"). The `WindowId` lets per-window state mutations (e.g.
///   `SetProjectShowInOverview`) land on the addressed window.
pub(crate) type FocusManagerResolver = Arc<
    dyn Fn(&App, Option<WindowId>) -> Option<(WindowId, Entity<crate::workspace::focus::FocusManager>)>
        + Send
        + Sync,
>;

/// Resolver returning the current set of open OS windows for `GET /v1/state`.
///
/// GUI mode enumerates main + extras (stable order: main first, then
/// `extra_windows` Vec order); headless mode returns a single synthetic main
/// window. Lets the read side report exactly what the user sees.
pub(crate) type WindowsResolver = Arc<dyn Fn(&App) -> Vec<ApiWindow> + Send + Sync>;

/// Dispatches a named GUI command (command palette) into a window for
/// `ActionRequest::InvokeAction`. Args: `(cx, target_window, action_name)`.
/// `None` target → focused window. Returns `Err` if the window or action name
/// can't be resolved, or in headless mode where there is no GUI to dispatch to.
pub(crate) type ActionDispatcher =
    Arc<dyn Fn(&mut App, Option<WindowId>, &str) -> Result<(), String> + Send + Sync>;

/// Shared remote command loop used by both GUI (`Okena`) and headless (`HeadlessApp`).
///
/// Processes commands from the remote API bridge on the GPUI main thread.
/// Callers are responsible for spawning this via `cx.spawn()`.
///
/// `focus_manager_resolver` is consulted per-action so the focused-window
/// scope (PRD user story 27 / slice 05 cri 13) is honored at the moment the
/// action lands, not at loop startup. GUI callers pass a closure that reads
/// `cx.active_window()` and looks the corresponding `WindowView` up on
/// `Okena`; headless callers pass a constant closure returning the synthetic
/// dormant `FocusManager` paired with `WindowId::Main`.
// Bridge loop: each param is a distinct channel/entity dependency.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn remote_command_loop(
    bridge_rx: BridgeReceiver,
    backend: Arc<dyn TerminalBackend>,
    workspace: Entity<Workspace>,
    focus_manager_resolver: FocusManagerResolver,
    windows_resolver: WindowsResolver,
    terminals: TerminalsRegistry,
    state_version: Arc<tokio_watch::Sender<u64>>,
    git_status_tx: Arc<tokio_watch::Sender<HashMap<String, ApiGitStatus>>>,
    service_manager: Entity<ServiceManager>,
    action_dispatcher: ActionDispatcher,
    cx: &mut AsyncApp,
) {
    loop {
        let msg: BridgeMessage = match bridge_rx.recv().await {
            Ok(msg) => msg,
            Err(_) => break,
        };

        let _slow = okena_core::timing::SlowGuard::new("remote_command_loop::iter");

        let result = match msg.command {
            RemoteCommand::Action(action) => {
                match action {
                    ActionRequest::StartService { project_id, service_name } => {
                        cx.update(|cx| {
                            service_manager.update(cx, |sm, cx| {
                                if let Some(path) = sm.project_path(&project_id).cloned() {
                                    sm.start_service(&project_id, &service_name, &path, cx);
                                    CommandResult::Ok(None)
                                } else {
                                    CommandResult::Err(format!("project not found: {}", project_id))
                                }
                            })
                        })
                    }
                    ActionRequest::StopService { project_id, service_name } => {
                        cx.update(|cx| {
                            service_manager.update(cx, |sm, cx| {
                                sm.stop_service(&project_id, &service_name, cx);
                                CommandResult::Ok(None)
                            })
                        })
                    }
                    ActionRequest::RestartService { project_id, service_name } => {
                        cx.update(|cx| {
                            service_manager.update(cx, |sm, cx| {
                                if let Some(path) = sm.project_path(&project_id).cloned() {
                                    sm.restart_service(&project_id, &service_name, &path, cx);
                                    CommandResult::Ok(None)
                                } else {
                                    CommandResult::Err(format!("project not found: {}", project_id))
                                }
                            })
                        })
                    }
                    ActionRequest::StartAllServices { project_id } => {
                        cx.update(|cx| {
                            service_manager.update(cx, |sm, cx| {
                                if let Some(path) = sm.project_path(&project_id).cloned() {
                                    sm.start_all(&project_id, &path, cx);
                                    CommandResult::Ok(None)
                                } else {
                                    CommandResult::Err(format!("project not found: {}", project_id))
                                }
                            })
                        })
                    }
                    ActionRequest::StopAllServices { project_id } => {
                        cx.update(|cx| {
                            service_manager.update(cx, |sm, cx| {
                                sm.stop_all(&project_id, cx);
                                CommandResult::Ok(None)
                            })
                        })
                    }
                    ActionRequest::ReloadServices { project_id } => {
                        cx.update(|cx| {
                            service_manager.update(cx, |sm, cx| {
                                if let Some(path) = sm.project_path(&project_id).cloned() {
                                    sm.reload_project_services(&project_id, &path, cx);
                                    CommandResult::Ok(None)
                                } else {
                                    CommandResult::Err(format!("project not found: {}", project_id))
                                }
                            })
                        })
                    }

                    // ── App-scoped: settings / theme / command palette ────
                    ActionRequest::GetSettings => {
                        cx.update(|cx| super::remote_config::get_settings(cx))
                    }
                    ActionRequest::GetSettingsSchema => {
                        cx.update(|_cx| super::remote_config::get_settings_schema())
                    }
                    ActionRequest::SetSettings { patch } => {
                        cx.update(|cx| super::remote_config::set_settings(cx, patch))
                    }
                    ActionRequest::GetThemes => {
                        cx.update(|cx| super::remote_config::get_themes(cx))
                    }
                    ActionRequest::GetTheme { id } => {
                        cx.update(|cx| super::remote_config::get_theme(cx, id))
                    }
                    ActionRequest::SetTheme { id } => {
                        cx.update(|cx| super::remote_config::set_theme(cx, id))
                    }
                    ActionRequest::SaveCustomTheme { id, config, activate } => {
                        cx.update(|cx| {
                            super::remote_config::save_custom_theme(cx, id, config, activate)
                        })
                    }
                    ActionRequest::ListActions => {
                        cx.update(|_cx| super::remote_config::list_actions())
                    }
                    ActionRequest::InvokeAction { action_name, window } => {
                        cx.update(|cx| {
                            let target = match window.as_deref() {
                                None => None,
                                Some(s) => match parse_window_id(s) {
                                    Some(w) => Some(w),
                                    None => {
                                        return CommandResult::Err(format!(
                                            "invalid window id: {s}"
                                        ));
                                    }
                                },
                            };
                            match action_dispatcher(cx, target, &action_name) {
                                Ok(()) => CommandResult::Ok(None),
                                Err(e) => CommandResult::Err(e),
                            }
                        })
                    }

                    action => {
                        cx.update(|cx| {
                            // Resolve the action's explicit target window (if
                            // any) BEFORE moving `action` into `execute_action`.
                            // An action that carries `window: Some(s)` must land
                            // on THAT window; `None` keeps the focused-window
                            // default. A malformed window id is rejected up
                            // front.
                            //
                            // The parsed `Option<WindowId>` is Copy, so it
                            // outlives the borrow of `action` from
                            // `target_window()`. We capture the malformed string
                            // into an owned `String` for the error path so
                            // nothing borrows `action` past this point.
                            let parsed_target = match action.target_window() {
                                None => Ok(None),
                                Some(s) => match parse_window_id(s) {
                                    Some(wid) => Ok(Some(wid)),
                                    None => Err(s.to_string()),
                                },
                            };
                            let parsed_target = match parsed_target {
                                Ok(t) => t,
                                Err(bad) => {
                                    return CommandResult::Err(format!("invalid window id: {bad}"));
                                }
                            };
                            // Resolve focus_manager + window_id so the targeted
                            // (or focused) window's per-window state is the
                            // target for both focus mutations and per-window
                            // data mutations (PRD cri 13 / CLI fallback).
                            match focus_manager_resolver(cx, parsed_target) {
                                None => CommandResult::Err(format!(
                                    "window not found: {}",
                                    match parsed_target {
                                        Some(WindowId::Main) => "main".to_string(),
                                        Some(WindowId::Extra(uuid)) => uuid.to_string(),
                                        None => String::new(),
                                    }
                                )),
                                Some((window_id, focus_manager)) => {
                                    focus_manager.update(cx, |fm, cx| {
                                        let result = workspace.update(cx, |ws, cx| {
                                            execute_action(action, ws, window_id, fm, &*backend, &terminals, cx)
                                                .into_command_result()
                                        });
                                        cx.notify();
                                        result
                                    })
                                }
                            }
                        })
                    }
                }
            }
            RemoteCommand::GetState => {
                cx.update(|cx| {
                    let ws = workspace.read(cx);
                    let sm = service_manager.read(cx);
                    let sv = *state_version.borrow();
                    let git_statuses = git_status_tx.borrow().clone();
                    let data = ws.data();

                    // Build a lookup map for projects
                    let project_map: std::collections::HashMap<&str, &crate::workspace::state::ProjectData> =
                        data.projects.iter().map(|p| (p.id.as_str(), p)).collect();

                    // Source of truth for runtime visibility (per-window
                    // viewport model).
                    let hidden_project_ids = &data.main_window.hidden_project_ids;

                    // Build ordered projects following project_order + folder expansion
                    let mut projects: Vec<ApiProject> = Vec::new();
                    let mut seen: HashSet<String> = HashSet::new();

                    let build_api_project = |p: &crate::workspace::state::ProjectData| -> ApiProject {
                        let git_status = git_statuses.get(&p.id).cloned();
                        let services: Vec<ApiServiceInfo> = sm.services_for_project(&p.id)
                            .into_iter()
                            .map(|inst| {
                                let (status, exit_code) = match &inst.status {
                                    ServiceStatus::Stopped => ("stopped", None),
                                    ServiceStatus::Starting => ("starting", None),
                                    ServiceStatus::Running => ("running", None),
                                    ServiceStatus::Crashed { exit_code } => ("crashed", *exit_code),
                                    ServiceStatus::Restarting => ("restarting", None),
                                };
                                let kind = match &inst.kind {
                                    crate::services::manager::ServiceKind::Okena => "okena",
                                    crate::services::manager::ServiceKind::DockerCompose { .. } => "docker_compose",
                                };
                                ApiServiceInfo {
                                    name: inst.definition.name.clone(),
                                    status: status.to_string(),
                                    terminal_id: inst.terminal_id.clone(),
                                    ports: inst.detected_ports.clone(),
                                    exit_code,
                                    kind: kind.to_string(),
                                    is_extra: inst.is_extra,
                                }
                            })
                            .collect();
                        ApiProject {
                            id: p.id.clone(),
                            name: p.name.clone(),
                            path: p.path.clone(),
                            show_in_overview: api_project_visibility(&p.id, hidden_project_ids),
                            layout: p.layout.as_ref().map(|l| l.to_api()),
                            terminal_names: p.terminal_names.clone(),
                            git_status,
                            folder_color: p.folder_color,
                            services,
                            worktree_info: p.worktree_info.as_ref().map(|wt| {
                                okena_core::api::ApiWorktreeMetadata {
                                    parent_project_id: wt.parent_project_id.clone(),
                                    color_override: wt.color_override,
                                }
                            }),
                            worktree_ids: p.worktree_ids.clone(),
                        }
                    };

                    for id in &data.project_order {
                        if let Some(folder) = data.folders.iter().find(|f| &f.id == id) {
                            for pid in &folder.project_ids {
                                if seen.insert(pid.clone())
                                    && let Some(p) = project_map.get(pid.as_str()) {
                                        projects.push(build_api_project(p));
                                    }
                            }
                        } else if seen.insert(id.clone())
                            && let Some(p) = project_map.get(id.as_str()) {
                                projects.push(build_api_project(p));
                            }
                    }

                    // Append orphan projects not in any order
                    for p in &data.projects {
                        if seen.insert(p.id.clone()) {
                            projects.push(build_api_project(p));
                        }
                    }

                    // Build folders for response
                    let folders: Vec<ApiFolder> = data.folders.iter().map(|f| {
                        ApiFolder {
                            id: f.id.clone(),
                            name: f.name.clone(),
                            project_ids: f.project_ids.clone(),
                            folder_color: f.folder_color,
                        }
                    }).collect();

                    // Enumerate the open OS windows (main first, then extras in
                    // persistence order) so the client sees exactly what the
                    // user sees per-window. The back-compat flat fields
                    // (`focused_project_id`, `fullscreen_terminal`) are derived
                    // from the ACTIVE window so old clients still get a sensible
                    // focused project / fullscreen.
                    let windows = windows_resolver(cx);
                    let focused_project_id = windows
                        .iter()
                        .find(|w| w.active)
                        .and_then(|w| w.focused_project_id.clone());
                    let fullscreen: Option<ApiFullscreen> = windows
                        .iter()
                        .find(|w| w.active)
                        .and_then(|w| w.fullscreen.clone());

                    let resp = StateResponse {
                        state_version: sv,
                        projects,
                        focused_project_id,
                        fullscreen_terminal: fullscreen,
                        project_order: data.project_order.clone(),
                        folders,
                        windows,
                    };

                    CommandResult::Ok(Some(serde_json::to_value(resp).expect("BUG: StateResponse must serialize")))
                })
            }
            RemoteCommand::GetTerminalSizes { terminal_ids } => {
                cx.update(|_cx| {
                    let terms = terminals.lock();
                    let mut sizes = std::collections::HashMap::new();
                    for id in &terminal_ids {
                        if let Some(term) = terms.get(id) {
                            let s = term.resize_state.lock().size;
                            sizes.insert(id.clone(), (s.cols, s.rows));
                        }
                    }
                    let val = serde_json::to_value(sizes).expect("BUG: sizes must serialize");
                    CommandResult::Ok(Some(val))
                })
            }
            RemoteCommand::RenderSnapshot { terminal_id } => {
                cx.update(|cx| {
                    let ws = workspace.read(cx);
                    match ensure_terminal(&terminal_id, &terminals, &*backend, ws) {
                        Some(term) => {
                            let snapshot = term.render_snapshot();
                            CommandResult::OkBytes(snapshot)
                        }
                        None => CommandResult::Err(format!("terminal not found: {}", terminal_id)),
                    }
                })
            }
            RemoteCommand::PasteImage { terminal_id, path } => {
                cx.update(|cx| {
                    let ws = workspace.read(cx);
                    match ensure_terminal(&terminal_id, &terminals, &*backend, ws) {
                        Some(term) => {
                            // Bracketed paste of the server-local image path —
                            // same as a local image paste, so the focused TUI's
                            // own paste handler attaches it.
                            term.send_paste(&path);
                            CommandResult::Ok(Some(serde_json::json!({ "path": path })))
                        }
                        None => CommandResult::Err(format!("terminal not found: {}", terminal_id)),
                    }
                })
            }
        };

        if let Some(reply) = msg.reply {
            let _ = reply.send(result);
        }
    }
}

impl Okena {
    /// Process commands from the remote API bridge.
    /// Thin wrapper that spawns the shared `remote_command_loop`.
    ///
    /// Builds a focus-manager resolver that, per-action, asks the live `Okena`
    /// entity which OS window currently has focus and returns that window's
    /// `(WindowId, FocusManager)` (PRD cri 13). When the `Okena` weak handle
    /// has been dropped (loop racing app shutdown) the resolver falls back to
    /// `(WindowId::Main, captured main FocusManager)`.
    pub(super) fn start_remote_command_loop(
        &mut self,
        bridge_rx: BridgeReceiver,
        backend: Arc<dyn TerminalBackend>,
        cx: &mut Context<Self>,
    ) {
        let workspace = self.workspace.clone();
        let main_focus_manager = self.main_window.read(cx).focus_manager();
        let okena_weak = cx.entity().downgrade();
        let focus_okena_weak = okena_weak.clone();
        let focus_manager_resolver: FocusManagerResolver =
            Arc::new(move |cx: &App, target: Option<WindowId>| {
                match focus_okena_weak.upgrade() {
                    Some(okena) => okena.read(cx).focus_manager_for_window(cx, target),
                    // Drop-race fallback (loop racing app shutdown): the live
                    // Okena is gone, so honor only the focused/main default.
                    None => match target {
                        None | Some(WindowId::Main) => {
                            Some((WindowId::Main, main_focus_manager.clone()))
                        }
                        Some(WindowId::Extra(_)) => None,
                    },
                }
            });
        let windows_okena_weak = okena_weak.clone();
        let windows_resolver: WindowsResolver = Arc::new(move |cx: &App| {
            windows_okena_weak
                .upgrade()
                .map(|okena| okena.read(cx).build_api_windows(cx))
                .unwrap_or_default()
        });
        // Command-palette dispatch: resolve the target window and the named
        // GUI action, then dispatch it into that window.
        let dispatch_okena_weak = okena_weak;
        let action_dispatcher: ActionDispatcher =
            Arc::new(move |cx: &mut App, target: Option<WindowId>, name: &str| {
                let okena = dispatch_okena_weak
                    .upgrade()
                    .ok_or_else(|| "app is shutting down".to_string())?;
                let handle = okena
                    .read(cx)
                    .window_handle_for(cx, target)
                    .ok_or_else(|| "window not found".to_string())?;
                let descriptions = crate::keybindings::get_action_descriptions();
                let desc = descriptions
                    .get(name)
                    .ok_or_else(|| format!("unknown command: {name}"))?;
                let action = (desc.factory)();
                handle
                    .update(cx, |_view, window, cx| window.dispatch_action(action, cx))
                    .map_err(|e| format!("dispatch failed: {e}"))
            });
        let terminals = self.terminals.clone();
        let state_version = self.state_version.clone();
        let git_status_tx = self.git_status_tx.clone();
        let service_manager = self.service_manager.clone();

        cx.spawn(async move |_this: WeakEntity<Okena>, cx: &mut AsyncApp| {
            remote_command_loop(
                bridge_rx, backend, workspace, focus_manager_resolver, windows_resolver, terminals,
                state_version, git_status_tx, service_manager, action_dispatcher, cx,
            ).await;
        })
        .detach();
    }
}

/// Pure visibility projection for the remote `ApiProject.show_in_overview`
/// wire flag. A project is "shown in overview" iff it is absent from the
/// per-window hidden set (today: `main_window.hidden_project_ids`).
fn api_project_visibility(project_id: &str, hidden_project_ids: &HashSet<String>) -> bool {
    !hidden_project_ids.contains(project_id)
}

#[cfg(test)]
mod api_project_visibility_tests {
    use super::api_project_visibility;
    use std::collections::HashSet;

    /// Regression: the wire-format visibility flag must derive from the
    /// per-window hidden set. With the legacy
    /// `ProjectData.show_in_overview` field removed entirely, this test
    /// pins the post-deletion contract.
    #[test]
    fn api_project_visibility_reads_from_hidden_set() {
        let hidden: HashSet<String> = ["p1".to_string()].into_iter().collect();
        assert!(
            !api_project_visibility("p1", &hidden),
            "membership in hidden set must read as not-visible",
        );
        assert!(
            api_project_visibility("p2", &hidden),
            "absent from hidden set must read as visible",
        );
    }

    #[test]
    fn api_project_visibility_empty_hidden_set_is_visible() {
        let hidden: HashSet<String> = HashSet::new();
        assert!(api_project_visibility("p1", &hidden));
    }
}

#[cfg(test)]
mod parse_window_id_tests {
    use super::parse_window_id;
    use crate::workspace::state::WindowId;
    use uuid::Uuid;

    #[test]
    fn main_maps_to_main_variant() {
        assert_eq!(parse_window_id("main"), Some(WindowId::Main));
    }

    #[test]
    fn valid_uuid_maps_to_extra() {
        let id = Uuid::new_v4();
        assert_eq!(parse_window_id(&id.to_string()), Some(WindowId::Extra(id)));
    }

    #[test]
    fn garbage_returns_none() {
        assert_eq!(parse_window_id("garbage"), None);
        assert_eq!(parse_window_id(""), None);
        // A near-miss UUID (one char short) is still rejected.
        assert_eq!(parse_window_id("550e8400-e29b-41d4-a716-44665544000"), None);
    }
}
