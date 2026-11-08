use crate::routes::{AppState, PeerInfo};
use axum::Json;
use axum::extract::{Extension, Query, State};
use axum::http::StatusCode;
use okena_core::api::{
    ApiAppRestartRequest, ApiRestartedDesktop, ApiUpdateRestartResponse, ApiWindow,
};
use okena_ext_updater::UpdateStatus;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tokio::sync::broadcast;

#[derive(serde::Deserialize)]
pub struct RevertRequest {
    pub version: String,
    #[serde(default)]
    pub keep_config: bool,
}

pub async fn get_status(
    Extension(peer): Extension<PeerInfo>,
    State(state): State<AppState>,
) -> Result<Json<okena_ext_updater::UpdateStatusSnapshot>, StatusCode> {
    if !peer.is_local_trusted() {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(Json(state.update_info.snapshot()))
}

pub async fn post_check(
    Extension(peer): Extension<PeerInfo>,
    State(state): State<AppState>,
) -> Result<Json<okena_ext_updater::UpdateStatusSnapshot>, StatusCode> {
    if !peer.is_local_trusted() {
        return Err(StatusCode::FORBIDDEN);
    }

    let info = state.update_info.clone();
    if info.try_start_manual() {
        let token = info.current_token();
        tokio::spawn(async move {
            okena_ext_updater::manager::run_check(info, token, true).await;
        });
    }

    Ok(Json(state.update_info.snapshot()))
}

pub async fn post_install(
    Extension(peer): Extension<PeerInfo>,
    State(state): State<AppState>,
) -> Result<Json<okena_ext_updater::UpdateStatusSnapshot>, StatusCode> {
    if !peer.is_local_trusted() {
        return Err(StatusCode::FORBIDDEN);
    }

    let info = state.update_info.clone();
    if matches!(info.status(), UpdateStatus::Ready { .. }) {
        tokio::spawn(async move {
            okena_ext_updater::manager::install_ready_update(info).await;
        });
    }

    Ok(Json(state.update_info.snapshot()))
}

/// A relaunch request for the desktops in `connections`.
#[derive(Clone, Debug)]
pub struct AppRestartPush {
    pub request: ApiAppRestartRequest,
    pub connections: Arc<HashSet<u64>>,
}

/// `POST /v1/update/restart`: restart the daemon into the installed update and
/// ask every desktop on this host to relaunch once the replacement is up.
///
/// The daemon restarts itself here, once, rather than each desktop asking for
/// it: two desktops would otherwise restart it twice.
pub async fn post_restart(
    Extension(peer): Extension<PeerInfo>,
    State(state): State<AppState>,
) -> Result<Json<ApiUpdateRestartResponse>, (StatusCode, String)> {
    if !peer.is_local_trusted() {
        return Err((StatusCode::FORBIDDEN, String::new()));
    }
    let UpdateStatus::ReadyToRestart { version, .. } = state.update_info.status() else {
        return Err((
            StatusCode::CONFLICT,
            "no installed update is waiting for a restart".to_string(),
        ));
    };

    // Spawn the replacement before anything else: if it cannot start, stay
    // alive and leave the desktops running.
    if let Err(error) = crate::local::spawn_replacement_daemon() {
        log::error!("Update restart failed to spawn a replacement daemon: {error}");
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to start the replacement daemon: {error}"),
        ));
    }
    let daemon_pid = std::process::id();
    let desktops = push_app_restart(
        &state.local_connections,
        &state.remote_windows,
        &state.app_restart_tx,
        ApiAppRestartRequest {
            version: version.clone(),
            daemon_pid,
        },
    );
    log::info!(
        "Restarting into v{version}; asked {} desktop(s) to relaunch",
        desktops.len()
    );
    super::restart::exit_after_ack(&state);

    Ok(Json(ApiUpdateRestartResponse {
        version,
        daemon_pid,
        desktops,
    }))
}

/// Push a relaunch request to every desktop on this host: a local connection
/// that declared its windows. Returns them in connection order.
fn push_app_restart(
    local_connections: &RwLock<HashSet<u64>>,
    remote_windows: &RwLock<HashMap<u64, Vec<ApiWindow>>>,
    app_restart_tx: &broadcast::Sender<AppRestartPush>,
    request: ApiAppRestartRequest,
) -> Vec<ApiRestartedDesktop> {
    let local = local_connections
        .read()
        .map(|set| set.clone())
        .unwrap_or_default();
    let mut desktops: Vec<ApiRestartedDesktop> = remote_windows
        .read()
        .map(|map| {
            map.iter()
                .filter(|(connection_id, _)| local.contains(connection_id))
                .map(|(connection_id, windows)| ApiRestartedDesktop {
                    connection_id: *connection_id,
                    windows: windows.len(),
                })
                .collect()
        })
        .unwrap_or_default();
    desktops.sort_by_key(|desktop| desktop.connection_id);

    if !desktops.is_empty() {
        let push = AppRestartPush {
            request,
            connections: Arc::new(desktops.iter().map(|d| d.connection_id).collect()),
        };
        if app_restart_tx.send(push).is_err() {
            log::warn!("No connection is listening for the app restart request");
        }
    }
    desktops
}

pub async fn post_dismiss(
    Extension(peer): Extension<PeerInfo>,
    State(state): State<AppState>,
) -> Result<Json<okena_ext_updater::UpdateStatusSnapshot>, StatusCode> {
    if !peer.is_local_trusted() {
        return Err(StatusCode::FORBIDDEN);
    }

    state.update_info.dismiss();
    Ok(Json(state.update_info.snapshot()))
}

