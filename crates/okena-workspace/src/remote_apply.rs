//! Pure reconciliation of remote connection state into `WorkspaceData`.
//!
//! This is the GPUI-free core of the remote-projects sync that used to live in
//! the desktop `RootView`. It materializes prefixed remote projects/folders into
//! the workspace data, merges locally-preserved visual layout state, prunes
//! stale remote entries, and computes which terminals should receive focus.
//!
//! The view layer is responsible only for snapshotting the connection data out
//! of the `RemoteConnectionManager` entity (to build `RemoteSnapshot`s) and for
//! applying the returned focus targets via `set_focused_terminal`. All of the
//! reconciliation logic is here so it can be unit-tested without GPUI.

use std::collections::{HashMap, HashSet};

use okena_core::api::StateResponse;
use okena_core::client::RemoteConnectionConfig;
use okena_layout::LayoutNode;
use okena_state::{FolderData, HooksConfig, ProjectData, WindowId, WorkspaceData, WorktreeMetadata};

use crate::remote_sync::RemoteSyncState;

/// Owned snapshot of a single remote connection, built by the view from the
/// `RemoteConnectionManager` so the pure core never touches a GPUI entity.
#[derive(Clone)]
pub struct RemoteSnapshot {
    pub config: RemoteConnectionConfig,
    pub state: Option<StateResponse>,
}

/// A terminal that should be focused after the sync, identified by the project
/// it lives in and the layout path to reach it.
#[derive(Clone, Debug, PartialEq)]
pub struct RemoteFocusTarget {
    pub project_id: String,
    pub layout_path: Vec<usize>,
}

/// Result of applying a set of remote snapshots to the workspace data.
#[derive(Clone, Debug, Default)]
pub struct RemoteSyncOutcome {
    /// Terminals to focus (for projects that had a pending CreateTerminal and
    /// whose layout grew a new terminal during this sync).
    pub focus_targets: Vec<RemoteFocusTarget>,
}

