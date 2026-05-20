//! Update orchestration driven by user actions (manual check / install).
//!
//! These async routines own the status transitions, download polling, and
//! error handling. The host view supplies a `notify` callback that is invoked
//! whenever the UI should re-render (e.g. to call `cx.notify()` on the
//! observing entity). Keeping this logic here keeps the view thin and makes
//! the orchestration testable independently of any particular GPUI view.

use crate::status::{UpdateInfo, UpdateStatus};
use gpui::AsyncApp;

/// Run a manual "check for updates" pass: check GitHub, and if a non-Homebrew
/// update is found, download it (with periodic progress refresh) and transition
/// to `Ready`. `notify` is called whenever the UI should refresh.
///
/// Assumes the caller has already taken the manual-check guard
/// (`UpdateInfo::try_start_manual`), set `Checking` status, and notified once.
/// This routine calls `finish_manual` when it completes.
pub async fn run_manual_check<F>(info: UpdateInfo, token: u64, cx: &mut AsyncApp, notify: F)
where
    F: Fn(&mut AsyncApp),
{
    match crate::checker::check_for_update(info.app_version()).await {
        Ok(Some(release)) => {
            if info.is_homebrew() {
                info.set_status(UpdateStatus::BrewUpdate {
                    version: release.version,
                });
                notify(cx);
            } else {
                // Set downloading status and notify before the blocking download
                info.set_status(UpdateStatus::Downloading {
                    version: release.version.clone(),
                    progress: 0,
                });
                notify(cx);

                // Download with periodic UI refresh for progress
                let download = crate::downloader::download_asset(
                    release.asset_url,
                    release.asset_name,
                    release.version.clone(),
                    info.clone(),
                    token,
                    release.checksum_url,
                );
                let mut download = std::pin::pin!(download);

                let download_result: anyhow::Result<std::path::PathBuf> = loop {
                    let polled = std::future::poll_fn(|task_cx| {
                        match download.as_mut().poll(task_cx) {
                            std::task::Poll::Ready(r) => std::task::Poll::Ready(Some(r)),
                            std::task::Poll::Pending => std::task::Poll::Ready(None),
                        }
                    })
                    .await;
                    match polled {
                        Some(r) => break r,
                        None => {
                            smol::Timer::after(std::time::Duration::from_millis(250)).await;
                            notify(cx);
                        }
                    }
                };

                match download_result {
                    Ok(path) => {
                        info.set_status(UpdateStatus::Ready {
                            version: release.version,
                            path,
                        });
                        notify(cx);
                    }
                    Err(e) => {
                        log::error!("Download failed: {}", e);
                        info.set_status(UpdateStatus::Failed {
                            error: e.to_string(),
                        });
                        notify(cx);
                    }
                }
            }
        }
        Ok(None) => {
            info.set_status(UpdateStatus::Idle);
            notify(cx);
        }
        Err(e) => {
            log::error!("Update check failed: {}", e);
            info.set_status(UpdateStatus::Failed {
                error: e.to_string(),
            });
            notify(cx);
        }
    }

    info.finish_manual();
}

/// Run the install step for an already-downloaded update at `path`,
/// transitioning to `ReadyToRestart` on success or `Failed` on error.
/// `notify` is called once the install completes.
///
/// Assumes the caller has already set `Installing` status and notified once.
pub async fn run_install<F>(
    info: UpdateInfo,
    version: String,
    path: std::path::PathBuf,
    cx: &mut AsyncApp,
    notify: F,
) where
    F: Fn(&mut AsyncApp),
{
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
    notify(cx);
}
