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
            info.set_status(UpdateStatus::ReadyToRestart {
                version,
                config_restore: None,
            });
        }
        Err(e) => {
            log::error!("Install failed: {}", e);
            info.set_status(UpdateStatus::Failed {
                error: e.to_string(),
            });
        }
    }
}

/// Download and install one exact older release. Config restoration is deferred
/// until daemon restart so the outgoing daemon cannot overwrite restored files.
pub async fn run_revert(info: UpdateInfo, target_version: String, restore_config: bool) {
    info.set_status(UpdateStatus::Checking);

    let result = run_revert_inner(&info, &target_version, restore_config).await;
    if let Err(error) = result {
        log::error!("Version revert failed: {error}");
        info.set_status(UpdateStatus::Failed {
            error: error.to_string(),
        });
    }
    info.finish_manual();
}

async fn run_revert_inner(
    info: &UpdateInfo,
    target_version: &str,
    restore_config: bool,
) -> anyhow::Result<()> {
    if info.is_homebrew() {
        anyhow::bail!("Homebrew installations must be changed through brew");
    }
    let release =
        crate::checker::release_for_revert(info.app_version(), target_version.to_string()).await?;
    let config_snapshot = if restore_config {
        let paths = okena_core::profiles::current();
        Some(
            okena_core::profiles::config_snapshot_for_version(paths, target_version).ok_or_else(
                || anyhow::anyhow!("no config snapshot exists for version {target_version}"),
            )?,
        )
    } else {
        None
    };

    info.set_status(UpdateStatus::Downloading {
        version: release.version.clone(),
        progress: 0,
    });
    let path = crate::downloader::download_asset(
        release.asset_url,
        release.asset_name,
        release.version.clone(),
        info.clone(),
        info.current_token(),
        release.checksum_url,
    )
    .await?;

    if restore_config {
        okena_core::profiles::schedule_config_restore(
            okena_core::profiles::current(),
            target_version,
        )?;
    }
    info.set_status(UpdateStatus::Installing {
        version: release.version.clone(),
    });
    let install = smol::unblock(move || crate::installer::install_update(&path)).await;
    if let Err(error) = install {
        okena_core::profiles::clear_pending_config_restore(okena_core::profiles::current());
        return Err(error);
    }

    info.set_status(UpdateStatus::ReadyToRestart {
        version: release.version,
        config_restore: config_snapshot,
    });
    Ok(())
}
