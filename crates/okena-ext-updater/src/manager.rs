use crate::status::{UpdateChannel, UpdateInfo, UpdateReservation, UpdateStatus};
use std::path::{Path, PathBuf};

/// Run the daemon's update policy for a reserved user or background check.
pub async fn run_check(info: UpdateInfo, token: u64, manual: bool) {
    let _reservation = if manual {
        info.adopt_manual(token)
    } else {
        info.adopt_background(token)
    };
    if info.has_staged_update() || !info.set_status_if_current(token, UpdateStatus::Checking) {
        return;
    }
    let channel = info.snapshot().channel;
    let version = info.app_version();
    if info.is_homebrew() {
        match crate::checker::check_for_package_update(&version).await {
            Ok(Some(version)) => {
                info.set_status_if_current(
                    token,
                    UpdateStatus::BrewUpdate {
                        version: version.to_string(),
                    },
                );
            }
            Ok(None) => {
                info.set_status_if_current(token, UpdateStatus::Idle);
            }
            Err(error) => fail(&info, token, error),
        }
        return;
    }
    match crate::checker::check_for_update(&version, channel).await {
        Ok(Some(release)) => {
            let stable_return = channel == UpdateChannel::Release
                && okena_core::build_info::BUILD_INFO.nightly.is_some();
            let version = release.version.clone();
            match download(&info, token, release).await {
                Ok(path) => info.stage_download(token, version, path, stable_return),
                Err(error) => fail(&info, token, error),
            }
        }
        Ok(None) => {
            info.set_status_if_current(token, UpdateStatus::Idle);
        }
        Err(error) => fail(&info, token, error),
    }
}

async fn download(
    info: &UpdateInfo,
    token: u64,
    release: crate::checker::ReleaseAsset,
) -> anyhow::Result<PathBuf> {
    anyhow::ensure!(
        info.set_status_if_current(
            token,
            UpdateStatus::Downloading {
                version: release.version.clone(),
                progress: 0
            }
        ),
        "download cancelled"
    );
    crate::downloader::download_asset(
        release.asset_url,
        release.asset_name,
        release.version,
        info.clone(),
        token,
        release.sha256,
    )
    .await
}

fn fail(info: &UpdateInfo, token: u64, error: anyhow::Error) {
    if info.set_status_if_current(
        token,
        UpdateStatus::Failed {
            error: format!("{error:#}"),
        },
    ) {
        log::error!("Update failed: {error:#}");
    }
}

/// The route claims the exact staged archive before acknowledging the request.
pub async fn install_claimed(
    info: UpdateInfo,
    reservation: UpdateReservation,
    version: String,
    path: PathBuf,
    restore_config: bool,
) {
    let result = install(&info, &version, &path, restore_config, false).await;
    crate::downloader::discard_download(&path);
    if let Err(error) = result {
        fail(&info, reservation.token(), error);
    }
}

async fn install(
    info: &UpdateInfo,
    version: &str,
    path: &Path,
    restore_config: bool,
    revert: bool,
) -> anyhow::Result<()> {
    let config_restore = if restore_config {
        let paths = okena_core::profiles::current();
        let snapshot = okena_core::profiles::config_snapshot_for_version(paths, version)
            .ok_or_else(|| anyhow::anyhow!("no config snapshot exists for version {version}"))?;
        okena_core::profiles::schedule_config_restore(paths, version)?;
        Some(snapshot)
    } else {
        None
    };
    let archive = path.to_owned();
    if let Err(error) = smol::unblock(move || crate::installer::install_update(&archive)).await {
        if restore_config {
            okena_core::profiles::clear_pending_config_restore(okena_core::profiles::current());
        }
        return Err(error);
    }
    info.set_status(UpdateStatus::ReadyToRestart {
        version: version.to_owned(),
        config_restore,
        revert,
    });
    Ok(())
}

/// A historical revert has already been confirmed by the caller.
pub async fn run_revert(
    info: UpdateInfo,
    token: u64,
    target_version: String,
    restore_config: bool,
) {
    let _reservation = info.adopt_manual(token);
    let result = run_revert_inner(&info, token, &target_version, restore_config).await;
    if let Err(error) = result {
        fail(&info, token, error);
    }
}

async fn run_revert_inner(
    info: &UpdateInfo,
    token: u64,
    version: &str,
    restore_config: bool,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        !info.is_homebrew(),
        "Homebrew installations must be changed through brew"
    );
    anyhow::ensure!(!info.is_cancelled(token), "version revert cancelled");
    let channel = info.snapshot().channel;
    let (release, newer) =
        crate::checker::release_for_revert(info.app_version(), version.to_owned(), channel).await?;
    let path = download(info, token, release).await?;
    let result = match info.begin_revert_install(token, version.to_owned()) {
        Ok(()) => install(info, version, &path, restore_config, !newer).await,
        Err(error) => Err(error),
    };
    crate::downloader::discard_download(&path);
    result
}
