use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
#[path = "preference.rs"]
mod preference;
use preference::Preference;
use std::path::PathBuf;
use std::sync::Arc;

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
        /// Installed from the version history rather than as the channel's update.
        #[serde(default)]
        revert: bool,
    },
    BrewUpdate {
        version: String,
    },
    Failed {
        error: String,
    },
}

/// The stream selected for this installation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UpdateChannel {
    #[default]
    Release,
    Nightly,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UpdateStatusSnapshot {
    pub app_version: String,
    #[serde(default)]
    pub running_version: Option<String>,
    pub status: UpdateStatus,
    pub dismissed: bool,
    pub is_homebrew: bool,
    #[serde(default)]
    pub channel: UpdateChannel,
    #[serde(default)]
    pub channel_supported: bool,
    #[serde(default)]
    pub stable_return: bool,
    #[serde(default)]
    pub stable_return_config: Option<okena_core::profiles::ConfigSnapshot>,
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
    generation: u64,
    preference: Preference,
    running_version: Option<String>,
    install_lock: Option<std::fs::File>,
    channel_supported: bool,
    stable_return: bool,
    stable_return_config: Option<okena_core::profiles::ConfigSnapshot>,
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
    channel_path: Option<Arc<PathBuf>>,
    app_version: Arc<String>,
    release_notes: crate::checker::ReleaseNotesCache,
}

impl UpdateInfo {
    pub fn new(app_version: String) -> Self {
        Self::with_channel_path(app_version, None, true)
    }

    pub fn for_daemon(app_version: String) -> Self {
        let path = okena_core::profiles::try_current().and_then(|paths| {
            let exe = crate::installer::remember_launch_path().ok()?;
            Some(preference::path(&paths.config_root, exe.parent()?))
        });
        Self::with_channel_path(app_version, path, crate::detect_local_checkout().is_none())
    }

    #[cfg(feature = "gpui-ui")]
    pub(crate) fn for_desktop(app_version: String) -> Self {
        Self::with_channel_path(app_version, None, false)
    }

    fn with_channel_path(app_version: String, path: Option<PathBuf>, installed: bool) -> Self {
        let is_homebrew = is_homebrew_install();
        let channel_supported = installed && !is_homebrew;
        let preference = path
            .as_deref()
            .filter(|_| channel_supported)
            .map(|path| {
                preference::load(path, Preference::initial()).unwrap_or_else(|error| {
                    log::warn!("Could not read update channel: {error}");
                    Preference::initial()
                })
            })
            .unwrap_or_default();
        Self {
            inner: Arc::new(Mutex::new(UpdateInfoInner {
                status: UpdateStatus::Idle,
                dismissed: false,
                is_homebrew,
                active: None,
                generation: 0,
                preference,
                running_version: Some(if okena_core::build_info::BUILD_INFO.nightly.is_some() {
                    okena_core::build_info::BUILD_INFO.version.clone()
                } else {
                    app_version.clone()
                }),
                install_lock: None,
                channel_supported,
                stable_return: false,
                stable_return_config: None,
            })),
            channel_path: path.map(Arc::new),
            app_version: Arc::new(app_version),
            release_notes: Default::default(),
        }
    }

    pub fn app_version(&self) -> String {
        (*self.app_version).clone()
    }

    pub fn release_notes_cache(&self) -> &crate::checker::ReleaseNotesCache {
        &self.release_notes
    }

    pub fn status(&self) -> UpdateStatus {
        self.inner.lock().status.clone()
    }

    pub fn snapshot(&self) -> UpdateStatusSnapshot {
        let inner = self.inner.lock();
        UpdateStatusSnapshot {
            app_version: self.app_version(),
            running_version: inner.running_version.clone(),
            status: inner.status.clone(),
            dismissed: inner.dismissed,
            is_homebrew: inner.is_homebrew,
            channel: inner.preference.channel,
            channel_supported: inner.channel_supported,
            stable_return: inner.stable_return,
            stable_return_config: inner.stable_return_config.clone(),
        }
    }

    pub fn apply_snapshot(&self, snapshot: UpdateStatusSnapshot) {
        let mut inner = self.inner.lock();
        inner.status = snapshot.status;
        inner.running_version = snapshot.running_version;
        inner.dismissed = snapshot.dismissed;
        inner.is_homebrew = snapshot.is_homebrew;
        inner.preference.channel = snapshot.channel;
        inner.channel_supported = snapshot.channel_supported;
        inner.stable_return = snapshot.stable_return;
        inner.stable_return_config = snapshot.stable_return_config;
    }

