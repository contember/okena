use super::{Okena, WindowId, WindowView};
use axum::{
    Json, Router,
    extract::{State, WebSocketUpgrade},
    routing::{get, post},
};
use gpui::{AppContext, Context, Entity, TestAppContext, Window};
use okena_core::{
    api::{ActionRequest, StateResponse},
    attention::{AttentionEpisode, AttentionKind, AttentionSource},
};
use okena_remote_client::manager::RemoteConnectionManager;
use okena_transport::client::{ConnectionStatus, RemoteConnectionConfig};
use okena_workspace::state::{Workspace, WorkspaceData};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, atomic::AtomicBool},
    time::{Duration, Instant},
};

#[derive(Clone)]
struct ServerState {
    snapshot: StateResponse,
    actions: Arc<parking_lot::Mutex<Vec<ActionRequest>>>,
}

struct Loopback {
    runtime: tokio::runtime::Runtime,
    config: RemoteConnectionConfig,
    actions: Arc<parking_lot::Mutex<Vec<ActionRequest>>>,
}

impl Loopback {
    fn new(episode: &AttentionEpisode) -> Self {
        let snapshot: StateResponse = serde_json::from_value(serde_json::json!({
            "state_version": 1, "focused_project_id": null, "fullscreen_terminal": null,
            "work_overview": {"missions": [], "attention": [episode], "lost_transitions": 0, "conversations": [], "terminal_bindings": []},
            "projects": [{"id": "p", "name": "Fixture", "path": "/nonexistent-reveal-fixture", "show_in_overview": true,
                "terminal_names": {}, "layout": {"type": "tabs", "active_tab": 0, "children": [
                    {"type": "terminal", "terminal_id": "other", "minimized": false, "detached": false, "cols": 80, "rows": 24},
                    {"type": "terminal", "terminal_id": "t", "minimized": false, "detached": false, "cols": 80, "rows": 24}
                ]}}]
        })).unwrap();
        let actions = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let state = ServerState {
            snapshot,
            actions: actions.clone(),
        };
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let listener = runtime
            .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        let router = Router::new()
            .route("/health", get(|| async { "ok" }))
            .route("/v1/state", get(|State(state): State<ServerState>| async move { Json(state.snapshot) }))
            .route("/v1/actions", post(|State(state): State<ServerState>, Json(action): Json<ActionRequest>| async move {
                state.actions.lock().push(action);
                Json(serde_json::json!({"ok": true}))
            }))
            .route("/v1/stream", get(|ws: WebSocketUpgrade| async move {
                ws.on_upgrade(|mut socket| async move {
                    if socket.recv().await.is_none() { return; }
                    if socket.send(axum::extract::ws::Message::Text("{\"type\":\"auth_ok\"}".into())).await.is_err() { return; }
                    while socket.recv().await.is_some() {}
                })
            }))
            .with_state(state);
        runtime.spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Self {
            runtime,
            actions,
            config: RemoteConnectionConfig {
                id: "fixture".into(),
                name: "Fixture".into(),
                host: "127.0.0.1".into(),
                port,
                saved_token: Some("fixture-token".into()),
                token_obtained_at: None,
                tls: false,
                pinned_cert_sha256: None,
                local_endpoint: None,
            },
        }
    }
}

