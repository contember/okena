// The `.expect("BUG: ... must serialize")` sites in this file serialize
// internal DTOs whose Serialize impls cannot fail in practice.
#![allow(clippy::expect_used)]

use crate::remote::bridge::{BridgeMessage, BridgeReceiver, CommandResult, RemoteCommand};
use crate::remote::types::{ActionRequest, ApiServiceInfo, ApiWindow};
use crate::services::manager::ServiceManager;
use crate::terminal::backend::TerminalBackend;
use crate::views::window::TerminalsRegistry;
use crate::workspace::actions::execute::{ensure_terminal, execute_action};
use crate::workspace::hook_monitor::HookMonitor;
use crate::workspace::state::{WindowId, Workspace};
use gpui::*;
use okena_core::api::ApiGitStatus;
use okena_workspace::actions::soft_close::{
    begin_soft_close_flow, close_now_flow, probe_busy, undo_soft_close_flow, SoftCloseDeadlines,
};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::watch as tokio_watch;
use uuid::Uuid;

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
    hook_monitor: HookMonitor,
    deadlines: SoftCloseDeadlines,
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
                                sm.start_service_action(&project_id, &service_name, cx)
                            })
                        })
                    }
                    ActionRequest::StopService { project_id, service_name } => {
                        cx.update(|cx| {
                            service_manager.update(cx, |sm, cx| {
                                sm.stop_service_action(&project_id, &service_name, cx)
                            })
                        })
                    }
                    ActionRequest::RestartService { project_id, service_name } => {
                        cx.update(|cx| {
                            service_manager.update(cx, |sm, cx| {
                                sm.restart_service_action(&project_id, &service_name, cx)
                            })
                        })
                    }
                    ActionRequest::StartAllServices { project_id } => {
                        cx.update(|cx| {
                            service_manager.update(cx, |sm, cx| {
                                sm.start_all_action(&project_id, cx)
                            })
                        })
                    }
                    ActionRequest::StopAllServices { project_id } => {
                        cx.update(|cx| {
                            service_manager.update(cx, |sm, cx| {
                                sm.stop_all_action(&project_id, cx)
                            })
                        })
                    }
                    ActionRequest::ReloadServices { project_id } => {
                        cx.update(|cx| {
                            service_manager.update(cx, |sm, cx| {
                                sm.reload_services_action(&project_id, cx)
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

                    // ── Soft-close: undo (restore the ejected pane) ──────────
                    ActionRequest::UndoSoftClose { terminal_id } => {
                        // Resolve the dormant main FocusManager (headless has a
                        // single synthetic window). Then clear the deadline +
                        // restore the pane through the shared flow.
                        cx.update(|cx| {
                            match focus_manager_resolver(cx, None) {
                                None => CommandResult::Err("window not found: main".to_string()),
                                Some((_window_id, focus_manager)) => {
                                    focus_manager.update(cx, |fm, cx| {
                                        workspace.update(cx, |ws, cx| {
                                            undo_soft_close_flow(
                                                &deadlines, ws, fm, &terminals, &terminal_id, cx,
                                            );
                                        });
                                        cx.notify();
                                    });
                                    CommandResult::Ok(None)
                                }
                            }
                        })
                    }

                    // ── Soft-close: finalize now ("Close now") ───────────────
                    ActionRequest::CloseTerminalNow { terminal_id } => {
                        cx.update(|cx| {
                            workspace.update(cx, |ws, cx| {
                                close_now_flow(
                                    &deadlines, ws, &*backend, &terminals, &terminal_id, cx,
                                );
                            });
                            CommandResult::Ok(None)
                        })
                    }

                    // ── Close terminal: grace-aware soft close ───────────────
                    // Mirrors the daemon-core loop: a busy terminal is ejected
                    // from the layout but its PTY is kept alive for the grace
                    // period (the finalizer tick kills it on expiry); idle
                    // terminals and `grace == 0` keep the immediate close.
                    ActionRequest::CloseTerminal { project_id, terminal_id } => {
                        let grace = cx.update(|cx| crate::settings::settings(cx).terminal_close_grace_secs);

                        if grace == 0 {
                            // Feature off → immediate close (unchanged behavior).
                            cx.update(|cx| {
                                let app_settings = crate::settings::settings(cx);
                                match focus_manager_resolver(cx, None) {
                                    None => CommandResult::Err("window not found: main".to_string()),
                                    Some((window_id, focus_manager)) => {
                                        focus_manager.update(cx, |fm, cx| {
                                            let result = workspace.update(cx, |ws, cx| {
                                                execute_action(
                                                    ActionRequest::CloseTerminal { project_id, terminal_id },
                                                    ws, window_id, fm, &*backend, &terminals, &app_settings, cx,
                                                )
                                                .into_command_result()
                                            });
                                            cx.notify();
                                            result
                                        })
                                    }
                                }
                            })
                        } else {
                            // Probe busy-ness OFF the gpui thread (forks
                            // tmux/lsof/pgrep). Hold NO gpui lock across it.
                            let (busy, command) = smol::unblock({
                                let backend = backend.clone();
                                let tid = terminal_id.clone();
                                move || probe_busy(&*backend, &tid)
                            })
                            .await;

                            cx.update(|cx| {
                                let app_settings = crate::settings::settings(cx);
                                match focus_manager_resolver(cx, None) {
                                    None => CommandResult::Err("window not found: main".to_string()),
                                    Some((window_id, focus_manager)) => {
                                        focus_manager.update(cx, |fm, cx| {
                                            // Try the soft close first when busy;
                                            // `None` means the terminal wasn't in
                                            // the layout, so fall through to the
                                            // immediate close (same as idle).
                                            if busy {
                                                let toast = workspace.update(cx, |ws, cx| {
                                                    begin_soft_close_flow(
                                                        &deadlines, ws, fm, &terminals,
                                                        &project_id, &terminal_id, grace, command, cx,
                                                    )
                                                });
                                                if let Some(toast) = toast {
                                                    hook_monitor.push_toast(toast);
                                                    cx.notify();
                                                    return CommandResult::Ok(None);
                                                }
                                            }
                                            // Idle, or busy-but-not-in-layout →
                                            // immediate close.
                                            let result = workspace.update(cx, |ws, cx| {
                                                execute_action(
                                                    ActionRequest::CloseTerminal { project_id, terminal_id },
                                                    ws, window_id, fm, &*backend, &terminals, &app_settings, cx,
                                                )
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
                            // Snapshot app settings to thread into the gpui-free
                            // `execute_action` (hooks / worktree template / default
                            // shell). Read here on the gpui thread before the
                            // nested entity updates borrow `cx`.
                            let app_settings = crate::settings::settings(cx);
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
                                            execute_action(action, ws, window_id, fm, &*backend, &terminals, &app_settings, cx)
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

                    // Build terminal size map from the registry
                    let size_map: HashMap<String, (u16, u16)> = {
                        let registry = terminals.lock();
                        registry.iter().map(|(id, term)| {
                            let size = term.resize_state.lock().size;
                            (id.clone(), (size.cols, size.rows))
                        }).collect()
                    };

                    // Source of truth for runtime visibility (per-window
                    // viewport model).
                    let hidden_project_ids = &data.main_window.hidden_project_ids;

                    // Pre-build the per-project wire service lists from THIS
                    // caller's `ServiceManager` (keeps `okena-services` out of the
                    // shared `okena-app-core` builder). The
                    // `ServiceInstance -> ApiServiceInfo` mapping is
                    // `ServiceInstance::to_api`, shared with the daemon loop.
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

                    // Enumerate the open OS windows (main first, then extras in
                    // persistence order) so the client sees exactly what the
                    // user sees per-window. The back-compat flat fields
                    // (`focused_project_id`, `fullscreen_terminal`) are derived
                    // from the ACTIVE window inside `build_state_response`.
                    let windows = windows_resolver(cx);

                    // Shared projection: ordered projects + folders + flat
                    // back-compat fields → `StateResponse` (identical to the
                    // daemon loop).
                    let resp = okena_app_core::remote_snapshot::build_state_response(
                        sv,
                        data,
                        &git_statuses,
                        &services_by_project,
                        hidden_project_ids,
                        &size_map,
                        windows,
                    );

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

#[cfg(test)]
mod api_project_visibility_tests {
    // The visibility projection now lives in the shared `okena-app-core`
    // snapshot builder (`build_state_response` uses it internally); this test
    // pins its contract from the GUI-crate side.
    use okena_app_core::remote_snapshot::api_project_visibility;
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