    pub fn set_status(&self, status: UpdateStatus) {
        Self::update_status(&mut self.inner.lock(), status);
    }

    fn update_status(inner: &mut UpdateInfoInner, status: UpdateStatus) {
        inner.stable_return = false;
        inner.stable_return_config = None;
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
        if !matches!(
            status,
            UpdateStatus::Installing { .. } | UpdateStatus::ReadyToRestart { .. }
        ) {
            inner.install_lock = None;
        }
        inner.status = status;
    }

    pub fn set_status_if_current(&self, token: u64, status: UpdateStatus) -> bool {
        let mut inner = self.inner.lock();
        if inner.generation != token {
            return false;
        }
        Self::update_status(&mut inner, status);
        true
    }

    pub(crate) fn stage_download(
        &self,
        token: u64,
        version: String,
        path: PathBuf,
        stable_return: bool,
    ) {
        self.refresh_channel();
        let config = if stable_return {
            okena_core::profiles::try_current().and_then(|paths| {
                okena_core::profiles::config_snapshot_for_version(paths, &version)
            })
        } else {
            None
        };
        let mut inner = self.inner.lock();
        if inner.generation != token {
            drop(inner);
            crate::downloader::discard_download(&path);
            return;
        }
        Self::update_status(&mut inner, UpdateStatus::Ready { version, path });
        inner.stable_return = stable_return;
        inner.stable_return_config = config;
    }

    fn lock_installation(&self) -> anyhow::Result<Option<std::fs::File>> {
        self.channel_path
            .as_ref()
            .map(|path| preference::lock(path))
            .transpose()
    }

    fn stored_preference(&self, inner: &UpdateInfoInner) -> anyhow::Result<Preference> {
        match &self.channel_path {
            Some(path) => preference::load(path, Preference::initial()),
            None => Ok(inner.preference),
        }
    }

    fn persist_preference(&self, preference: Preference) -> anyhow::Result<()> {
        if let Some(path) = &self.channel_path {
            preference::save(path, preference)?;
        }
        Ok(())
    }

    fn accept_preference(inner: &mut UpdateInfoInner, preference: Preference) {
        let old = std::mem::replace(&mut inner.status, UpdateStatus::Idle);
        inner.generation += 1;
        inner.preference = preference;
        inner.active = None;
        inner.dismissed = false;
        inner.stable_return = false;
        inner.stable_return_config = None;
        if let UpdateStatus::Ready { path, .. } = old {
            crate::downloader::discard_download(&path);
        }
    }

    /// Other profiles may change the stream for these same binaries.
    pub fn refresh_channel(&self) {
        let mut inner = self.inner.lock();
        if !inner.channel_supported || inner.install_lock.is_some() {
            return;
        }
        if let Ok(stored) = self.stored_preference(&inner)
            && stored != inner.preference
        {
            Self::accept_preference(&mut inner, stored);
        }
    }

    /// Persist the preference, invalidate earlier workers and claim the next check atomically.
    pub fn change_channel(&self, channel: UpdateChannel) -> anyhow::Result<Option<u64>> {
        let mut inner = self.inner.lock();
        anyhow::ensure!(
            inner.channel_supported,
            "channel selection requires an Okena-managed installation"
        );
        anyhow::ensure!(
            !matches!(
                inner.status,
                UpdateStatus::Installing { .. } | UpdateStatus::ReadyToRestart { .. }
            ),
            "restart before changing update channels"
        );
        let _lock = self.lock_installation()?;
        let mut stored = self.stored_preference(&inner)?;
        if stored.channel != channel {
            stored = stored.selecting(channel);
            self.persist_preference(stored)?;
        }
        if inner.preference == stored {
            return Ok(None);
        }
        Self::accept_preference(&mut inner, stored);
        inner.active = Some(UpdateOp::ManualCheck);
        inner.status = UpdateStatus::Checking;
        Ok(Some(inner.generation))
    }

    fn current_install_lock(
        &self,
        inner: &mut UpdateInfoInner,
    ) -> anyhow::Result<Option<std::fs::File>> {
        let lock = self.lock_installation()?;
        let stored = self.stored_preference(inner)?;
        if inner.preference != stored {
            Self::accept_preference(inner, stored);
            anyhow::bail!("the update channel changed in another profile; check for updates again");
        }
        Ok(lock)
    }

