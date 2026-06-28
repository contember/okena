use crate::routes::AppState;
use axum::Json;
use axum::extract::{ConnectInfo, State};
use axum::http::StatusCode;
use okena_ext_updater::UpdateStatus;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

pub async fn get_status(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(state): State<AppState>,
) -> Result<Json<okena_ext_updater::UpdateStatusSnapshot>, StatusCode> {
    if !is_loopback_peer(addr) {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(Json(state.update_info.snapshot()))
}

pub async fn post_check(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(state): State<AppState>,
) -> Result<Json<okena_ext_updater::UpdateStatusSnapshot>, StatusCode> {
    if !is_loopback_peer(addr) {
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
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(state): State<AppState>,
) -> Result<Json<okena_ext_updater::UpdateStatusSnapshot>, StatusCode> {
    if !is_loopback_peer(addr) {
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
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(state): State<AppState>,
) -> Result<Json<okena_ext_updater::UpdateStatusSnapshot>, StatusCode> {
    if !is_loopback_peer(addr) {
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

fn is_loopback_peer(addr: SocketAddr) -> bool {
    match addr.ip() {
        IpAddr::V4(v4) => v4.is_loopback(),
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => v4.is_loopback(),
            None => v6.is_loopback(),
        },
    }
}
