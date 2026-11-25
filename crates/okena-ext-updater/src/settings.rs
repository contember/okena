use crate::{GlobalUpdateInfo, ReleaseCatalog, RevertRelease, UpdateChannel, UpdateStatus};
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::{h_flex, v_flex};
use okena_core::build_info::BUILD_INFO;
use okena_core::profiles::ConfigSnapshot;
use okena_extensions::ThemeColors;
use okena_ui::button::{button, button_primary};
use okena_ui::settings::{section_container, section_header};
use okena_ui::toggle::toggle_switch;
use okena_ui::tokens::{ui_text, ui_text_md, ui_text_sm};

fn theme(cx: &App) -> ThemeColors {
    okena_extensions::theme(cx)
}

enum CatalogState {
    Loading,
    Loaded(ReleaseCatalog),
    Failed(String),
}

#[derive(Clone)]
enum InstallSelection {
    CatalogVersion(String),
    StableReturn(String),
}

pub struct UpdaterSettingsView {
    catalog: CatalogState,
    catalog_channel: UpdateChannel,
    catalog_request: u64,
    status_revision: u64,
    selection: Option<InstallSelection>,
    keep_config: bool,
    submitting: bool,
    restarting: bool,
    request_error: Option<String>,
}

impl UpdaterSettingsView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let catalog_channel = cx
            .try_global::<GlobalUpdateInfo>()
            .map(|global| global.0.snapshot().channel)
            .unwrap_or_default();
        let mut view = Self {
            catalog: CatalogState::Loading,
            catalog_channel,
            catalog_request: 0,
            status_revision: 0,
            selection: None,
            keep_config: false,
            submitting: false,
            restarting: false,
            request_error: None,
        };
        view.load_catalog(cx);
        Self::poll_status(cx);
        view
    }

    fn invalidate_catalog(&mut self) {
        self.catalog_request += 1;
        self.catalog = CatalogState::Loading;
    }

    fn load_catalog(&mut self, cx: &mut Context<Self>) {
        self.invalidate_catalog();
        let request = self.catalog_request;
        cx.spawn(async move |this, cx| {
            let result = smol::unblock(crate::daemon_client::fetch_releases).await;
            let _ = this.update(cx, |this, cx| {
                if this.apply_catalog_response(request, result) {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn apply_catalog_response(
        &mut self,
        request: u64,
        result: anyhow::Result<ReleaseCatalog>,
    ) -> bool {
        // An earlier request for this same channel can still be in flight.
        if request != self.catalog_request {
            return false;
        }
        self.catalog = match result {
            Ok(catalog) if catalog.channel == self.catalog_channel => CatalogState::Loaded(catalog),
            Ok(_) => CatalogState::Failed(
                "The update channel changed while release history was loading.".to_string(),
            ),
            Err(error) => CatalogState::Failed(error.to_string()),
        };
        true
    }

    fn refresh_catalog(&mut self, channel: UpdateChannel, force: bool, cx: &mut Context<Self>) {
        let channel_changed = self.catalog_channel != channel;
        if channel_changed {
            self.catalog_channel = channel;
            self.selection = None;
            self.request_error = None;
        }
        if channel_changed || force {
            self.load_catalog(cx);
        }
    }

    fn poll_status(cx: &mut Context<Self>) {
        let info = cx
            .try_global::<GlobalUpdateInfo>()
            .map(|global| global.0.clone());
        cx.spawn(async move |this, cx| {
            loop {
                let Ok(revision) = this.update(cx, |this, _| {
                    (!this.submitting).then_some(this.status_revision)
                }) else {
                    return;
                };
                let snapshot = if revision.is_some() && info.is_some() {
                    smol::unblock(crate::daemon_client::fetch_status).await.ok()
                } else {
                    None
                };
                if this
                    .update(cx, |this, cx| {
                        if revision == Some(this.status_revision)
                            && let (Some(info), Some(snapshot)) = (&info, snapshot)
                        {
                            this.refresh_catalog(snapshot.channel, false, cx);
                            info.apply_snapshot(snapshot);
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
                let delay = match &info {
                    Some(info)
                        if matches!(
                            info.status(),
                            UpdateStatus::Checking
                                | UpdateStatus::Downloading { .. }
                                | UpdateStatus::Installing { .. }
                        ) =>
                    {
                        std::time::Duration::from_millis(500)
                    }
                    _ => std::time::Duration::from_secs(5),
                };
                smol::Timer::after(delay).await;
            }
        })
        .detach();
    }

    fn selected_install(
        &self,
        cx: &App,
    ) -> Option<(InstallSelection, Option<ConfigSnapshot>, bool, bool)> {
        let selection = self.selection.clone()?;
        let (config, legacy_nightly, newer) = match &selection {
            InstallSelection::CatalogVersion(version) => {
                let CatalogState::Loaded(catalog) = &self.catalog else {
                    return None;
                };
                let release = catalog
                    .releases
                    .iter()
                    .find(|release| &release.version == version)?;
                (
                    release.config_snapshot.clone(),
                    release.legacy_nightly,
                    release.newer,
                )
            }
            InstallSelection::StableReturn(version) => {
                let snapshot = cx.try_global::<GlobalUpdateInfo>()?.0.snapshot();
                if !snapshot.stable_return
                    || !matches!(snapshot.status, UpdateStatus::Ready { version: ready, .. } if &ready == version)
                {
                    return None;
                }
                (snapshot.stable_return_config, false, false)
            }
        };
        Some((selection, config, legacy_nightly, newer))
    }

    fn render_status(&self, t: &ThemeColors, cx: &App) -> Option<AnyElement> {
        let status = cx.try_global::<GlobalUpdateInfo>()?.0.status();
        let (label, color) = match status {
            UpdateStatus::Checking => ("Checking for updates…".to_string(), t.text_muted),
            UpdateStatus::Downloading { version, progress } => (
                format!(
                    "Downloading {} · {progress}%",
                    crate::update_version_label(&version)
                ),
                t.term_yellow,
            ),
            UpdateStatus::Installing { version } => (
                format!("Installing {}…", crate::update_version_label(&version)),
                t.term_yellow,
            ),
            UpdateStatus::ReadyToRestart {
                version,
                config_restore,
                ..
            } => {
                let config = config_restore
                    .map(|snapshot| format!(" · config from {}", snapshot.created_at))
                    .unwrap_or_else(|| " · keeping current config".to_string());
                (
                    format!(
                        "{} installed{config}",
                        crate::update_version_label(&version)
                    ),
                    t.term_green,
                )
            }
            UpdateStatus::Failed { error } => (format!("Update failed: {error}"), t.term_red),
            _ => return None,
        };
        Some(
            div()
                .mx(px(16.0))
                .mb(px(12.0))
                .px(px(12.0))
                .py(px(8.0))
                .rounded(px(4.0))
                .border_1()
                .border_color(rgb(t.border))
                .text_size(ui_text_sm(cx))
                .text_color(rgb(color))
                .child(label)
                .into_any_element(),
        )
    }

    fn render_current(&self, catalog: &ReleaseCatalog, t: &ThemeColors, cx: &App) -> AnyElement {
        let running = cx
            .try_global::<GlobalUpdateInfo>()
            .and_then(|info| info.0.snapshot().running_version)
            .unwrap_or_else(|| catalog.current_version.clone());
        section_container(t)
            .child(
                h_flex()
                    .px(px(12.0))
                    .py(px(10.0))
                    .gap(px(10.0))
                    .items_center()
                    .child(
                        div()
                            .w(px(12.0))
                            .h(px(12.0))
                            .rounded_full()
                            .border_1()
                            .border_color(rgb(t.term_green))
                            .child(
                                div()
                                    .m(px(3.0))
                                    .w(px(4.0))
                                    .h(px(4.0))
                                    .rounded_full()
                                    .bg(rgb(t.term_green)),
                            ),
                    )
                    .child(
                        v_flex()
                            .gap(px(2.0))
                            .child(
                                div()
                                    .font_family("monospace")
                                    .text_size(ui_text(13.0, cx))
                                    .text_color(rgb(t.text_primary))
                                    .child(crate::update_version_label(&running)),
                            )
                            .child(
                                div()
                                    .text_size(ui_text_sm(cx))
                                    .text_color(rgb(t.term_green))
                                    .child("Running version"),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn submit(
        &mut self,
        request: impl FnOnce() -> anyhow::Result<crate::UpdateStatusSnapshot> + Send + 'static,
        channel_change: bool,
        cx: &mut Context<Self>,
    ) {
        if self.submitting {
            return;
        }
        self.submitting = true;
        self.status_revision += 1;
        if channel_change {
            self.invalidate_catalog();
        }
        self.request_error = None;
        cx.notify();
        let info = cx
            .try_global::<GlobalUpdateInfo>()
            .map(|global| global.0.clone());
        cx.spawn(async move |this, cx| {
            let result = smol::unblock(request).await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(snapshot) => {
                        this.refresh_catalog(snapshot.channel, channel_change, cx);
                        if let Some(info) = info {
                            info.apply_snapshot(snapshot);
                        }
                        this.selection = None;
                    }
                    Err(error) => {
                        this.request_error = Some(error.to_string());
                        if channel_change {
                            this.load_catalog(cx);
                        }
                    }
                }
                this.submitting = false;
                cx.notify();
            });
        })
        .detach();
    }

    fn render_channel(&self, t: &ThemeColors, cx: &mut Context<Self>) -> Option<AnyElement> {
        let snapshot = cx.try_global::<GlobalUpdateInfo>()?.0.snapshot();
        if !snapshot.channel_supported || snapshot.is_homebrew {
            return None;
        }
        let can_change = !self.submitting
            && !matches!(
                snapshot.status,
                UpdateStatus::Installing { .. } | UpdateStatus::ReadyToRestart { .. }
            );
        let mut choices = h_flex().gap(px(8.0));
        for (channel, id, label) in [
            (UpdateChannel::Release, "update-channel-release", "Release"),
            (UpdateChannel::Nightly, "update-channel-nightly", "Nightly"),
        ] {
            let choice = if self.catalog_channel == channel {
                button_primary(id, label, t).cursor_default()
            } else {
                button(id, label, t)
                    .when(!can_change, |choice| choice.opacity(0.45).cursor_default())
                    .when(can_change, |choice| {
                        choice.on_click(cx.listener(move |this, _, _, cx| {
                            this.submit(
                                move || crate::daemon_client::request_channel(channel),
                                true,
                                cx,
                            );
                        }))
                    })
            };
            choices = choices.child(choice);
        }
        Some(
            section_container(t)
                .child(
                    h_flex()
                        .px(px(12.0))
                        .py(px(10.0))
                        .gap(px(8.0))
                        .justify_between()
                        .child(div().text_size(ui_text_md(cx)).child("Update channel"))
                        .child(choices),
                )
                .into_any_element(),
        )
    }

    fn render_ready(&self, t: &ThemeColors, cx: &mut Context<Self>) -> Option<AnyElement> {
        let snapshot = cx.try_global::<GlobalUpdateInfo>()?.0.snapshot();
        let UpdateStatus::Ready { version, .. } = snapshot.status else {
            return None;
        };
        let (message, action) = if snapshot.stable_return {
            let has_config = snapshot.stable_return_config.is_some();
            let message = format!("Release v{version} is ready");
            let action = button_primary("review-stable-return", "Review installation…", t)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.selection = Some(InstallSelection::StableReturn(version.clone()));
                    this.keep_config = !has_config;
                    this.request_error = None;
                    cx.notify();
                }));
            (message, action)
        } else {
            let message = format!("{} is ready", crate::update_version_label(&version));
            let action = button_primary("install-update", "Install update", t).on_click(
                cx.listener(|this, _, _, cx| {
                    this.submit(crate::daemon_client::request_install, false, cx);
                }),
            );
            (message, action)
        };
        Some(
            section_container(t)
                .child(
                    h_flex()
                        .px(px(12.0))
                        .py(px(10.0))
                        .gap(px(8.0))
                        .justify_between()
                        .child(div().text_size(ui_text_md(cx)).child(message))
                        .child(action),
                )
                .into_any_element(),
        )
    }

    fn render_request_error(&self, t: &ThemeColors, cx: &App) -> Option<AnyElement> {
        Some(
            div()
                .mx(px(16.0))
                .mb(px(12.0))
                .text_size(ui_text_sm(cx))
                .text_color(rgb(t.term_red))
                .child(self.request_error.clone()?)
                .into_any_element(),
        )
    }

    fn revert_block_reason(cx: &App) -> Option<&'static str> {
        let status = cx.try_global::<GlobalUpdateInfo>()?.0.status();
        crate::daemon_client::revert_block_reason(&status)
    }

    fn render_revert_block(reason: &'static str, t: &ThemeColors, cx: &App) -> Div {
        div()
            .mx(px(16.0))
            .mb(px(12.0))
            .text_size(ui_text_sm(cx))
            .text_color(rgb(t.term_yellow))
            .child(reason)
    }

    fn render_release_row(
        &self,
        release: &RevertRelease,
        is_last: bool,
        can_revert: bool,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let version = release.version.clone();
        let has_snapshot = release.config_snapshot.is_some();
        let keep_config = self.catalog_channel == UpdateChannel::Nightly || !has_snapshot;
        let request = self.catalog_request;
        let published = release
            .published_at
            .get(..10)
            .map(|date| format!("Published {date}"))
            .unwrap_or_else(|| "Publication date unavailable".to_string());
        div()
            .id(ElementId::Name(format!("revert-version-{version}").into()))
            .relative()
            .px(px(12.0))
            .py(px(9.0))
            .flex()
            .items_center()
            .gap(px(10.0))
            .when(!is_last, |row| row.border_b_1().border_color(rgb(t.border)))
            .child(
                div()
                    .relative()
                    .w(px(12.0))
                    .h(px(28.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .absolute()
                            .top(px(-10.0))
                            .bottom(px(-10.0))
                            .w(px(1.0))
                            .bg(rgb(t.border)),
                    )
                    .child(
                        div()
                            .w(px(7.0))
                            .h(px(7.0))
                            .rounded_full()
                            .bg(rgb(t.bg_primary))
                            .border_1()
                            .border_color(rgb(t.text_muted)),
                    ),
            )
            .child(
                v_flex()
                    .flex_1()
                    .gap(px(2.0))
                    .child(
                        h_flex()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .font_family("monospace")
                                    .text_size(ui_text_md(cx))
                                    .text_color(rgb(t.text_primary))
                                    .child(crate::update_version_label(&release.version)),
                            )
                            .when(has_snapshot, |row| {
                                row.child(
                                    div()
                                        .px(px(5.0))
                                        .py(px(1.0))
                                        .rounded(px(3.0))
                                        .bg(rgb(t.bg_secondary))
                                        .text_size(ui_text_sm(cx))
                                        .text_color(rgb(t.text_secondary))
                                        .child("config checkpoint"),
                                )
                            }),
                    )
                    .child(
                        div()
                            .text_size(ui_text_sm(cx))
                            .text_color(rgb(t.text_muted))
                            .child(published),
                    ),
            )
            .child(
                button(
                    ElementId::Name(format!("select-revert-{version}").into()),
                    if release.newer { "Install" } else { "Revert" },
                    t,
                )
                .when(!can_revert, |button| button.opacity(0.45).cursor_default())
                .when(can_revert, |button| {
                    button.on_click(cx.listener(move |this, _, _, cx| {
                        if this.catalog_request != request
                            || Self::revert_block_reason(cx).is_some()
                        {
                            return;
                        }
                        this.selection = Some(InstallSelection::CatalogVersion(version.clone()));
                        this.keep_config = keep_config;
                        this.request_error = None;
                        cx.notify();
                    }))
                }),
            )
            .into_any_element()
    }

    fn render_confirmation(
        &self,
        selection: InstallSelection,
        snapshot: Option<ConfigSnapshot>,
        legacy_nightly: bool,
        newer: bool,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let nightly_target = matches!(
            &selection,
            InstallSelection::CatalogVersion(version) if version.starts_with("nightly-")
        );
        let (version, title, action) = match &selection {
            InstallSelection::CatalogVersion(version) => (
                crate::update_version_label(version),
                if newer {
                    "Confirm version installation"
                } else if nightly_target {
                    "Confirm nightly revert"
                } else {
                    "Confirm version revert"
                },
                if newer { "Install" } else { "Revert to" },
            ),
            InstallSelection::StableReturn(version) => (
                crate::update_version_label(version),
                "Confirm return to Release",
                "Install Release",
            ),
        };
        let restore_config = snapshot.is_some() && !self.keep_config;
        let historical = matches!(&selection, InstallSelection::CatalogVersion(_));
        let stable_to_nightly = nightly_target && !newer && BUILD_INFO.nightly.is_none();
        let channel = self.catalog_channel;
        let request = self.catalog_request;
        let block_reason = historical.then(|| Self::revert_block_reason(cx)).flatten();
        let can_submit = !self.submitting && block_reason.is_none();
        v_flex()
            .child(section_header(title, t, cx))
            .children(self.render_request_error(t, cx))
            .children(block_reason.map(|reason| Self::render_revert_block(reason, t, cx)))
            .children(self.render_restart_action(t, cx))
            .child(
                section_container(t)
                    .child(
                        v_flex()
                            .px(px(12.0))
                            .py(px(10.0))
                            .gap(px(8.0))
                            .child(
                                div()
                                    .text_size(ui_text(13.0, cx))
                                    .text_color(rgb(t.text_primary))
                                    .child(if historical && !newer {
                                        format!("Revert Okena to {version}")
                                    } else {
                                        format!("Install Okena {version}")
                                    }),
                            )
                            .child(
                                div()
                                    .text_size(ui_text_sm(cx))
                                    .text_color(rgb(t.term_yellow))
                                    .child("Restarting the daemon will end active terminal sessions."),
                            )
                            .when(legacy_nightly, |content| {
                                content.child(
                                    div()
                                        .text_size(ui_text_sm(cx))
                                        .text_color(rgb(t.term_yellow))
                                        .child("After restart, this build reports its base version and checks only for stable releases. Update channel controls require a newer build."),
                                )
                            })
                            .when(stable_to_nightly, |content| {
                                content.child(
                                    div()
                                        .text_size(ui_text_sm(cx))
                                        .text_color(rgb(t.term_yellow))
                                        .child("This nightly build may predate the stable release you are running. Okena keeps your current configuration."),
                                )
                            }),
                    )
                    .child(
                        div()
                            .border_t_1()
                            .border_color(rgb(t.border))
                            .px(px(12.0))
                            .py(px(10.0))
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                v_flex()
                                    .gap(px(2.0))
                                    .child(
                                        div()
                                            .text_size(ui_text(13.0, cx))
                                            .text_color(rgb(t.text_primary))
                                            .child("Restore previous configuration"),
                                    )
                                    .child(
                                        div()
                                            .max_w(px(390.0))
                                            .text_size(ui_text_sm(cx))
                                            .text_color(rgb(if restore_config {
                                                t.text_muted
                                            } else {
                                                t.term_yellow
                                            }))
                                            .child(match &snapshot {
                                                Some(snapshot) if restore_config => format!(
                                                    "Restore the checkpoint created {}.",
                                                    snapshot.created_at
                                                ),
                                                Some(_) => "Keep the current config. It may be incompatible with this version.".to_string(),
                                                None => "No exact checkpoint exists. The current config must be kept and may be incompatible.".to_string(),
                                            }),
                                    ),
                            )
                            .child(
                                toggle_switch("restore-revert-config", restore_config, t)
                                    .when(snapshot.is_none(), |toggle| {
                                        toggle.opacity(0.45).cursor_default()
                                    })
                                    .when(snapshot.is_some(), |toggle| {
                                        toggle.on_click(cx.listener(|this, _, _, cx| {
                                            this.keep_config = !this.keep_config;
                                            cx.notify();
                                        }))
                                    }),
                            ),
                    )
                    .child(
                        h_flex()
                            .border_t_1()
                            .border_color(rgb(t.border))
                            .px(px(12.0))
                            .py(px(10.0))
                            .gap(px(8.0))
                            .justify_end()
                            .child(button("cancel-version-revert", "Cancel", t).on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.selection = None;
                                    this.request_error = None;
                                    cx.notify();
                                }),
                            ))
                            .child(
                                button_primary(
                                    "confirm-version-revert",
                                    format!("{action} {version}"),
                                    t,
                                )
                                    .when(!can_submit, |button| {
                                        button.opacity(0.45).cursor_default()
                                    })
                                    .when(can_submit, |button| {
                                        button.on_click(cx.listener(move |this, _, _, cx| {
                                            if historical
                                                && (this.catalog_request != request
                                                    || Self::revert_block_reason(cx).is_some())
                                            {
                                                return;
                                            }
                                            let selection = selection.clone();
                                            this.submit(
                                                move || match selection {
                                                    InstallSelection::CatalogVersion(version) => {
                                                        crate::daemon_client::request_revert(
                                                            &version,
                                                            !restore_config,
                                                            channel,
                                                        )
                                                    }
                                                    InstallSelection::StableReturn(version) => {
                                                        crate::daemon_client::request_stable_install(
                                                            &version,
                                                            restore_config,
                                                        )
                                                    }
                                                },
                                                false,
                                                cx,
                                            );
                                        }))
                                    }),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn render_restart_action(&self, t: &ThemeColors, cx: &mut Context<Self>) -> Option<AnyElement> {
        let status = cx.try_global::<GlobalUpdateInfo>()?.0.status();
        let UpdateStatus::ReadyToRestart { version, .. } = status else {
            return None;
        };
        Some(
            section_container(t)
                .child(
                    div()
                        .px(px(12.0))
                        .py(px(10.0))
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            v_flex()
                                .gap(px(2.0))
                                .child(
                                    div()
                                        .text_size(ui_text(13.0, cx))
                                        .text_color(rgb(t.text_primary))
                                        .child(format!(
                                            "{} is ready",
                                            crate::update_version_label(&version)
                                        )),
                                )
                                .child(
                                    div()
                                        .text_size(ui_text_sm(cx))
                                        .text_color(rgb(t.term_yellow))
                                        .child("Restart ends active terminal sessions."),
                                ),
                        )
                        .child(
                            button_primary(
                                "restart-after-revert",
                                if self.restarting {
                                    "Restarting…"
                                } else {
                                    "Restart now"
                                },
                                t,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                if this.restarting {
                                    return;
                                }
                                this.restarting = true;
                                cx.notify();
                                let info = cx
                                    .try_global::<GlobalUpdateInfo>()
                                    .map(|global| global.0.clone());
                                cx.spawn(async move |this, cx| {
                                    let result = smol::unblock(
                                        crate::daemon_client::restart_daemon_and_wait,
                                    )
                                    .await;
                                    let failure = match result {
                                        Ok(()) => this
                                            .update(cx, |_this, cx| {
                                                crate::installer::restart_app(cx)
                                            })
                                            .ok()
                                            .and_then(Result::err),
                                        Err(error) => Some(error),
                                    };
                                    if let Some(error) = failure {
                                        if let Some(info) = info {
                                            info.set_status(UpdateStatus::Failed {
                                                error: error.to_string(),
                                            });
                                        }
                                        let _ = this.update(cx, |this, cx| {
                                            this.restarting = false;
                                            cx.notify();
                                        });
                                    }
                                })
                                .detach();
                            })),
                        ),
                )
                .into_any_element(),
        )
    }
}

impl Render for UpdaterSettingsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        if let Some((selection, config, legacy_nightly, newer)) = self.selected_install(cx) {
            return self.render_confirmation(selection, config, legacy_nightly, newer, &t, cx);
        }
        self.selection = None;

        let mut content = v_flex()
            .child(section_header("Auto Update", &t, cx))
            .children(self.render_channel(&t, cx))
            .children(self.render_request_error(&t, cx))
            .children(self.render_ready(&t, cx))
            .when_some(self.render_status(&t, cx), |content, status| {
                content.child(status)
            })
            .when_some(self.render_restart_action(&t, cx), |content, action| {
                content.child(action)
            });

        content = match &self.catalog {
            CatalogState::Loading => content.child(
                div()
                    .mx(px(16.0))
                    .text_size(ui_text_md(cx))
                    .text_color(rgb(t.text_muted))
                    .child("Loading release history…"),
            ),
            CatalogState::Failed(message) => {
                let message = message.clone();
                content.child(
                    section_container(&t).child(
                        div()
                            .px(px(12.0))
                            .py(px(10.0))
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_size(ui_text_sm(cx))
                                    .text_color(rgb(t.term_red))
                                    .child(message),
                            )
                            .child(button("retry-release-history", "Retry", &t).on_click(
                                cx.listener(|this, _, _, cx| {
                                    if !this.submitting {
                                        this.load_catalog(cx);
                                        cx.notify();
                                    }
                                }),
                            )),
                    ),
                )
            }
            CatalogState::Loaded(catalog) => {
                let (heading, empty) = match catalog.channel {
                    UpdateChannel::Release => (
                        "Earlier stable releases",
                        "No older compatible releases found.",
                    ),
                    UpdateChannel::Nightly => {
                        ("Available versions", "No compatible versions found.")
                    }
                };
                let mut next = content.child(self.render_current(catalog, &t, cx));
                let is_homebrew = cx
                    .try_global::<GlobalUpdateInfo>()
                    .is_some_and(|global| global.0.is_homebrew());
                if is_homebrew {
                    next = next.child(
                        section_container(&t).child(
                            div()
                                .px(px(12.0))
                                .py(px(10.0))
                                .text_size(ui_text_md(cx))
                                .text_color(rgb(t.text_muted))
                                .child(
                                    "This installation is managed by Homebrew. Direct version reverts are disabled.",
                                ),
                        ),
                    );
                } else {
                    let block_reason = Self::revert_block_reason(cx);
                    let can_revert = !self.submitting && block_reason.is_none();
                    next = next.children(
                        block_reason.map(|reason| Self::render_revert_block(reason, &t, cx)),
                    );
                    next = next.child(section_header(heading, &t, cx));
                    if catalog.releases.is_empty() {
                        next = next.child(
                            div()
                                .mx(px(16.0))
                                .text_size(ui_text_md(cx))
                                .text_color(rgb(t.text_muted))
                                .child(empty),
                        );
                    } else {
                        let count = catalog.releases.len();
                        let mut releases = section_container(&t);
                        for (index, release) in catalog.releases.iter().enumerate() {
                            releases = releases.child(self.render_release_row(
                                release,
                                index + 1 == count,
                                can_revert,
                                &t,
                                cx,
                            ));
                        }
                        next = next.child(releases);
                    }
                }
                next
            }
        };
        content.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{CatalogState, ReleaseCatalog, UpdateChannel, UpdaterSettingsView};

    fn catalog(channel: UpdateChannel, version: &str) -> ReleaseCatalog {
        serde_json::from_value(serde_json::json!({
            "channel": channel,
            "current_version": "1.2.3",
            "releases": [{
                "version": version,
                "published_at": "2026-11-21T23:10:00Z",
                "release_url": "https://github.com/contember/okena/releases",
                "asset_name": "okena.exe",
                "config_snapshot": null,
            }],
        }))
        .expect("valid release history fixture")
    }

    fn settings() -> UpdaterSettingsView {
        UpdaterSettingsView {
            catalog: CatalogState::Loaded(catalog(UpdateChannel::Release, "1.2.2")),
            catalog_channel: UpdateChannel::Release,
            catalog_request: 1,
            status_revision: 0,
            selection: None,
            keep_config: false,
            submitting: false,
            restarting: false,
            request_error: None,
        }
    }

    #[test]
    fn switching_to_nightly_drops_release_history_and_ignores_delayed_replies() {
        let mut view = settings();
        let old_request = view.catalog_request;

        // The channel POST clears the old rows before waiting for the daemon.
        view.invalidate_catalog();
        assert!(matches!(view.catalog, CatalogState::Loading));

        view.catalog_channel = UpdateChannel::Nightly;
        view.invalidate_catalog();
        let nightly_request = view.catalog_request;
        let stale_history = catalog(UpdateChannel::Release, "1.2.2");
        assert!(!view.apply_catalog_response(old_request, Ok(stale_history)));
        assert!(!view.apply_catalog_response(
            old_request,
            Err(anyhow::anyhow!("release history timed out")),
        ));
        assert!(matches!(view.catalog, CatalogState::Loading));

        let tag = "nightly-20261121-a1b2c3d";
        let nightly_history = catalog(UpdateChannel::Nightly, tag);
        assert!(view.apply_catalog_response(nightly_request, Ok(nightly_history)));
        let CatalogState::Loaded(history) = &view.catalog else {
            panic!("nightly history should be loaded");
        };
        assert_eq!(history.channel, UpdateChannel::Nightly);
        assert_eq!(history.releases[0].version, tag);
    }

    #[test]
    fn switching_back_does_not_accept_an_old_reply_for_the_same_channel() {
        let mut view = settings();
        let original_release_request = view.catalog_request;
        view.catalog_channel = UpdateChannel::Nightly;
        view.invalidate_catalog();
        view.catalog_channel = UpdateChannel::Release;
        view.invalidate_catalog();

        assert!(!view.apply_catalog_response(
            original_release_request,
            Ok(catalog(UpdateChannel::Release, "1.2.2")),
        ));
        assert!(matches!(view.catalog, CatalogState::Loading));
    }

    #[test]
    fn response_for_a_different_channel_finishes_loading_with_a_retriable_error() {
        let mut view = settings();
        view.catalog_channel = UpdateChannel::Nightly;
        view.invalidate_catalog();

        assert!(view.apply_catalog_response(
            view.catalog_request,
            Ok(catalog(UpdateChannel::Release, "1.2.2")),
        ));
        assert!(matches!(view.catalog, CatalogState::Failed(_)));
    }
}
