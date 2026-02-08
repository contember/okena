use crate::updater::{UpdateInfo, UpdateStatus};
use gpui::*;
use std::future::Future;

use super::Okena;

impl Okena {
    /// Spawn the update checker loop (30s delay, check, optionally download, sleep 24h).
    /// Uses `try_start()` to prevent duplicate loops and respects cancellation via token.
    pub(super) fn start_update_checker(update_info: UpdateInfo, cx: &mut Context<Self>) {
        let token = match update_info.try_start() {
            Some(t) => t,
            None => return, // A checker loop is already running
        };

        cx.spawn(async move |this: WeakEntity<Okena>, cx| {
            // Initial delay — check cancellation every second
            for _ in 0..30 {
                if update_info.is_cancelled(token) {
                    update_info.mark_stopped(token);
                    return;
                }
                smol::Timer::after(std::time::Duration::from_secs(1)).await;
            }

            loop {
                if update_info.is_cancelled(token) {
                    update_info.mark_stopped(token);
                    return;
                }

                // Pause while a manual check is in progress
                while update_info.is_manual_active() {
                    if update_info.is_cancelled(token) {
                        update_info.mark_stopped(token);
                        return;
                    }
                    smol::Timer::after(std::time::Duration::from_secs(1)).await;
                }

                // If an update was already found (e.g. by a manual check), stop
                match update_info.status() {
                    UpdateStatus::Ready { .. }
                    | UpdateStatus::ReadyToRestart { .. }
                    | UpdateStatus::Installing { .. }
                    | UpdateStatus::BrewUpdate { .. } => {
                        update_info.mark_stopped(token);
                        return;
                    }
                    _ => {}
                }

                update_info.set_status(UpdateStatus::Checking);
                let _ = this.update(cx, |_, cx| cx.notify());

                match crate::updater::checker::check_for_update().await {
                    Ok(Some(release)) => {

                        if update_info.is_homebrew() {
                            update_info.set_status(UpdateStatus::BrewUpdate {
                                version: release.version,
                            });
                            let _ = this.update(cx, |_, cx| cx.notify());
                            update_info.mark_stopped(token);
                            return;
                        }

                        if update_info.is_cancelled(token) {
                            update_info.mark_stopped(token);
                            return;
                        }

                        // Download with retry (up to 3 attempts) and periodic UI refresh
                        let asset_url = release.asset_url;
                        let asset_name = release.asset_name;
                        let version = release.version;
                        let checksum_url = release.checksum_url;

                        update_info.set_status(UpdateStatus::Downloading {
                            version: version.clone(),
                            progress: 0,
                        });
                        let _ = this.update(cx, |_, cx| cx.notify());

                        let mut last_err: Option<anyhow::Error> = None;
                        for attempt in 0..3u32 {
                            if attempt > 0 {
                                // Backoff: 30s, 60s
                                let delay_secs = 30u64 * (1 << (attempt - 1));
                                for _ in 0..delay_secs {
                                    if update_info.is_cancelled(token) {
                                        update_info.mark_stopped(token);
                                        return;
                                    }
                                    smol::Timer::after(std::time::Duration::from_secs(1)).await;
                                }
                                update_info.set_status(UpdateStatus::Downloading {
                                    version: version.clone(),
                                    progress: 0,
                                });
                                let _ = this.update(cx, |_, cx| cx.notify());
                            }

                            let download = crate::updater::downloader::download_asset(
                                asset_url.clone(),
                                asset_name.clone(),
                                version.clone(),
                                update_info.clone(),
                                token,
                                checksum_url.clone(),
                            );
                            let mut download = std::pin::pin!(download);

                            let result = loop {
                                let polled = std::future::poll_fn(|task_cx| {
                                    match download.as_mut().poll(task_cx) {
                                        std::task::Poll::Ready(r) => std::task::Poll::Ready(Some(r)),
                                        std::task::Poll::Pending => std::task::Poll::Ready(None),
                                    }
                                }).await;
                                match polled {
                                    Some(r) => break r,
                                    None => {
                                        smol::Timer::after(std::time::Duration::from_millis(250)).await;
                                        let _ = this.update(cx, |_, cx| cx.notify());
                                    }
                                }
                            };

                            match result {
                                Ok(path) => {
                                    update_info.set_status(UpdateStatus::Ready {
                                        version,
                                        path,
                                    });
                                    let _ = this.update(cx, |_, cx| cx.notify());
                                    update_info.mark_stopped(token);
                                    return;
                                }
                                Err(e) => {
                                    if update_info.is_cancelled(token) {
                                        update_info.mark_stopped(token);
                                        return;
                                    }
                                    log::warn!("Download attempt {}/3 failed: {}", attempt + 1, e);
                                    last_err = Some(e);
                                }
                            }
                        }

                        if let Some(e) = last_err {
                            log::error!("Download failed after 3 attempts: {}", e);
                            update_info.set_status(UpdateStatus::Failed {
                                error: e.to_string(),
                            });
                            let _ = this.update(cx, |_, cx| cx.notify());
                        }
                    }
                    Ok(None) => {
                        update_info.set_status(UpdateStatus::Idle);
                        let _ = this.update(cx, |_, cx| cx.notify());
                    }
                    Err(e) => {
                        log::error!("Update check failed: {}", e);
                        update_info.set_status(UpdateStatus::Failed {
                            error: e.to_string(),
                        });
                        let _ = this.update(cx, |_, cx| cx.notify());
                    }
                }

                // Keep Failed status visible for 60 seconds before clearing
                if matches!(update_info.status(), UpdateStatus::Failed { .. }) {
                    for _ in 0..60 {
                        if update_info.is_cancelled(token) {
                            update_info.mark_stopped(token);
                            return;
                        }
                        smol::Timer::after(std::time::Duration::from_secs(1)).await;
                    }
                    // Only reset if still Failed (a manual check may have changed status)
                    if matches!(update_info.status(), UpdateStatus::Failed { .. }) {
                        update_info.set_status(UpdateStatus::Idle);
                        let _ = this.update(cx, |_, cx| cx.notify());
                    }
                }

                // Wait 24 hours, checking cancellation every minute
                for _ in 0..(24 * 60) {
                    if update_info.is_cancelled(token) {
                        update_info.mark_stopped(token);
                        return;
                    }
                    smol::Timer::after(std::time::Duration::from_secs(60)).await;
                }
            }
        })
        .detach();
    }
}
