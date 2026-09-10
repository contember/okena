use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(feature = "gpui-ui")]
use gpui::*;
#[cfg(feature = "gpui-ui")]
use gpui_component::h_flex;
#[cfg(feature = "gpui-ui")]
use gpui_component::tooltip::Tooltip;
#[cfg(feature = "gpui-ui")]
use okena_extensions::ThemeColors;
#[cfg(feature = "gpui-ui")]
use okena_ui::tokens::ui_text_sm;

/// Status of the update process.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum UpdateStatus {
    Idle,
    Checking,
    #[allow(dead_code)]
    Available {
        version: String,
        asset_url: String,
        asset_name: String,
    },
    Downloading {
        version: String,
        progress: u8,
    },
    Ready {
        version: String,
        path: std::path::PathBuf,
    },
    Installing {
        version: String,
    },
    ReadyToRestart {
        version: String,
        #[serde(default)]
        config_restore: Option<okena_core::profiles::ConfigSnapshot>,
    },
    BrewUpdate {
        version: String,
    },
    Failed {
        error: String,
    },
}

/// Serializable view of the daemon-owned update state.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UpdateStatusSnapshot {
    pub app_version: String,
    pub status: UpdateStatus,
    pub dismissed: bool,
    pub is_homebrew: bool,
}

/// The one operation an `UpdateInfo` may have in flight. Checks, downloads,
/// installs and reverts all claim this slot, so none can clean up or overwrite
/// the artifacts another is using.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum UpdateOp {
    ManualCheck,
    BackgroundCheck,
    Install,
}

struct UpdateInfoInner {
    status: UpdateStatus,
    dismissed: bool,
    is_homebrew: bool,
    active: Option<UpdateOp>,
}

fn is_in_flight(status: &UpdateStatus) -> bool {
    matches!(
        status,
        UpdateStatus::Checking | UpdateStatus::Downloading { .. } | UpdateStatus::Installing { .. }
    )
}

/// Thread-safe shared update state, readable from any thread/view.
#[derive(Clone)]
pub struct UpdateInfo {
    inner: Arc<Mutex<UpdateInfoInner>>,
    cancel_token: Arc<AtomicU64>,
    app_version: Arc<String>,
}

impl UpdateInfo {
    pub fn new(app_version: String) -> Self {
        Self {
            inner: Arc::new(Mutex::new(UpdateInfoInner {
                status: UpdateStatus::Idle,
                dismissed: false,
                is_homebrew: is_homebrew_install(),
                active: None,
            })),
            cancel_token: Arc::new(AtomicU64::new(0)),
            app_version: Arc::new(app_version),
        }
    }

    pub fn app_version(&self) -> String {
        (*self.app_version).clone()
    }

    pub fn status(&self) -> UpdateStatus {
        self.inner.lock().status.clone()
    }

    pub fn snapshot(&self) -> UpdateStatusSnapshot {
        let inner = self.inner.lock();
        UpdateStatusSnapshot {
            app_version: self.app_version(),
            status: inner.status.clone(),
            dismissed: inner.dismissed,
            is_homebrew: inner.is_homebrew,
        }
    }

    pub fn apply_snapshot(&self, snapshot: UpdateStatusSnapshot) {
        let mut inner = self.inner.lock();
        inner.status = snapshot.status;
        inner.dismissed = snapshot.dismissed;
        inner.is_homebrew = snapshot.is_homebrew;
    }

    pub fn set_status(&self, status: UpdateStatus) {
        let mut inner = self.inner.lock();
        if matches!(
            status,
            UpdateStatus::Available { .. }
                | UpdateStatus::Downloading { .. }
                | UpdateStatus::Ready { .. }
                | UpdateStatus::Installing { .. }
                | UpdateStatus::ReadyToRestart { .. }
                | UpdateStatus::BrewUpdate { .. }
                | UpdateStatus::Failed { .. }
        ) {
            inner.dismissed = false;
        }
        inner.status = status;
    }

    pub fn is_homebrew(&self) -> bool {
        self.inner.lock().is_homebrew
    }

    pub fn is_dismissed(&self) -> bool {
        self.inner.lock().dismissed
    }

    pub fn dismiss(&self) {
        self.inner.lock().dismissed = true;
    }

    /// Reserve the update state for a user-initiated check or revert.
    pub fn try_start_manual(&self) -> bool {
        let mut inner = self.inner.lock();
        if inner.active.is_some() || is_in_flight(&inner.status) {
            return false;
        }
        inner.active = Some(UpdateOp::ManualCheck);
        inner.dismissed = false;
        true
    }