/// Apply remote connection snapshots to `data`, reconciling materialized remote
/// projects, folders, and project order, while preserving local visual state and
/// pruning stale remote entries.
///
/// `remote_sync` is updated in place (transient per-project snapshots written,
/// pending focus drained). The returned outcome carries the focus targets the
/// caller should apply via `set_focused_terminal`.
///
/// This function performs NO GPUI work and is fully unit-testable.
pub fn apply_remote_snapshot(
    data: &mut WorkspaceData,
    remote_sync: &mut RemoteSyncState,
    snapshots: &[RemoteSnapshot],
    window_id: WindowId,
) -> RemoteSyncOutcome {
    let mut expected_remote_ids: HashSet<String> = HashSet::new();
    let active_conn_ids: HashSet<String> = snapshots.iter()
        .map(|s| s.config.id.clone())
        .collect();

    for snap in snapshots {
        let conn_id = &snap.config.id;

        if let Some(ref state) = snap.state {
            // Build the server folder lookup
            let server_folder_map: HashMap<&str, &okena_core::api::ApiFolder> =
                state.folders.iter().map(|f| (f.id.as_str(), f)).collect();

            // Build prefixed project_order and folder entries that mirror the server structure
            let mut remote_order: Vec<String> = Vec::new();
            let mut remote_folders: Vec<FolderData> = Vec::new();

            if !state.project_order.is_empty() {
                for order_id in &state.project_order {
                    if let Some(sf) = server_folder_map.get(order_id.as_str()) {
                        // This is a folder — create a prefixed FolderData
                        let prefixed_folder_id = format!("remote:{}:{}", conn_id, sf.id);
                        let prefixed_project_ids: Vec<String> = sf.project_ids.iter()
                            .map(|pid| format!("remote:{}:{}", conn_id, pid))
                            .collect();
                        remote_folders.push(FolderData {
                            id: prefixed_folder_id.clone(),
                            name: sf.name.clone(),
                            project_ids: prefixed_project_ids,
                            folder_color: sf.folder_color,
                        });
                        remote_order.push(prefixed_folder_id);
                    } else {
                        // This is a top-level project
                        remote_order.push(format!("remote:{}:{}", conn_id, order_id));
                    }
                }
            } else {
                // Old server without project_order: put all projects as top-level
                for api_project in &state.projects {
                    remote_order.push(format!("remote:{}:{}", conn_id, api_project.id));
                }
            };

            for api_project in &state.projects {
                let prefixed_id = format!("remote:{}:{}", conn_id, api_project.id);
                expected_remote_ids.insert(prefixed_id.clone());

                let layout = api_project.layout.as_ref().map(|l| {
                    LayoutNode::from_api_prefixed(l, &format!("remote:{}", conn_id))
                });

                let terminal_names: HashMap<String, String> = api_project.terminal_names.iter()
                    .map(|(k, v)| (format!("remote:{}:{}", conn_id, k), v.clone()))
                    .collect();

                let project_color = api_project.folder_color;
                let conn_id_owned = conn_id.clone();

                // Build remote services with prefixed terminal IDs
                let remote_services: Vec<okena_core::api::ApiServiceInfo> = api_project.services.iter().map(|s| {
                    let mut svc = s.clone();
                    svc.terminal_id = s.terminal_id.as_ref()
                        .map(|tid| format!("remote:{}:{}", conn_id, tid));
                    svc
                }).collect();
                let remote_host = Some(snap.config.host.clone());
                let remote_git_status = api_project.git_status.clone();

                if let Some(existing) = data.projects.iter_mut().find(|p| p.id == prefixed_id) {
                    existing.name = api_project.name.clone();
                    existing.path = api_project.path.clone();
                    // Merge server layout with locally-preserved visual state
                    // (split sizes, minimized, detached, active_tab).
                    existing.layout = match (&existing.layout, &layout) {
                        (Some(local), Some(server)) => {
                            Some(LayoutNode::merge_visual_state(server, local))
                        }
                        _ => layout,
                    };
                    existing.terminal_names = terminal_names;
                    existing.folder_color = project_color;
                    existing.worktree_info = api_project.worktree_info.as_ref().map(|wt| {
                        WorktreeMetadata {
                            parent_project_id: format!("remote:{}:{}", conn_id, wt.parent_project_id),
                            color_override: wt.color_override,
                            main_repo_path: String::new(),
                            worktree_path: String::new(),
                            branch_name: String::new(),
                        }
                    });
                    existing.worktree_ids = api_project.worktree_ids.iter()
                        .map(|id| format!("remote:{}:{}", conn_id, id))
                        .collect();
                    // Don't overwrite show_in_overview — it's client-side state
                    // (the user may have toggled visibility locally).
                } else {
                    let worktree_info = api_project.worktree_info.as_ref().map(|wt| {
                        WorktreeMetadata {
                            parent_project_id: format!("remote:{}:{}", conn_id, wt.parent_project_id),
                            color_override: wt.color_override,
                            main_repo_path: String::new(),
                            worktree_path: String::new(),
                            branch_name: String::new(),
                        }
                    });
                    let worktree_ids: Vec<String> = api_project.worktree_ids.iter()
                        .map(|id| format!("remote:{}:{}", conn_id, id))
                        .collect();
                    apply_initial_remote_project_visibility(
                        data,
                        remote_sync,
                        conn_id,
                        &prefixed_id,
                        &api_project.name,
                        &api_project.path,
                        api_project.show_in_overview,
                    );
                    data.projects.push(ProjectData {
                        id: prefixed_id.clone(),
                        name: api_project.name.clone(),
                        path: api_project.path.clone(),
                        layout,
                        terminal_names,
                        hidden_terminals: HashMap::new(),
                        worktree_info,
                        worktree_ids,
                        folder_color: project_color,
                        hooks: HooksConfig::default(),
                        is_remote: true,
                        connection_id: Some(conn_id_owned),
                        service_terminals: HashMap::new(),
                        default_shell: None,
                        hook_terminals: HashMap::new(),
                    });
                }
                // Update the transient remote snapshot regardless of create/update path.
                let snapshot = remote_sync.snapshot_mut(&prefixed_id);
                snapshot.services = remote_services;
                snapshot.host = remote_host;
                snapshot.git_status = remote_git_status;
            }

            // Sync remote folders and project_order into workspace
            let remote_prefix = format!("remote:{}:", conn_id);
            // Scrub per-window state for remote folders that disappeared this sync.
            let next_remote_folder_ids: HashSet<String> =
                remote_folders.iter().map(|f| f.id.clone()).collect();
            let removed_folder_ids: Vec<String> = data.folders.iter()
                .filter(|f| f.id.starts_with(&remote_prefix) && !next_remote_folder_ids.contains(&f.id))
                .map(|f| f.id.clone())
                .collect();
            for folder_id in removed_folder_ids {
                data.delete_folder_scrub_all_windows(&folder_id);
            }
            // Remove old remote folders for this connection
            data.folders.retain(|f| !f.id.starts_with(&remote_prefix));
            // Remove old remote entries from project_order for this connection
            data.project_order.retain(|id| !id.starts_with(&remote_prefix));

            // Add new remote folders
            for rf in remote_folders {
                data.folders.push(rf);
            }

            // Add new remote project_order entries
            data.project_order.extend(remote_order);
        } else {
            // No state (disconnected/connecting) — remove materialized projects and folders
            let prefix = format!("remote:{}:", conn_id);
            let removed_project_ids: Vec<String> = data.projects.iter()
                .filter(|p| p.id.starts_with(&prefix))
                .map(|p| p.id.clone())
                .collect();
            let removed_folder_ids: Vec<String> = data.folders.iter()
                .filter(|f| f.id.starts_with(&prefix))
                .map(|f| f.id.clone())
                .collect();
            for project_id in removed_project_ids {
                data.delete_project_scrub_all_windows(&project_id);
            }
            for folder_id in removed_folder_ids {
                data.delete_folder_scrub_all_windows(&folder_id);
            }
            data.projects.retain(|p| !p.id.starts_with(&prefix));
            data.folders.retain(|f| !f.id.starts_with(&prefix));
            data.project_order.retain(|id| !id.starts_with(&prefix));
        }
    }

    // Remove stale remote projects/folders from connections that no longer exist
    let removed_project_ids: Vec<String> = data.projects.iter()
        .filter(|p| p.is_remote && !expected_remote_ids.contains(&p.id))
        .map(|p| p.id.clone())
        .collect();
    let removed_folder_ids: Vec<String> = data.folders.iter()
        .filter(|f| {
            if f.id.starts_with("remote:") {
                // Remote folder IDs are "remote:{conn_id}:{folder_id}"
                // Extract conn_id (second segment)
                let rest = f.id.strip_prefix("remote:").unwrap_or("");
                let conn_id = rest.split(':').next().unwrap_or("");
                !active_conn_ids.contains(conn_id)
            } else {
                false
            }
        })
        .map(|f| f.id.clone())
        .collect();
    for project_id in removed_project_ids {
        data.delete_project_scrub_all_windows(&project_id);
    }
    for folder_id in removed_folder_ids {
        data.delete_folder_scrub_all_windows(&folder_id);
    }
    data.projects.retain(|p| {
        if p.is_remote {
            expected_remote_ids.contains(&p.id)
        } else {
            true
        }
    });
    data.folders.retain(|f| {
        if f.id.starts_with("remote:") {
            // Remote folder IDs are "remote:{conn_id}:{folder_id}"
            // Extract conn_id (second segment)
            let rest = f.id.strip_prefix("remote:").unwrap_or("");
            let conn_id = rest.split(':').next().unwrap_or("");
            active_conn_ids.contains(conn_id)
        } else {
            true
        }
    });
    let valid_ids: HashSet<&str> = data.projects.iter().map(|p| p.id.as_str())
        .chain(data.folders.iter().map(|f| f.id.as_str()))
        .collect();
    data.project_order.retain(|id| valid_ids.contains(id.as_str()));

    // Compute focus targets for projects that had a window-scoped pending
    // CreateTerminal and whose layout grew a new terminal during this sync.
    let mut outcome = RemoteSyncOutcome::default();
    for pending_focus in remote_sync.drain_pending_focus(window_id) {
        let pid = pending_focus.project_id;
        let layout = match data.projects.iter().find(|p| p.id == pid).and_then(|p| p.layout.as_ref()) {
            Some(layout) => layout,
            None => continue,
        };
        let new_ids = layout.collect_terminal_ids();
        // Find the first terminal ID that wasn't present when the
        // CreateTerminal action originated in this window.
        let old_set: HashSet<&str> = pending_focus.old_terminal_ids.iter().map(|s| s.as_str()).collect();
        if let Some(new_tid) = new_ids.iter().find(|id| !old_set.contains(id.as_str()))
            && let Some(path) = layout.find_terminal_path(new_tid)
        {
            outcome.focus_targets.push(RemoteFocusTarget {
                project_id: pid.clone(),
                layout_path: path,
            });
        }
    }

    outcome
}

