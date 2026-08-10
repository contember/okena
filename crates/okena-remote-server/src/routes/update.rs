use crate::routes::{AppState, PeerInfo};
use axum::Json;
use axum::extract::{Extension, State};
use axum::http::StatusCode;
use okena_ext_updater::UpdateStatus;
use std::time::Duration;

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