    pub fn is_manual_active(&self) -> bool {
        self.inner.lock().active == Some(UpdateOp::ManualCheck)
    }

    /// Reserve the update state for the background check loop.
    pub fn try_start(&self) -> Option<u64> {
        let mut inner = self.inner.lock();
        if inner.active.is_some() || is_in_flight(&inner.status) {
            return None;
        }
        inner.active = Some(UpdateOp::BackgroundCheck);
        Some(self.cancel_token.load(Ordering::SeqCst))
    }

    /// Claim the downloaded archive and move to `Installing` under one lock, so
    /// two callers cannot both read `Ready` and both start installing it.
    pub fn try_start_install(&self) -> Option<(UpdateReservation, String, std::path::PathBuf)> {
        let mut inner = self.inner.lock();
        if inner.active.is_some() {
            return None;
        }
        let UpdateStatus::Ready { version, path } = inner.status.clone() else {
            return None;
        };
        inner.active = Some(UpdateOp::Install);
        inner.dismissed = false;
        inner.status = UpdateStatus::Installing {
            version: version.clone(),
        };
        drop(inner);
        Some((self.reservation(UpdateOp::Install), version, path))
    }

    /// Take ownership of a reservation an HTTP route already claimed, so the
    /// worker frees it even if it panics or its task is dropped.
    pub fn adopt_manual(&self) -> UpdateReservation {
        self.reservation(UpdateOp::ManualCheck)
    }

    pub fn adopt_background(&self, token: u64) -> UpdateReservation {
        UpdateReservation {
            info: self.clone(),
            op: UpdateOp::BackgroundCheck,
            token,
        }
    }

    /// A downloaded or installed update is waiting. Re-checking would download
    /// over its artifacts and turn the `.old` rollback target into a copy of the
    /// version being installed.
    pub fn has_staged_update(&self) -> bool {
        matches!(
            self.inner.lock().status,
            UpdateStatus::Ready { .. } | UpdateStatus::ReadyToRestart { .. }
        )
    }

    fn reservation(&self, op: UpdateOp) -> UpdateReservation {
        UpdateReservation {
            info: self.clone(),
            op,
            token: self.cancel_token.load(Ordering::SeqCst),
        }
    }

    fn release(&self, op: UpdateOp) {
        let mut inner = self.inner.lock();
        if inner.active == Some(op) {
            inner.active = None;
        }
    }

    pub fn cancel(&self) {
        self.cancel_token.fetch_add(1, Ordering::SeqCst);
        self.release(UpdateOp::BackgroundCheck);
    }

    pub fn is_cancelled(&self, token: u64) -> bool {
        self.cancel_token.load(Ordering::SeqCst) != token
    }

    pub fn current_token(&self) -> u64 {
        self.cancel_token.load(Ordering::SeqCst)
    }
}

/// Holds the reservation for the length of one operation. Dropping it — on
/// return, on panic, or when the worker's task is cancelled — frees the slot.
pub struct UpdateReservation {
    info: UpdateInfo,
    op: UpdateOp,
    token: u64,
}

impl Drop for UpdateReservation {
    fn drop(&mut self) {
        // A cancelled background check must not free a reservation that a newer
        // one has since taken.
        if self.op == UpdateOp::BackgroundCheck
            && self.info.cancel_token.load(Ordering::SeqCst) != self.token
        {
            return;
        }
        self.info.release(self.op);
    }
}

/// Detect if running from a Homebrew installation.
pub fn is_homebrew_install() -> bool {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.canonicalize().ok())
        .map(|p| {
            let s = p.to_string_lossy();
            s.contains("/Caskroom/") || s.contains("/Cellar/")
        })
        .unwrap_or(false)
}

/// GPUI global wrapper for UpdateInfo.
#[derive(Clone)]
pub struct GlobalUpdateInfo(pub UpdateInfo);

#[cfg(feature = "gpui-ui")]
impl Global for GlobalUpdateInfo {}

#[cfg(feature = "gpui-ui")]
fn theme(cx: &App) -> ThemeColors {
    okena_extensions::theme(cx)
}

#[cfg(feature = "gpui-ui")]
fn open_url(url: &str) {
    okena_core::process::open_url(url);
}

/// Status bar widget that shows update status.
#[cfg(feature = "gpui-ui")]
pub struct UpdateStatusWidget {
    _subscription: Option<Subscription>,
    restarting: bool,
}

