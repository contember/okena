use okena_extensions::ThemeColors;
use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::h_flex;
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;


/// Status of the update process.
#[derive(Clone, Debug)]
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
    },
    BrewUpdate {
        version: String,
    },
    Failed {
        error: String,
    },
}

struct UpdateInfoInner {
    status: UpdateStatus,
    dismissed: bool,
    is_homebrew: bool,
    manual_check_active: bool,
}

/// Thread-safe shared update state, readable from any thread/view.
#[derive(Clone)]
pub struct UpdateInfo {
    inner: Arc<Mutex<UpdateInfoInner>>,
    running: Arc<AtomicBool>,
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
                manual_check_active: false,
            })),
            running: Arc::new(AtomicBool::new(false)),
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

    pub fn try_start_manual(&self) -> bool {
        let mut inner = self.inner.lock();
        if inner.manual_check_active {
            return false;
        }
        if matches!(
            inner.status,
            UpdateStatus::Checking | UpdateStatus::Downloading { .. }
        ) {
            return false;
        }
        inner.manual_check_active = true;
        inner.dismissed = false;
        true
    }

    pub fn is_manual_active(&self) -> bool {
        self.inner.lock().manual_check_active
    }

    pub fn finish_manual(&self) {
        self.inner.lock().manual_check_active = false;
    }

    pub fn try_start(&self) -> Option<u64> {
        if self.inner.lock().manual_check_active {
            return None;
        }
        if self
            .running
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            Some(self.cancel_token.load(Ordering::SeqCst))
        } else {
            None
        }
    }

    pub fn cancel(&self) {
        self.cancel_token.fetch_add(1, Ordering::SeqCst);
        self.running.store(false, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self, token: u64) -> bool {
        self.cancel_token.load(Ordering::SeqCst) != token
    }

    pub fn current_token(&self) -> u64 {
        self.cancel_token.load(Ordering::SeqCst)
    }

    pub fn mark_stopped(&self, token: u64) {
        if self.cancel_token.load(Ordering::SeqCst) == token {
            self.running.store(false, Ordering::SeqCst);
        }
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

impl Global for GlobalUpdateInfo {}

fn theme(cx: &App) -> ThemeColors {
    okena_extensions::theme(cx)
}

fn open_url(url: &str) {
    #[cfg(target_os = "macos")]
    { let _ = std::process::Command::new("open").arg(url).spawn(); }
    #[cfg(target_os = "linux")]
    { let _ = std::process::Command::new("xdg-open").arg(url).spawn(); }
    #[cfg(windows)]
    { let _ = std::process::Command::new("cmd").args(["/C", "start", "", url]).spawn(); }
}

/// Status bar widget that shows update status.
pub struct UpdateStatusWidget {
    _subscription: Option<()>,
}

impl UpdateStatusWidget {
    pub fn new(cx: &mut Context<Self>) -> Self {
        // Start the background checker when the widget is created
        if let Some(gui) = cx.try_global::<GlobalUpdateInfo>() {
            let info = gui.0.clone();
            crate::update_checker::start_update_checker(info, cx);
        }

        Self { _subscription: None }
    }
}

impl Render for UpdateStatusWidget {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(update_info) = cx.try_global::<GlobalUpdateInfo>() else {
            return div().size_0().into_any_element();
        };
        let info = &update_info.0;
        if info.is_dismissed() {
            return div().size_0().into_any_element();
        }

        let t = theme(cx);

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
                    .text_size(px(10.0))
                    .child(
                        div()
                            .id("update-install")
                            .cursor_pointer()
                            .text_color(rgb(t.term_green))
                            .child("New version available")
                            .on_click(cx.listener(|_this, _, _window, cx| {
                                if let Some(gui) = cx.try_global::<GlobalUpdateInfo>() {
                                    let info = gui.0.clone();
                                    if let UpdateStatus::Ready { version, path } = info.status() {
                                        info.set_status(UpdateStatus::Installing {
                                            version: version.clone(),
                                        });
                                        cx.notify();
                                        cx.spawn(async move |_this, cx| {
                                            let result = smol::unblock({
                                                move || crate::installer::install_update(&path)
                                            }).await;
                                            match result {
                                                Ok(_) => {
                                                    info.set_status(UpdateStatus::ReadyToRestart {
                                                        version,
                                                    });
                                                }
                                                Err(e) => {
                                                    log::error!("Install failed: {}", e);
                                                    info.set_status(UpdateStatus::Failed {
                                                        error: e.to_string(),
                                                    });
                                                }
                                            }
                                            let _ = _this.update(cx, |_, cx| cx.notify());
                                        }).detach();
                                    }
                                }
                            }))
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
                            })
                    )
                    .into_any_element()
            }
            UpdateStatus::Installing { version } => {
                div()
                    .px(px(6.0))
                    .py(px(1.0))
                    .text_color(rgb(t.term_yellow))
                    .text_size(px(10.0))
                    .child(format!("Installing v{}...", version))
                    .into_any_element()
            }
            UpdateStatus::ReadyToRestart { .. } => {
                div()
                    .id("update-restart")
                    .cursor_pointer()
                    .px(px(6.0))
                    .py(px(1.0))
                    .text_color(rgb(t.term_green))
                    .text_size(px(10.0))
                    .child("Restart to update")
                    .on_click(move |_, _, cx| {
                        crate::installer::restart_app(cx);
                    })
                    .into_any_element()
            }
            UpdateStatus::Downloading { version, progress } => {
                h_flex()
                    .gap(px(4.0))
                    .child(
                        div()
                            .text_color(rgb(t.term_yellow))
                            .text_size(px(10.0))
                            .child(format!("Downloading v{}... {}%", version, progress))
                    )
                    .into_any_element()
            }
            UpdateStatus::Checking => {
                div()
                    .px(px(6.0))
                    .py(px(1.0))
                    .text_color(rgb(t.text_muted))
                    .text_size(px(10.0))
                    .child("Checking for updates...")
                    .into_any_element()
            }
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
                            .text_size(px(10.0))
                            .child(format!("Update failed: {}", error))
                    )
                    .child(
                        div()
                            .id("update-failed-dismiss")
                            .cursor_pointer()
                            .text_color(rgb(t.text_muted))
                            .text_size(px(10.0))
                            .child("x")
                            .on_click(move |_, _, _cx| {
                                info_dismiss.dismiss();
                            })
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
                            .text_size(px(10.0))
                            .child(format!("v{} — brew upgrade okena", version))
                    )
                    .child(
                        div()
                            .id("update-dismiss")
                            .cursor_pointer()
                            .text_color(rgb(t.text_muted))
                            .text_size(px(10.0))
                            .child("x")
                            .on_click(move |_, _, _cx| {
                                info_dismiss.dismiss();
                            })
                    )
                    .into_any_element()
            }
            _ => div().size_0().into_any_element(),
        }
    }
}