    pub fn clear_background_failure(&self, token: u64, message: &str) {
        let mut inner = self.inner.lock();
        if inner.generation == token
            && inner.active.is_none()
            && matches!(&inner.status, UpdateStatus::Failed {error} if error == message)
        {
            Self::update_status(&mut inner, UpdateStatus::Idle);
        }
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
        self.try_start_manual_token().is_some()
    }

    pub fn try_start_manual_token(&self) -> Option<u64> {
        let mut inner = self.inner.lock();
        if inner.active.is_some() || is_in_flight(&inner.status) {
            return None;
        }
        inner.active = Some(UpdateOp::ManualCheck);
        inner.dismissed = false;
        Some(inner.generation)
    }

    pub fn is_manual_active(&self) -> bool {
        self.inner.lock().active == Some(UpdateOp::ManualCheck)
    }

    /// A confirmed historical revert supersedes any downloaded candidate.
    pub fn try_start_revert(&self, target_channel: UpdateChannel) -> anyhow::Result<u64> {
        let mut inner = self.inner.lock();
        anyhow::ensure!(
            inner.preference.channel == target_channel,
            "this version belongs to a different update channel; refresh the history"
        );
        if inner.active.is_some()
            || is_in_flight(&inner.status)
            || matches!(inner.status, UpdateStatus::ReadyToRestart { .. })
        {
            anyhow::bail!(
                "another update is in progress; finish the installation and restart before reverting"
            );
        }
        let old = std::mem::replace(&mut inner.status, UpdateStatus::Checking);
        inner.active = Some(UpdateOp::ManualCheck);
        inner.dismissed = false;
        inner.stable_return = false;
        inner.stable_return_config = None;
        let token = inner.generation;
        drop(inner);
        if let UpdateStatus::Ready { path, .. } = old {
            crate::downloader::discard_download(&path);
        }
        Ok(token)
    }

    /// Reserve the update state for the background check loop.
    pub fn try_start(&self) -> Option<u64> {
        let mut inner = self.inner.lock();
        if inner.active.is_some() || is_in_flight(&inner.status) {
            return None;
        }
        inner.active = Some(UpdateOp::BackgroundCheck);
        Some(inner.generation)
    }

    /// Claim the downloaded archive and move to `Installing` under one lock, so
    /// two callers cannot both read `Ready` and both start installing it.
    pub fn try_start_install(&self) -> Option<(UpdateReservation, String, std::path::PathBuf)> {
        self.claim_install(None, false).ok()
    }

    pub fn claim_install(
        &self,
        confirmed_version: Option<&str>,
        restore_config: bool,
    ) -> anyhow::Result<(UpdateReservation, String, PathBuf)> {
        let mut inner = self.inner.lock();
        anyhow::ensure!(inner.active.is_none(), "another update is in progress");
        let UpdateStatus::Ready { version, path } = inner.status.clone() else {
            anyhow::bail!("no update is ready");
        };
        if inner.stable_return || confirmed_version.is_some() {
            anyhow::ensure!(
                inner.stable_return && confirmed_version == Some(version.as_str()),
                "confirm the selected stable release before installing it"
            );
        }
        anyhow::ensure!(
            !restore_config || inner.stable_return_config.is_some(),
            "no configuration snapshot is available for this version"
        );
        inner.install_lock = self.current_install_lock(&mut inner)?;
        inner.active = Some(UpdateOp::Install);
        inner.dismissed = false;
        Self::update_status(
            &mut inner,
            UpdateStatus::Installing {
                version: version.clone(),
            },
        );
        Ok((
            self.reservation(UpdateOp::Install, inner.generation),
            version,
            path,
        ))
    }

    /// Take ownership of a reservation an HTTP route already claimed, so the
    /// worker frees it even if it panics or its task is dropped.
    pub fn adopt_manual(&self, token: u64) -> UpdateReservation {
        self.reservation(UpdateOp::ManualCheck, token)
    }