/// Apply one-shot per-window visibility for a freshly materialized remote
/// project. When a local window issued the create, the spawn intent ("visible
/// in this window, hidden everywhere else") wins over the wire
/// `show_in_overview` flag; otherwise the wire flag is translated into
/// per-window hidden state on this first sync.
fn apply_initial_remote_project_visibility(
    data: &mut WorkspaceData,
    remote_sync: &mut RemoteSyncState,
    connection_id: &str,
    prefixed_id: &str,
    name: &str,
    path: &str,
    show_in_overview: bool,
) {
    if let Some(spawning_window) = remote_sync.take_project_visibility(connection_id, name, path) {
        data.add_project_hide_in_other_windows(prefixed_id, spawning_window);
        return;
    }
    if !show_in_overview {
        data.hide_project_in_all_windows(prefixed_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use okena_core::api::{ApiFolder, ApiLayoutNode, ApiProject, StateResponse};
    use okena_core::theme::FolderColor;

    fn empty_data() -> WorkspaceData {
        WorkspaceData {
            version: 1,
            projects: Vec::new(),
            project_order: Vec::new(),
            folders: Vec::new(),
            service_panel_heights: HashMap::new(),
            hook_panel_heights: HashMap::new(),
            main_window: okena_state::WindowState::default(),
            extra_windows: Vec::new(),
        }
    }

    fn config(id: &str) -> RemoteConnectionConfig {
        RemoteConnectionConfig {
            id: id.to_string(),
            name: format!("conn-{id}"),
            host: format!("{id}.example.com"),
            port: 19100,
            saved_token: None,
            token_obtained_at: None,
            tls: false,
            pinned_cert_sha256: None,
        }
    }

    fn api_project(id: &str, layout: Option<ApiLayoutNode>) -> ApiProject {
        ApiProject {
            id: id.to_string(),
            name: format!("proj-{id}"),
            path: format!("/srv/{id}"),
            show_in_overview: true,
            layout,
            terminal_names: HashMap::new(),
            git_status: None,
            folder_color: FolderColor::Default,
            services: Vec::new(),
            worktree_info: None,
            worktree_ids: Vec::new(),
        }
    }

    fn terminal(id: &str) -> ApiLayoutNode {
        ApiLayoutNode::Terminal {
            terminal_id: Some(id.to_string()),
            minimized: false,
            detached: false,
        }
    }

    fn state_with(projects: Vec<ApiProject>, order: Vec<String>, folders: Vec<ApiFolder>) -> StateResponse {
        StateResponse {
            state_version: 1,
            projects,
            focused_project_id: None,
            fullscreen_terminal: None,
            project_order: order,
            folders,
            windows: vec![],
        }
    }

    #[test]
    fn adds_prefixed_projects_in_server_order() {
        let mut data = empty_data();
        let mut rs = RemoteSyncState::new();
        let snap = RemoteSnapshot {
            config: config("c1"),
            state: Some(state_with(
                vec![
                    api_project("a", Some(terminal("ta"))),
                    api_project("b", Some(terminal("tb"))),
                ],
                vec!["a".into(), "b".into()],
                vec![],
            )),
        };

        apply_remote_snapshot(&mut data, &mut rs, &[snap], WindowId::Main);

        assert_eq!(data.project_order, vec!["remote:c1:a", "remote:c1:b"]);
        let ids: Vec<&str> = data.projects.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, vec!["remote:c1:a", "remote:c1:b"]);
        assert!(data.projects.iter().all(|p| p.is_remote));
        assert_eq!(data.projects[0].connection_id.as_deref(), Some("c1"));
        // Terminal IDs in the layout are prefixed.
        assert_eq!(
            data.projects[0].layout.as_ref().unwrap().collect_terminal_ids(),
            vec!["remote:c1:ta"]
        );
        // Transient snapshot host recorded.
        assert_eq!(rs.snapshot("remote:c1:a").unwrap().host.as_deref(), Some("c1.example.com"));
    }

    #[test]
    fn builds_prefixed_folders_from_server_order() {
        let mut data = empty_data();
        let mut rs = RemoteSyncState::new();
        let snap = RemoteSnapshot {
            config: config("c1"),
            state: Some(state_with(
                vec![api_project("a", None), api_project("b", None)],
                vec!["f1".into(), "b".into()],
                vec![ApiFolder {
                    id: "f1".into(),
                    name: "Group".into(),
                    project_ids: vec!["a".into()],
                    folder_color: FolderColor::Red,
                }],
            )),
        };

        apply_remote_snapshot(&mut data, &mut rs, &[snap], WindowId::Main);

        assert_eq!(data.project_order, vec!["remote:c1:f1", "remote:c1:b"]);
        assert_eq!(data.folders.len(), 1);
        assert_eq!(data.folders[0].id, "remote:c1:f1");
        assert_eq!(data.folders[0].project_ids, vec!["remote:c1:a"]);
    }

    #[test]
    fn merge_visual_state_preserves_local_state_on_resync() {
        let mut data = empty_data();
        let mut rs = RemoteSyncState::new();

        // First sync: a Tabs layout with two terminals, active_tab 0.
        let layout = ApiLayoutNode::Tabs {
            active_tab: 0,
            children: vec![terminal("t1"), terminal("t2")],
        };
        let first = RemoteSnapshot {
            config: config("c1"),
            state: Some(state_with(vec![api_project("a", Some(layout.clone()))], vec!["a".into()], vec![])),
        };
        apply_remote_snapshot(&mut data, &mut rs, &[first], WindowId::Main);

        // Locally the user switched to tab 1 — mutate the materialized layout.
        if let Some(LayoutNode::Tabs { active_tab, .. }) = data.projects[0].layout.as_mut() {
            *active_tab = 1;
        } else {
            panic!("expected tabs layout");
        }

        // Re-sync with the same server layout (active_tab 0). Local active_tab must win.
        let second = RemoteSnapshot {
            config: config("c1"),
            state: Some(state_with(vec![api_project("a", Some(layout))], vec!["a".into()], vec![])),
        };
        apply_remote_snapshot(&mut data, &mut rs, &[second], WindowId::Main);

        match data.projects[0].layout.as_ref().unwrap() {
            LayoutNode::Tabs { active_tab, .. } => assert_eq!(*active_tab, 1, "local active_tab preserved"),
            _ => panic!("expected tabs layout"),
        }
        // Still only one materialized project (update, not duplicate).
        assert_eq!(data.projects.len(), 1);
    }

    #[test]
    fn prunes_stale_remote_projects_when_connection_gone() {
        let mut data = empty_data();
        let mut rs = RemoteSyncState::new();

        // Sync two connections.
        let c1 = RemoteSnapshot {
            config: config("c1"),
            state: Some(state_with(vec![api_project("a", None)], vec!["a".into()], vec![])),
        };
        let c2 = RemoteSnapshot {
            config: config("c2"),
            state: Some(state_with(vec![api_project("x", None)], vec!["x".into()], vec![])),
        };
        apply_remote_snapshot(&mut data, &mut rs, &[c1, c2], WindowId::Main);
        assert_eq!(data.projects.len(), 2);

        // Re-sync with only c1 present — c2's projects must be pruned.
        let c1_only = RemoteSnapshot {
            config: config("c1"),
            state: Some(state_with(vec![api_project("a", None)], vec!["a".into()], vec![])),
        };
        apply_remote_snapshot(&mut data, &mut rs, &[c1_only], WindowId::Main);

        let ids: Vec<&str> = data.projects.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, vec!["remote:c1:a"]);
        assert_eq!(data.project_order, vec!["remote:c1:a"]);
    }

    #[test]
    fn disconnected_connection_removes_its_materialized_projects() {
        let mut data = empty_data();
        let mut rs = RemoteSyncState::new();

        apply_remote_snapshot(&mut data, &mut rs, &[RemoteSnapshot {
            config: config("c1"),
            state: Some(state_with(vec![api_project("a", None)], vec!["a".into()], vec![])),
        }], WindowId::Main);
        assert_eq!(data.projects.len(), 1);

        // Same connection now reports no state (disconnected).
        apply_remote_snapshot(&mut data, &mut rs, &[RemoteSnapshot {
            config: config("c1"),
            state: None,
        }], WindowId::Main);
        assert!(data.projects.is_empty());
        assert!(data.project_order.is_empty());
    }

    #[test]
    fn pending_focus_detects_new_terminal() {
        let mut data = empty_data();
        let mut rs = RemoteSyncState::new();

        // Initial sync: project with one terminal.
        apply_remote_snapshot(&mut data, &mut rs, &[RemoteSnapshot {
            config: config("c1"),
            state: Some(state_with(vec![api_project("a", Some(terminal("t1")))], vec!["a".into()], vec![])),
        }], WindowId::Main);

        // Queue pending focus for the project (as a CreateTerminal dispatch would).
        rs.queue_focus(WindowId::Main, "remote:c1:a", vec!["remote:c1:t1".to_string()]);

        // Next sync grows the layout with a second terminal.
        let grown = ApiLayoutNode::Split {
            direction: okena_core::types::SplitDirection::Horizontal,
            sizes: vec![50.0, 50.0],
            children: vec![terminal("t1"), terminal("t2")],
        };
        let outcome = apply_remote_snapshot(&mut data, &mut rs, &[RemoteSnapshot {
            config: config("c1"),
            state: Some(state_with(vec![api_project("a", Some(grown))], vec!["a".into()], vec![])),
        }], WindowId::Main);

        assert_eq!(outcome.focus_targets.len(), 1);
        let target = &outcome.focus_targets[0];
        assert_eq!(target.project_id, "remote:c1:a");
        // The new terminal is the second child of the split → path [1].
        assert_eq!(target.layout_path, vec![1]);
        // Pending focus drained.
        assert!(rs.drain_pending_focus(WindowId::Main).is_empty());
    }

    #[test]
    fn no_focus_target_when_no_new_terminal() {
        let mut data = empty_data();
        let mut rs = RemoteSyncState::new();

        apply_remote_snapshot(&mut data, &mut rs, &[RemoteSnapshot {
            config: config("c1"),
            state: Some(state_with(vec![api_project("a", Some(terminal("t1")))], vec!["a".into()], vec![])),
        }], WindowId::Main);
        rs.queue_focus(WindowId::Main, "remote:c1:a", vec!["remote:c1:t1".to_string()]);

        // Re-sync with the identical layout — nothing new appeared.
        let outcome = apply_remote_snapshot(&mut data, &mut rs, &[RemoteSnapshot {
            config: config("c1"),
            state: Some(state_with(vec![api_project("a", Some(terminal("t1")))], vec!["a".into()], vec![])),
        }], WindowId::Main);

        assert!(outcome.focus_targets.is_empty());
        assert!(rs.drain_pending_focus(WindowId::Main).is_empty());
    }

    #[test]
    fn initial_visibility_consumes_pending_create_window() {
        let mut data = empty_data();
        let extra_a = okena_state::WindowState::default();
        let extra_a_id = extra_a.id;
        let extra_b = okena_state::WindowState::default();
        let extra_b_id = extra_b.id;
        data.extra_windows = vec![extra_a, extra_b];
        let mut rs = RemoteSyncState::new();
        rs.queue_project_visibility(
            WindowId::Extra(extra_a_id),
            "conn",
            "Project",
            Some("/repo/project"),
        );

        apply_initial_remote_project_visibility(
            &mut data,
            &mut rs,
            "conn",
            "remote:conn:p1",
            "Project",
            "/repo/project",
            true,
        );

        assert!(data.main_window.hidden_project_ids.contains("remote:conn:p1"));
        assert!(
            !data.window(WindowId::Extra(extra_a_id)).unwrap()
                .hidden_project_ids.contains("remote:conn:p1")
        );
        assert!(
            data.window(WindowId::Extra(extra_b_id)).unwrap()
                .hidden_project_ids.contains("remote:conn:p1")
        );
        // The pending create-visibility request was consumed.
        assert_eq!(rs.take_project_visibility("conn", "Project", "/repo/project"), None);
    }

    #[test]
    fn initial_visibility_without_pending_uses_wire_hidden_flag() {
        let mut data = empty_data();
        let extra = okena_state::WindowState::default();
        let extra_id = extra.id;
        data.extra_windows = vec![extra];
        let mut rs = RemoteSyncState::new();

        apply_initial_remote_project_visibility(
            &mut data,
            &mut rs,
            "conn",
            "remote:conn:p1",
            "Project",
            "/repo/project",
            false,
        );

        assert!(data.main_window.hidden_project_ids.contains("remote:conn:p1"));
        assert!(
            data.window(WindowId::Extra(extra_id)).unwrap()
                .hidden_project_ids.contains("remote:conn:p1")
        );
    }
}
