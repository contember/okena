use crate::status::{UpdateInfo, UpdateStatus};

/// Run one check/download pass. The caller owns concurrency guards
/// (`try_start_manual` for user-initiated checks, `try_start` for background).
pub async fn run_check(info: UpdateInfo, token: u64, finish_manual: bool) {
    info.set_status(UpdateStatus::Checking);

    match crate::checker::check_for_update(info.app_version()).await {
        Ok(Some(release)) => {
            if info.is_homebrew() {
                info.set_status(UpdateStatus::BrewUpdate {
                    version: release.version,
                });
            } else {
                info.set_status(UpdateStatus::Downloading {
                    version: release.version.clone(),
                    progress: 0,
                });

                match crate::downloader::download_asset(
                    release.asset_url,
                    release.asset_name,
                    release.version.clone(),
                    info.clone(),
                    token,
                    release.checksum_url,
                )
                .await
                {
                    Ok(path) => {
                        info.set_status(UpdateStatus::Ready {
                            version: release.version,
                            path,
                        });
                    }
                    Err(e) => {
                        if !info.is_cancelled(token) {
                            log::error!("Download failed: {}", e);
                            info.set_status(UpdateStatus::Failed {
                                error: e.to_string(),
                            });
                        }
                    }
                }
            }
        }
        Ok(None) => {
            info.set_status(UpdateStatus::Idle);
        }
        Err(e) => {
            log::error!("Update check failed: {}", e);
            info.set_status(UpdateStatus::Failed {
                error: e.to_string(),
            });
        }
    }

    if finish_manual {
        info.finish_manual();
    } else {
        info.mark_stopped(token);
    }
}

/// Install the downloaded update currently held in `Ready` status.
pub async fn install_ready_update(info: UpdateInfo) {
    let (version, path) = match info.status() {
        UpdateStatus::Ready { version, path } => (version, path),
        _ => return,
    };

    info.set_status(UpdateStatus::Installing {
        version: version.clone(),
    });

    let result = smol::unblock(move || crate::installer::install_update(&path)).await;
    match result {
        Ok(_) => {
            info.set_status(UpdateStatus::ReadyToRestart { version });
        }
        Err(e) => {
            log::error!("Install failed: {}", e);
            info.set_status(UpdateStatus::Failed {
                error: e.to_string(),
            });
        }
    }
}