pub async fn get_releases(
    Extension(peer): Extension<PeerInfo>,
    State(state): State<AppState>,
) -> Result<Json<okena_ext_updater::ReleaseCatalog>, StatusCode> {
    if !peer.is_local_trusted() {
        return Err(StatusCode::FORBIDDEN);
    }
    okena_ext_updater::checker::list_revert_releases(state.update_info.app_version())
        .await
        .map(Json)
        .map_err(|error| {
            log::error!("Failed to list revert releases: {error}");
            StatusCode::BAD_GATEWAY
        })
}

#[derive(serde::Deserialize)]
pub struct ReleaseNotesQuery {
    /// The client's running version: an older daemon can outlive a desktop update.
    pub version: String,
}

pub async fn get_release_notes(
    Extension(peer): Extension<PeerInfo>,
    State(state): State<AppState>,
    Query(query): Query<ReleaseNotesQuery>,
) -> Result<Json<okena_ext_updater::ReleaseNotes>, StatusCode> {
    if !peer.is_local_trusted() {
        return Err(StatusCode::FORBIDDEN);
    }
    let current = okena_ext_updater::checker::parse_release_version(&query.version)
        .ok_or(StatusCode::BAD_REQUEST)?;
    okena_ext_updater::checker::list_release_notes(
        state.update_info.release_notes_cache(),
        state.update_info.app_version(),
        current,
    )
    .await
    .map(Json)
    .map_err(|error| {
        log::error!("Failed to list release notes: {error}");
        StatusCode::BAD_GATEWAY
    })
}

pub async fn post_revert(
    Extension(peer): Extension<PeerInfo>,
    State(state): State<AppState>,
    Json(request): Json<RevertRequest>,
) -> Result<Json<okena_ext_updater::UpdateStatusSnapshot>, StatusCode> {
    if !peer.is_local_trusted() {
        return Err(StatusCode::FORBIDDEN);
    }
    let info = state.update_info.clone();
    // Unlike check/install, a rejected revert must be reported: the caller waits
    // for a status transition that would otherwise never come.
    if !info.try_start_manual() {
        return Err(StatusCode::CONFLICT);
    }
    info.set_status(UpdateStatus::Checking);
    tokio::spawn(async move {
        okena_ext_updater::manager::run_revert(info, request.version, !request.keep_config).await;
    });
    Ok(Json(state.update_info.snapshot()))
}

pub fn spawn_background_checker(update_info: okena_ext_updater::UpdateInfo) {
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(30)).await;

        loop {
            if let Some(token) = update_info.try_start() {
                okena_ext_updater::manager::run_check(update_info.clone(), token, false).await;
            }

            match update_info.status() {
                UpdateStatus::Ready { .. }
                | UpdateStatus::ReadyToRestart { .. }
                | UpdateStatus::Installing { .. }
                | UpdateStatus::BrewUpdate { .. } => return,
                _ => {}
            }

            if matches!(update_info.status(), UpdateStatus::Failed { .. }) {
                tokio::time::sleep(Duration::from_secs(60)).await;
                if matches!(update_info.status(), UpdateStatus::Failed { .. }) {
                    update_info.set_status(UpdateStatus::Idle);
                }
            }

            tokio::time::sleep(Duration::from_secs(24 * 60 * 60)).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::{AppRestartPush, push_app_restart};
    use okena_core::api::{ApiAppRestartRequest, ApiRestartedDesktop, ApiWindow};
    use std::collections::{HashMap, HashSet};
    use std::sync::RwLock;
    use tokio::sync::broadcast;

    fn window(id: &str) -> ApiWindow {
        let kind = if id == "main" { "main" } else { "extra" };
        ApiWindow {
            id: id.to_string(),
            kind: kind.to_string(),
            active: id == "main",
            focused_project_id: None,
            focused_terminal_id: None,
            fullscreen: None,
            visible_project_ids: Vec::new(),
            folder_filter: None,
            bounds: None,
            sidebar_open: None,
        }
    }

    fn request() -> ApiAppRestartRequest {
        ApiAppRestartRequest {
            version: "0.30.0".to_string(),
            daemon_pid: 4242,
        }
    }

    #[test]
    fn no_desktop_gets_no_push() {
        // A local client that declared no windows (CLI, web on loopback) is no desktop.
        let local = RwLock::new(HashSet::from([1]));
        let windows = RwLock::new(HashMap::new());
        let (tx, mut rx) = broadcast::channel::<AppRestartPush>(4);

        let desktops = push_app_restart(&local, &windows, &tx, request());

        assert!(desktops.is_empty());
        assert!(rx.try_recv().is_err(), "nothing must be pushed");
    }

    #[test]
    fn every_desktop_on_this_host_is_asked_once() {
        // Connections 2 and 5 are local desktops, 3 a desktop on another host.
        let local = RwLock::new(HashSet::from([1, 2, 5]));
        let windows = RwLock::new(HashMap::from([
            (5, vec![window("main")]),
            (3, vec![window("main")]),
            (2, vec![window("main"), window("6f1c")]),
        ]));
        let (tx, mut rx) = broadcast::channel::<AppRestartPush>(4);

        let desktops = push_app_restart(&local, &windows, &tx, request());

        assert_eq!(
            desktops,
            [
                ApiRestartedDesktop {
                    connection_id: 2,
                    windows: 2,
                },
                ApiRestartedDesktop {
                    connection_id: 5,
                    windows: 1,
                },
            ]
        );
        let push = rx.try_recv().expect("one push");
        assert_eq!(push.request, request());
        assert_eq!(*push.connections, HashSet::from([2, 5]));
        assert!(rx.try_recv().is_err(), "exactly one push");
    }
}
