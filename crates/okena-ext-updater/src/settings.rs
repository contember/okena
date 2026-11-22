use crate::{GlobalUpdateInfo, ReleaseCatalog, RevertRelease, UpdateChannel, UpdateStatus};
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::{h_flex, v_flex};
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
    PreviousRelease(String),
    StableReturn(String),
}

pub struct UpdaterSettingsView {
    catalog: CatalogState,
    selection: Option<InstallSelection>,
    keep_config: bool,
    submitting: bool,
    restarting: bool,
    request_error: Option<String>,
}

impl UpdaterSettingsView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self::load_catalog(cx);
        Self::poll_status(cx);
        Self {
            catalog: CatalogState::Loading,
            selection: None,
            keep_config: false,
            submitting: false,
            restarting: false,
            request_error: None,
        }
    }

    fn load_catalog(cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let result = smol::unblock(crate::daemon_client::fetch_releases).await;
            let _ = this.update(cx, |this, cx| {
                this.catalog = match result {
                    Ok(catalog) => CatalogState::Loaded(catalog),
                    Err(error) => CatalogState::Failed(error.to_string()),
                };
                cx.notify();
            });
        })
        .detach();
    }

    fn poll_status(cx: &mut Context<Self>) {
        let info = cx
            .try_global::<GlobalUpdateInfo>()
            .map(|global| global.0.clone());
        cx.spawn(async move |this, cx| {
            loop {
                if let Some(info) = &info
                    && let Ok(snapshot) = smol::unblock(crate::daemon_client::fetch_status).await
                {
                    info.apply_snapshot(snapshot);
                }
                if this
                    .update(cx, |_, cx| {
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

    fn selected_install(&self, cx: &App) -> Option<(InstallSelection, Option<ConfigSnapshot>)> {
        let selection = self.selection.clone()?;
        let config = match &selection {
            InstallSelection::PreviousRelease(version) => {
                let CatalogState::Loaded(catalog) = &self.catalog else {
                    return None;
                };
                catalog
                    .releases
                    .iter()
                    .find(|release| &release.version == version)?
                    .config_snapshot
                    .clone()
            }
            InstallSelection::StableReturn(version) => {
                let snapshot = cx.try_global::<GlobalUpdateInfo>()?.0.snapshot();
                if !snapshot.stable_return
                    || !matches!(snapshot.status, UpdateStatus::Ready { version: ready, .. } if &ready == version)
                {
                    return None;
                }
                snapshot.stable_return_config
            }
        };
        Some((selection, config))
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
                                    .child(format!("v{running}")),
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
        cx: &mut Context<Self>,
    ) {
        if self.submitting {
            return;
        }
        self.submitting = true;
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
                        if let Some(info) = info {
                            info.apply_snapshot(snapshot);
                        }
                        this.selection = None;
                    }
                    Err(error) => this.request_error = Some(error.to_string()),
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
            let choice = if snapshot.channel == channel {
                button_primary(id, label, t).cursor_default()
            } else {
                button(id, label, t)
                    .when(!can_change, |choice| choice.opacity(0.45).cursor_default())
                    .when(can_change, |choice| {
                        choice.on_click(cx.listener(move |this, _, _, cx| {
                            this.submit(move || crate::daemon_client::request_channel(channel), cx);
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
                    this.submit(crate::daemon_client::request_install, cx);
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
        let published = release.published_at.get(..10).unwrap_or("unknown");
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
                                    .child(format!("v{}", release.version)),
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
                            .child(format!("Released {published}")),
                    ),
            )
            .child(
                button(
                    ElementId::Name(format!("select-revert-{version}").into()),
                    "Revert",
                    t,
                )
                .when(!can_revert, |button| button.opacity(0.45).cursor_default())
                .when(can_revert, |button| {
                    button.on_click(cx.listener(move |this, _, _, cx| {
                        if Self::revert_block_reason(cx).is_some() {
                            return;
                        }
                        this.selection = Some(InstallSelection::PreviousRelease(version.clone()));
                        this.keep_config = !has_snapshot;
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
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (version, title, action) = match &selection {
            InstallSelection::PreviousRelease(version) => (
                version,
                "Confirm version revert",
                format!("Revert to v{version}"),
            ),
            InstallSelection::StableReturn(version) => (
                version,
                "Confirm return to Release",
                format!("Install Release v{version}"),
            ),
        };
        let restore_config = snapshot.is_some() && !self.keep_config;
        let historical = matches!(&selection, InstallSelection::PreviousRelease(_));
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
                                    .child(format!("Install Okena v{version}")),
                            )
                            .child(
                                div()
                                    .text_size(ui_text_sm(cx))
                                    .text_color(rgb(t.term_yellow))
                                    .child("Restarting the daemon will end active terminal sessions."),
                            ),
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
                                button_primary("confirm-version-revert", action, t)
                                    .when(!can_submit, |button| {
                                        button.opacity(0.45).cursor_default()
                                    })
                                    .when(can_submit, |button| {
                                        button.on_click(cx.listener(move |this, _, _, cx| {
                                            if historical && Self::revert_block_reason(cx).is_some()
                                            {
                                                return;
                                            }
                                            let selection = selection.clone();
                                            this.submit(
                                                move || match selection {
                                                    InstallSelection::PreviousRelease(version) => {
                                                        crate::daemon_client::request_revert(
                                                            &version,
                                                            !restore_config,
                                                        )
                                                    }
                                                    InstallSelection::StableReturn(version) => {
                                                        crate::daemon_client::request_stable_install(
                                                            &version,
                                                            restore_config,
                                                        )
                                                    }
                                                },
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
        if let Some((selection, config)) = self.selected_install(cx) {
            return self.render_confirmation(selection, config, &t, cx);
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
            CatalogState::Failed(error) => {
                let error = error.clone();
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
                                    .child(error),
                            )
                            .child(button("retry-release-history", "Retry", &t).on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.catalog = CatalogState::Loading;
                                    Self::load_catalog(cx);
                                    cx.notify();
                                }),
                            )),
                    ),
                )
            }
            CatalogState::Loaded(catalog) => {
                let catalog = catalog.clone();
                let mut next = content
                    .child(self.render_current(&catalog, &t, cx))
                    .child(section_header("Earlier stable releases", &t, cx));
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
                } else if catalog.releases.is_empty() {
                    next = next.child(
                        div()
                            .mx(px(16.0))
                            .text_size(ui_text_md(cx))
                            .text_color(rgb(t.text_muted))
                            .child("No older compatible releases found."),
                    );
                } else {
                    let block_reason = Self::revert_block_reason(cx);
                    let can_revert = !self.submitting && block_reason.is_none();
                    next = next.children(
                        block_reason.map(|reason| Self::render_revert_block(reason, &t, cx)),
                    );
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
                next
            }
        };
        content.into_any_element()
    }
}