#[cfg(feature = "gpui-ui")]
impl UpdateStatusWidget {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let subscription = cx
            .try_global::<crate::GlobalLocalBuild>()
            .map(|local| local.0.clone())
            .map(|state| cx.observe(&state, |_this, _state, cx| cx.notify()));

        // Installed builds mirror daemon-owned update state into this process.
        if subscription.is_none()
            && let Some(gui) = cx.try_global::<GlobalUpdateInfo>()
        {
            let info = gui.0.clone();
            crate::update_checker::start_update_status_poll(info, cx);
        }

        Self {
            _subscription: subscription,
            restarting: false,
        }
    }
}

#[cfg(feature = "gpui-ui")]
impl Render for UpdateStatusWidget {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(local) = cx
            .try_global::<crate::GlobalLocalBuild>()
            .map(|global| global.0.clone())
        {
            let t = theme(cx);
            let state = local.read(cx);
            let text = |label: String, color| {
                div()
                    .px(px(6.0))
                    .py(px(1.0))
                    .text_color(color)
                    .text_size(ui_text_sm(cx))
                    .child(label)
                    .into_any_element()
            };

            if state.daemon_ui_owned() == Some(false) {
                return text(
                    "Local build · external daemon".to_string(),
                    rgb(t.text_muted),
                );
            }

            return match state.status() {
                crate::LocalBuildStatus::Idle => div()
                    .id("local-rebuild")
                    .cursor_pointer()
                    .px(px(6.0))
                    .py(px(1.0))
                    .text_color(rgb(t.term_green))
                    .text_size(ui_text_sm(cx))
                    .child("Local build · Rebuild")
                    .on_click(|_, window, cx| {
                        window.dispatch_action(Box::new(crate::RebuildLocal), cx);
                    })
                    .into_any_element(),
                crate::LocalBuildStatus::Building => {
                    text("Building release…".to_string(), rgb(t.term_yellow))
                }
                crate::LocalBuildStatus::ReadyToRestart => div()
                    .id("local-restart")
                    .cursor_pointer()
                    .px(px(6.0))
                    .py(px(1.0))
                    .text_color(rgb(t.term_green))
                    .text_size(ui_text_sm(cx))
                    .child("Local build · Restart")
                    .on_click(|_, window, cx| {
                        window.dispatch_action(Box::new(crate::RestartLocalBuild), cx);
                    })
                    .into_any_element(),
                crate::LocalBuildStatus::RestartingDaemon => {
                    text("Restarting daemon…".to_string(), rgb(t.term_yellow))
                }
                crate::LocalBuildStatus::RestartingApp => {
                    text("Restarting Okena…".to_string(), rgb(t.term_yellow))
                }
                crate::LocalBuildStatus::Failed { error } => h_flex()
                    .id("local-rebuild-failed")
                    .gap(px(6.0))
                    .items_center()
                    .text_size(ui_text_sm(cx))
                    .child(
                        div()
                            .text_color(rgb(t.term_red))
                            .child(format!("Rebuild failed: {error}")),
                    )
                    .child(
                        div()
                            .id("local-rebuild-retry")
                            .cursor_pointer()
                            .text_color(rgb(t.text_muted))
                            .hover(|s| s.text_color(rgb(t.text_primary)))
                            .child("Retry")
                            .on_click(|_, window, cx| {
                                window.dispatch_action(Box::new(crate::RebuildLocal), cx);
                            }),
                    )
                    .into_any_element(),
            };
        }

        let Some(update_info) = cx.try_global::<GlobalUpdateInfo>() else {
            return div().size_0().into_any_element();
        };
        let info = &update_info.0;
        if info.is_dismissed() {
            return div().size_0().into_any_element();
        }

        let t = theme(cx);

        if self.restarting {
            return div()
                .px(px(6.0))
                .py(px(1.0))
                .text_color(rgb(t.term_yellow))
                .text_size(ui_text_sm(cx))
                .child("Restarting Okena…")
                .into_any_element();
        }