fn pump_until(cx: &mut TestAppContext, mut done: impl FnMut(&mut TestAppContext) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        cx.run_until_parked();
        if done(cx) {
            return;
        }
        assert!(Instant::now() < deadline, "coordinator fixture timed out");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn init(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    cx.update(|cx| {
        gpui_component::init(cx);
        let settings = cx.new(|_| crate::settings::SettingsState::new(Default::default()));
        cx.set_global(crate::settings::GlobalSettings(settings));
        let theme =
            cx.new(|_| crate::theme::AppTheme::new(okena_core::theme::ThemeMode::Dark, true));
        cx.set_global(crate::theme::GlobalTheme(theme));
        cx.set_global(okena_ui::theme::GlobalThemeProvider(|_| {
            okena_ui::theme::DARK_THEME
        }));
        cx.set_global(okena_extensions::ExtensionSettingsStore::new(
            |_, _| None,
            |_, _, _| {},
        ));
        cx.set_global(okena_workspace::toast::ToastManager::new());
        cx.set_global(okena_extensions::ExtensionRegistry::new());
    });
}

fn flush_action_queue(
    manager: &Entity<RemoteConnectionManager>,
    connection_id: &str,
    cx: &mut TestAppContext,
) {
    cx.run_until_parked();
    let response = manager.update(cx, |manager, cx| {
        manager.send_action_with_result(connection_id, ActionRequest::GetSettings, cx)
    });
    let completed = Arc::new(parking_lot::Mutex::new(None));
    let result = completed.clone();
    cx.executor()
        .spawn(async move {
            *result.lock() = Some(response.await);
        })
        .detach();
    pump_until(cx, |_| completed.lock().is_some());
    assert!(
        completed.lock().take().unwrap().is_ok(),
        "FIFO barrier failed"
    );
}

fn coordinator(
    workspace: Entity<Workspace>,
    window: &mut Window,
    cx: &mut Context<Okena>,
) -> Okena {
    cx.set_global(crate::workspace::state::GlobalWorkspace(workspace.clone()));
    let terminals = Arc::new(parking_lot::Mutex::new(HashMap::new()));
    let remote_manager = cx.new(|cx| RemoteConnectionManager::new(terminals.clone(), cx));
    let main_window = cx.new(|cx| {
        WindowView::new(
            WindowId::Main,
            workspace.clone(),
            terminals.clone(),
            window,
            cx,
        )
    });
    main_window.update(cx, |view, cx| {
        view.set_remote_manager(remote_manager.clone(), cx)
    });
    Okena {
        main_window,
        main_window_handle: window.window_handle(),
        extra_windows: HashMap::new(),
        extra_window_handles: HashMap::new(),
        workspace,
        terminals,
        scrollback_visible_projects: parking_lot::Mutex::new(None),
        opened_detached_windows: HashSet::new(),
        detached_window_handles: HashMap::new(),
        remote_manager,
        last_settings_sent: serde_json::Value::Null,
        terminal_activity_repaints: Default::default(),
        sidebar_activity_repaints: Default::default(),
        notification_jump_tx: async_channel::unbounded().0,
        spawned_daemon: None,
        preserve_daemon_on_quit: true,
        recovering: Arc::new(AtomicBool::new(false)),
        quitting: Arc::new(AtomicBool::new(false)),
        pending_extra_forgets: Default::default(),
    }
}

#[gpui::test]
fn coordinator_reveals_hidden_minimized_inactive_terminal_then_acknowledges(
    cx: &mut TestAppContext,
) {
    reveal_case(cx, false, true);
}

#[gpui::test]
fn coordinator_reveals_detached_terminal_without_reattaching(cx: &mut TestAppContext) {
    reveal_case(cx, true, true);
}

#[gpui::test]
fn coordinator_reveals_cleared_completion_then_acknowledges(cx: &mut TestAppContext) {
    reveal_case(cx, false, false);
}

fn reveal_case(cx: &mut TestAppContext, detached: bool, status_available: bool) {
    init(cx);
    let episode = AttentionEpisode {
        id: "00000000-0000-0000-0000-000000000001".into(),
        revision: 7,
        source: AttentionSource {
            project_id: "p".into(),
            terminal_id: "t".into(),
            attachment_id: "attachment".into(),
            generation: 3,
            conversation: None,
        },
        kind: AttentionKind::Completion,
        summary: "Done".into(),
        created_at: 1,
        updated_at: 1,
        available: status_available,
        terminal_available: true,
        read: false,
    };
    let server = Loopback::new(&episode);
    let mut other_server = Loopback::new(&episode);
    other_server.config.id = "other-owner".into();
    let workspace = cx.new(|_| Workspace::new(WorkspaceData::empty()));
    let root = cx.add_window(|window, cx| coordinator(workspace.clone(), window, cx));
    gpui::VisualTestContext::from_window(root.into(), cx).deactivate_window();
    cx.run_until_parked();
    let manager = root
        .read_with(cx, |root, _| root.remote_manager.clone())
        .unwrap();
    manager.update(cx, |manager, cx| {
        manager.add_connection(server.config.clone(), cx).unwrap()
    });
    manager.update(cx, |manager, cx| {
        manager
            .add_connection(other_server.config.clone(), cx)
            .unwrap()
    });
    pump_until(cx, |cx| {
        manager.read_with(cx, |manager, _| {
            manager
                .connections()
                .iter()
                .all(|(_, status, _)| matches!(status, ConnectionStatus::Connected))
        }) && workspace.read_with(cx, |workspace, _| {
            workspace.project("remote:fixture:p").is_some()
        })
    });
    workspace.update(cx, |workspace, cx| {
        workspace
            .data
            .window_mut(WindowId::Main)
            .unwrap()
            .hidden_project_ids
            .insert("remote:fixture:p".into());
        let project = workspace
            .data
            .projects
            .iter_mut()
            .find(|project| project.id == "remote:fixture:p")
            .unwrap();
        if let Some(okena_workspace::state::LayoutNode::Tabs {
            children,
            active_tab,
            ..
        }) = &mut project.layout
        {
            *active_tab = 0;
            if let okena_workspace::state::LayoutNode::Terminal {
                minimized,
                detached: is_detached,
                ..
            } = &mut children[1]
            {
                *minimized = true;
                *is_detached = detached;
            }
        } else {
            panic!("expected tabs");
        }
        workspace.notify_data(cx);
    });
    assert!(
        !server
            .actions
            .lock()
            .iter()
            .any(|action| matches!(action, ActionRequest::AcknowledgeAttention { .. }))
    );
    root.entity(cx).unwrap().update(cx, |root, cx| {
        root.reveal_work_terminal(
            WindowId::Main,
            "fixture",
            "p",
            "missing",
            Some(episode.clone()),
            cx,
        )
    });
    cx.update_window(root.into(), |_, window, cx| {
        window.refresh();
        window.draw(cx).clear(cx);
    })
    .unwrap();
    cx.run_until_parked();
    flush_action_queue(&manager, "fixture", cx);
    flush_action_queue(&manager, "other-owner", cx);
    assert!(
        !server
            .actions
            .lock()
            .iter()
            .any(|action| matches!(action, ActionRequest::AcknowledgeAttention { .. }))
    );
    root.entity(cx).unwrap().update(cx, |root, cx| {
        root.reveal_work_terminal(WindowId::Main, "fixture", "p", "t", Some(episode), cx)
    });
    let target = root
        .read_with(cx, |root, _| {
            root.detached_window_handles
                .get("remote:fixture:t")
                .copied()
                .unwrap_or(root.main_window_handle)
        })
        .unwrap();
    if !detached {
        let focus = root
            .read_with(cx, |root, cx| root.main_window.read(cx).focus_manager())
            .unwrap();
        workspace.read_with(cx, |workspace, cx| {
            let focus = focus.read(cx);
            assert!(
                workspace
                    .visible_projects(
                        WindowId::Main,
                        focus.focused_project_id(),
                        focus.is_focus_individual()
                    )
                    .iter()
                    .any(|project| project.id == "remote:fixture:p")
            );
            let layout = workspace
                .project("remote:fixture:p")
                .unwrap()
                .layout
                .as_ref()
                .unwrap();
            let okena_workspace::state::LayoutNode::Tabs {
                active_tab,
                children,
                ..
            } = layout
            else {
                panic!("expected tabs");
            };
            assert_eq!(*active_tab, 1);
            assert!(matches!(
                &children[1],
                okena_workspace::state::LayoutNode::Terminal {
                    minimized: false,
                    ..
                }
            ));
        });
    }
    pump_until(cx, |cx| {
        cx.update_window(target, |_, window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        })
        .unwrap();
        server.actions.lock().iter().any(|action| matches!(action, ActionRequest::AcknowledgeAttention { episode_id, revision: 7, dismiss: false } if episode_id == "00000000-0000-0000-0000-000000000001"))
    });
    flush_action_queue(&manager, "fixture", cx);
    flush_action_queue(&manager, "other-owner", cx);
    assert_eq!(
        server
            .actions
            .lock()
            .iter()
            .filter(|action| matches!(action, ActionRequest::AcknowledgeAttention { .. }))
            .count(),
        1
    );
    assert_eq!(
        workspace.read_with(cx, |workspace, _| workspace
            .is_terminal_detached("remote:fixture:t")),
        detached
    );
    assert!(
        !other_server
            .actions
            .lock()
            .iter()
            .any(|action| matches!(action, ActionRequest::AcknowledgeAttention { .. }))
    );
    drop(server.runtime);
}
