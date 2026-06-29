use crate::routes::{AppState, PeerInfo};
use axum::Json;
use axum::extract::{Extension, State};
use axum::http::StatusCode;
use okena_ext_updater::UpdateStatus;
use std::time::Duration;

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