        match info.status() {
            UpdateStatus::Ready { version, .. } => {
                let release_url = format!(
                    "https://github.com/contember/okena/releases/tag/v{}",
                    version
                );
                h_flex()
                    .id("update-ready")
                    .gap(px(6.0))
                    .items_center()
                    .text_size(ui_text_sm(cx))
                    .child(
                        div()
                            .id("update-install")
                            .cursor_pointer()
                            .text_color(rgb(t.term_green))
                            .child("New version available")
                            .on_click(cx.listener(|_this, _, _window, cx| {
                                if let Some(gui) = cx.try_global::<GlobalUpdateInfo>() {
                                    let info = gui.0.clone();
                                    cx.spawn(async move |this, cx| {
                                        match smol::unblock(crate::daemon_client::request_install)
                                            .await
                                        {
                                            Ok(snapshot) => info.apply_snapshot(snapshot),
                                            Err(e) => info.set_status(UpdateStatus::Failed {
                                                error: e.to_string(),
                                            }),
                                        }
                                        let _ = this.update(cx, |_, cx| cx.notify());
                                    })
                                    .detach();
                                }
                            })),
                    )
                    .child(
                        div()
                            .id("whats-new")
                            .cursor_pointer()
                            .text_color(rgb(t.text_muted))
                            .hover(|s| s.text_color(rgb(t.text_primary)))
                            .child("What's new")
                            .on_click(move |_, _, _cx| {
                                open_url(&release_url);
                            }),
                    )
                    .into_any_element()
            }
            UpdateStatus::Installing { version } => div()
                .px(px(6.0))
                .py(px(1.0))
                .text_color(rgb(t.term_yellow))
                .text_size(ui_text_sm(cx))
                .child(format!("Installing v{}...", version))
                .into_any_element(),
            UpdateStatus::ReadyToRestart { version, .. } => div()
                .id("update-restart")
                .cursor_pointer()
                .px(px(6.0))
                .py(px(1.0))
                .text_color(rgb(t.term_green))
                .text_size(ui_text_sm(cx))
                .child(format!("Restart into v{version}"))
                // The daemon restarts too, so every PTY dies — say so before the click.
                .tooltip(|window, cx| {
                    Tooltip::new("Restarts Okena and the daemon; active terminal sessions end")
                        .build(window, cx)
                })
                .on_click(cx.listener(|this, _, _, cx| {
                    if this.restarting {
                        return;
                    }
                    this.restarting = true;
                    cx.notify();
                    let Some(global) = cx.try_global::<GlobalUpdateInfo>() else {
                        this.restarting = false;
                        return;
                    };
                    let info = global.0.clone();
                    cx.spawn(async move |this, cx| {
                        let failure = match smol::unblock(
                            crate::daemon_client::restart_daemon_and_wait,
                        )
                        .await
                        {
                            Ok(()) => this
                                .update(cx, |_this, cx| crate::installer::restart_app(cx))
                                .ok()
                                .and_then(Result::err),
                            Err(error) => Some(error),
                        };
                        if let Some(error) = failure {
                            info.set_status(UpdateStatus::Failed {
                                error: error.to_string(),
                            });
                            let _ = this.update(cx, |this, cx| {
                                this.restarting = false;
                                cx.notify();
                            });
                        }
                    })
                    .detach();
                }))
                .into_any_element(),
            UpdateStatus::Downloading { version, progress } => h_flex()
                .gap(px(4.0))
                .child(
                    div()
                        .text_color(rgb(t.term_yellow))
                        .text_size(ui_text_sm(cx))
                        .child(format!("Downloading v{}... {}%", version, progress)),
                )
                .into_any_element(),
            UpdateStatus::Checking => div()
                .px(px(6.0))
                .py(px(1.0))
                .text_color(rgb(t.text_muted))
                .text_size(ui_text_sm(cx))
                .child("Checking for updates...")
                .into_any_element(),
            UpdateStatus::Failed { ref error } => {
                let info_dismiss = info.clone();
                div()
                    .id("update-failed")
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .child(
                        div()
                            .text_color(rgb(t.term_red))
                            .text_size(ui_text_sm(cx))
                            .child(format!("Update failed: {}", error)),
                    )
                    .child(
                        div()
                            .id("update-failed-dismiss")
                            .cursor_pointer()
                            .text_color(rgb(t.text_muted))
                            .text_size(ui_text_sm(cx))
                            .child("x")
                            .on_click(move |_, _, _cx| {
                                info_dismiss.dismiss();
                                let _ = crate::daemon_client::request_dismiss();
                            }),
                    )
                    .into_any_element()
            }
            UpdateStatus::BrewUpdate { version } => {
                let info_dismiss = info.clone();
                div()
                    .id("update-brew")
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .child(
                        div()
                            .text_color(rgb(t.text_muted))
                            .text_size(ui_text_sm(cx))
                            .child(format!("v{} — brew upgrade okena", version)),
                    )
                    .child(
                        div()
                            .id("update-dismiss")
                            .cursor_pointer()
                            .text_color(rgb(t.text_muted))
                            .text_size(ui_text_sm(cx))
                            .child("x")
                            .on_click(move |_, _, _cx| {
                                info_dismiss.dismiss();
                                let _ = crate::daemon_client::request_dismiss();
                            }),
                    )
                    .into_any_element()
            }
            _ => div().size_0().into_any_element(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{UpdateInfo, UpdateStatus};
    use std::panic::AssertUnwindSafe;
    use std::path::PathBuf;

    fn info() -> UpdateInfo {
        UpdateInfo::new("0.1.0".to_string())
    }

    fn ready(info: &UpdateInfo) {
        info.set_status(UpdateStatus::Ready {
            version: "0.2.0".to_string(),
            path: PathBuf::from("/tmp/okena-update.tar.gz"),
        });
    }

    #[test]
    fn only_one_caller_can_start_the_install() {
        let first = info();
        let second = first.clone();
        ready(&first);

        let (_reservation, version, _path) =
            first.try_start_install().expect("first caller installs");
        assert_eq!(version, "0.2.0");
        assert!(matches!(first.status(), UpdateStatus::Installing { .. }));
        assert!(second.try_start_install().is_none());
    }

    #[test]
    fn install_is_refused_unless_an_archive_is_ready() {
        let info = info();
        assert!(info.try_start_install().is_none());
        info.set_status(UpdateStatus::Downloading {
            version: "0.2.0".to_string(),
            progress: 10,
        });
        assert!(info.try_start_install().is_none());
        info.set_status(UpdateStatus::Failed {
            error: "boom".to_string(),
        });
        assert!(info.try_start_install().is_none());
    }

    #[test]
    fn an_install_blocks_every_check() {
        let info = info();
        ready(&info);
        let reservation = info.try_start_install().expect("install reservation");

        assert!(!info.try_start_manual());
        assert!(info.try_start().is_none());

        info.set_status(UpdateStatus::ReadyToRestart {
            version: "0.2.0".to_string(),
            config_restore: None,
        });
        drop(reservation);
        // A pending restart still allows a revert, which enters via try_start_manual.
        assert!(info.try_start_manual());
    }

    #[test]
    fn a_check_reservation_cannot_release_an_install() {
        let info = info();
        ready(&info);
        let _install = info.try_start_install().expect("install reservation");

        drop(info.adopt_manual());
        drop(info.adopt_background(info.current_token()));

        assert!(!info.try_start_manual());
        assert!(info.try_start().is_none());
    }

    #[test]
    fn a_check_is_refused_while_installing() {
        let info = info();
        info.set_status(UpdateStatus::Installing {
            version: "0.2.0".to_string(),
        });
        assert!(!info.try_start_manual());
        assert!(info.try_start().is_none());
    }

    #[test]
    fn manual_and_background_checks_exclude_each_other() {
        let info = info();
        let token = info.try_start().expect("background reservation");
        assert!(!info.try_start_manual());
        drop(info.adopt_background(token));

        assert!(info.try_start_manual());
        assert!(info.try_start().is_none());
        drop(info.adopt_manual());
        assert!(info.try_start().is_some());
    }

    #[test]
    fn a_panicking_install_frees_the_slot() {
        let info = info();
        ready(&info);

        let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| {
            let _reservation = info.try_start_install().expect("install reservation");
            panic!("install worker died");
        }));
        assert!(outcome.is_err());

        // The panic latches the status; what must not leak is the reservation.
        info.set_status(UpdateStatus::Failed {
            error: "worker panicked".to_string(),
        });
        assert!(info.try_start_manual());
    }

    #[test]
    fn a_panicking_background_check_does_not_block_an_install() {
        let info = info();
        let token = info.try_start().expect("background reservation");

        let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| {
            let _reservation = info.adopt_background(token);
            panic!("check worker died");
        }));
        assert!(outcome.is_err());

        ready(&info);
        assert!(info.try_start_install().is_some());
    }

    #[test]
    fn a_staged_update_is_recognised() {
        let info = info();
        assert!(!info.has_staged_update());
        ready(&info);
        assert!(info.has_staged_update());
        info.set_status(UpdateStatus::ReadyToRestart {
            version: "0.2.0".to_string(),
            config_restore: None,
        });
        assert!(info.has_staged_update());
        info.set_status(UpdateStatus::Idle);
        assert!(!info.has_staged_update());
    }
}