    pub fn adopt_background(&self, token: u64) -> UpdateReservation {
        self.reservation(UpdateOp::BackgroundCheck, token)
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

    fn reservation(&self, op: UpdateOp, token: u64) -> UpdateReservation {
        UpdateReservation {
            info: self.clone(),
            op,
            token,
        }
    }

    fn release(&self, op: UpdateOp, token: u64) {
        let mut inner = self.inner.lock();
        if inner.active == Some(op) && inner.generation == token {
            inner.active = None;
        }
    }

    pub(crate) fn begin_revert_install(&self, token: u64, version: String) -> anyhow::Result<()> {
        let mut inner = self.inner.lock();
        anyhow::ensure!(
            inner.generation == token && inner.active == Some(UpdateOp::ManualCheck),
            "version revert cancelled"
        );
        let lock = self.current_install_lock(&mut inner)?;
        inner.install_lock = lock;
        Self::update_status(&mut inner, UpdateStatus::Installing { version });
        Ok(())
    }

    pub fn is_cancelled(&self, token: u64) -> bool {
        self.current_token() != token
    }

    pub fn current_token(&self) -> u64 {
        self.inner.lock().generation
    }
}

/// Holds the reservation for the length of one operation. Dropping it — on
/// return, on panic, or when the worker's task is cancelled — frees the slot.
pub struct UpdateReservation {
    info: UpdateInfo,
    op: UpdateOp,
    token: u64,
}

impl UpdateReservation {
    pub(crate) fn token(&self) -> u64 {
        self.token
    }
}

impl Drop for UpdateReservation {
    fn drop(&mut self) {
        self.info.release(self.op, self.token);
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

        let snapshot = info.snapshot();
        match snapshot.status {
            UpdateStatus::Ready { version, .. } if snapshot.stable_return => div()
                .id("update-return-to-release")
                .cursor_pointer()
                .text_color(rgb(t.term_green))
                .text_size(ui_text_sm(cx))
                .child(format!("Review Release v{version} installation…"))
                .on_click(|_, window, cx| {
                    window.dispatch_action(Box::new(crate::OpenUpdateSettings), cx);
                })
                .into_any_element(),
            UpdateStatus::Ready { version, .. } => {
                let tag = crate::update_version_label(&version);
                let release_url = format!("https://github.com/contember/okena/releases/tag/{tag}");
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
                .child(format!(
                    "Installing {}...",
                    crate::update_version_label(&version)
                ))
                .into_any_element(),
            UpdateStatus::ReadyToRestart { version, .. } => div()
                .id("update-restart")
                .cursor_pointer()
                .px(px(6.0))
                .py(px(1.0))
                .text_color(rgb(t.term_green))
                .text_size(ui_text_sm(cx))
                .child(format!(
                    "Restart into {}",
                    crate::update_version_label(&version)
                ))
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
                        .child(format!(
                            "Downloading {}... {progress}%",
                            crate::update_version_label(&version)
                        )),
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
    fn changing_channel_invalidates_a_download_and_its_reservation() {
        let info = info();
        let token = info.try_start().unwrap();
        let old_worker = info.adopt_background(token);
        info.set_status(UpdateStatus::Downloading {
            version: "0.2.0".into(),
            progress: 50,
        });
        let current = info
            .change_channel(super::UpdateChannel::Nightly)
            .unwrap()
            .unwrap();
        assert!(!info.set_status_if_current(token, UpdateStatus::Idle));
        drop(old_worker);
        assert!(info.try_start().is_none());
        assert!(info.set_status_if_current(current, UpdateStatus::Checking));
        assert_eq!(info.snapshot().channel, super::UpdateChannel::Nightly);
    }

    #[test]
    fn channel_is_locked_until_the_installed_update_restarts() {
        let info = info();
        ready(&info);
        let install = info.try_start_install().unwrap();
        assert!(info.change_channel(super::UpdateChannel::Nightly).is_err());
        info.set_status(UpdateStatus::ReadyToRestart {
            version: "0.2.0".into(),
            config_restore: None,
            revert: false,
        });
        drop(install);
        assert!(info.change_channel(super::UpdateChannel::Nightly).is_err());
    }

    #[test]
    fn returning_to_stable_requires_confirmation_for_that_candidate() {
        let info = info();
        info.stage_download(
            info.current_token(),
            "0.1.0".into(),
            PathBuf::from("unused"),
            true,
        );
        assert!(info.try_start_install().is_none());
        assert!(info.claim_install(Some("0.2.0"), false).is_err());
        assert!(info.claim_install(Some("0.1.0"), true).is_err());
        assert!(info.claim_install(Some("0.1.0"), false).is_ok());
    }

    #[test]
    fn install_lock_applies_to_all_profiles_but_only_for_these_binaries() {
        let dir = tempfile::tempdir().unwrap();
        let start = |install_dir: &str| {
            let path = super::preference::path(dir.path(), &dir.path().join(install_dir));
            UpdateInfo::with_channel_path("0.1.0".into(), Some(path), true)
        };
        let first = start("stable-copy");
        let second = start("stable-copy");
        let preview = start("preview-copy");
        ready(&first);
        let install = first.try_start_install().unwrap();
        assert!(
            second
                .change_channel(super::UpdateChannel::Nightly)
                .is_err()
        );
        assert!(
            preview
                .change_channel(super::UpdateChannel::Nightly)
                .is_ok()
        );
        first.set_status(UpdateStatus::ReadyToRestart {
            version: "0.2.0".into(),
            config_restore: None,
            revert: false,
        });
        drop(install);
        assert!(
            second
                .change_channel(super::UpdateChannel::Nightly)
                .is_err()
        );
        drop(first);
        assert!(second.change_channel(super::UpdateChannel::Nightly).is_ok());
        assert_eq!(
            start("stable-copy").snapshot().channel,
            super::UpdateChannel::Nightly
        );
    }

    #[test]
    fn a_truncated_preference_can_be_replaced_by_a_channel_selection() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("channel.json");
        std::fs::write(&path, b"").unwrap();
        let start = || UpdateInfo::with_channel_path("0.1.0".into(), Some(path.clone()), true);
        let info = start();
        assert_eq!(info.snapshot().channel, super::UpdateChannel::Release);
        info.change_channel(super::UpdateChannel::Nightly).unwrap();
        assert_eq!(start().snapshot().channel, super::UpdateChannel::Nightly);
    }

    #[test]
    fn reverting_a_nightly_preserves_the_nightly_channel() {
        let info = info();
        let check = info
            .change_channel(super::UpdateChannel::Nightly)
            .unwrap()
            .unwrap();
        drop(info.adopt_manual(check));
        info.set_status(UpdateStatus::Idle);
        let token = info
            .try_start_revert(super::UpdateChannel::Nightly)
            .unwrap();
        info.begin_revert_install(token, "nightly-20261121-292fa56".into())
            .unwrap();
        assert_eq!(info.snapshot().channel, super::UpdateChannel::Nightly);
    }

    #[test]
    fn stale_history_from_another_channel_cannot_replace_a_staged_update() {
        let info = info();
        ready(&info);
        assert!(
            info.try_start_revert(super::UpdateChannel::Nightly)
                .is_err()
        );
        assert!(matches!(info.status(), UpdateStatus::Ready { .. }));
    }

    #[test]
    fn another_profile_cannot_install_a_superseded_download() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("channel.json");
        let profile = || UpdateInfo::with_channel_path("0.1.0".into(), Some(path.clone()), true);
        let first = profile();
        let second = profile();
        ready(&first);
        second
            .change_channel(super::UpdateChannel::Nightly)
            .unwrap();
        assert!(first.try_start_install().is_none());
        assert_eq!(first.snapshot().channel, super::UpdateChannel::Nightly);
    }

    #[test]
    fn restarts_reload_the_selected_channel_and_legacy_installs_start_on_release() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("update-channel.json");
        let start = || UpdateInfo::with_channel_path("0.1.0".into(), Some(path.clone()), true);
        let info = start();
        assert_eq!(info.snapshot().channel, super::UpdateChannel::Release);
        info.change_channel(super::UpdateChannel::Nightly).unwrap();
        assert_eq!(start().snapshot().channel, super::UpdateChannel::Nightly);
        info.change_channel(super::UpdateChannel::Release).unwrap();
        assert_eq!(start().snapshot().channel, super::UpdateChannel::Release);
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
            revert: false,
        });
        drop(reservation);
        assert!(
            info.try_start_revert(super::UpdateChannel::Release)
                .is_err()
        );
    }

    #[test]
    fn a_check_reservation_cannot_release_an_install() {
        let info = info();
        ready(&info);
        let _install = info.try_start_install().expect("install reservation");

        drop(info.adopt_manual(info.current_token()));
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
        drop(info.adopt_manual(info.current_token()));
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
            revert: false,
        });
        assert!(info.has_staged_update());
        info.set_status(UpdateStatus::Idle);
        assert!(!info.has_staged_update());
    }
}
