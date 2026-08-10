use crate::{GlobalUpdateInfo, ReleaseCatalog, RevertRelease, UpdateStatus};
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::{h_flex, v_flex};
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

pub struct UpdaterSettingsView {
    catalog: CatalogState,
    selected_version: Option<String>,
    keep_config: bool,
    submitting: bool,
    restarting: bool,
}

impl UpdaterSettingsView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self::load_catalog(cx);
        Self::poll_status(cx);
        Self {
            catalog: CatalogState::Loading,
            selected_version: None,
            keep_config: false,
            submitting: false,
            restarting: false,
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
                    .update(cx, |this, cx| {
                        if let Some(info) = &info
                            && !matches!(
                                info.status(),
                                UpdateStatus::Checking
                                    | UpdateStatus::Downloading { .. }
                                    | UpdateStatus::Installing { .. }
                            )
                        {
                            this.submitting = false;
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

    fn selected_release(&self) -> Option<RevertRelease> {
        let CatalogState::Loaded(catalog) = &self.catalog else {
            return None;
        };
        let selected = self.selected_version.as_deref()?;
        catalog
            .releases
            .iter()
            .find(|release| release.version == selected)
            .cloned()
    }

    fn render_status(&self, t: &ThemeColors, cx: &App) -> Option<AnyElement> {
        let status = cx.try_global::<GlobalUpdateInfo>()?.0.status();
        let (label, color) = match status {
            UpdateStatus::Checking => ("Resolving release…".to_string(), t.text_muted),
            UpdateStatus::Downloading { version, progress } => (
                format!("Downloading v{version} · {progress}%"),
                t.term_yellow,
            ),
            UpdateStatus::Installing { version } => {
                (format!("Installing v{version}…"), t.term_yellow)
            }
            UpdateStatus::ReadyToRestart {
                version,
                config_restore,
            } => {
                let config = config_restore
                    .map(|snapshot| format!(" · config from {}", snapshot.created_at))
                    .unwrap_or_else(|| " · keeping current config".to_string());
                (format!("v{version} installed{config}"), t.term_green)
            }
            UpdateStatus::Failed { error } => (format!("Revert failed: {error}"), t.term_red),
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
                                    .child(format!("v{}", catalog.current_version)),
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

    fn render_release_row(
        &self,
        release: &RevertRelease,
        is_last: bool,
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
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.selected_version = Some(version.clone());
                    this.keep_config = !has_snapshot;
                    cx.notify();
                })),
            )
            .into_any_element()
    }

    fn render_confirmation(
        &self,
        release: RevertRelease,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let version = release.version.clone();
        let snapshot = release.config_snapshot.clone();
        let restore_config = snapshot.is_some() && !self.keep_config;
        let keep_config = self.keep_config;
        v_flex()
            .child(section_header("Confirm version revert", t, cx))
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
                                                Some(_) => "Keep the current config. It may be incompatible with this older version.".to_string(),
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
                                    this.selected_version = None;
                                    cx.notify();
                                }),
                            ))
                            .child(
                                button_primary(
                                    "confirm-version-revert",
                                    format!("Revert to v{version}"),
                                    t,
                                )
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if this.submitting {
                                        return;
                                    }
                                    this.submitting = true;
                                    let version = version.clone();
                                    let keep_config = keep_config;
                                    let info = cx
                                        .try_global::<GlobalUpdateInfo>()
                                        .map(|global| global.0.clone());
                                    cx.spawn(async move |this, cx| {
                                        let result = smol::unblock(move || {
                                            crate::daemon_client::request_revert(
                                                &version,
                                                keep_config,
                                            )
                                        })
                                        .await;
                                        let _ = this.update(cx, |this, cx| {
                                            match result {
                                                Ok(snapshot) => {
                                                    if let Some(info) = info {
                                                        info.apply_snapshot(snapshot);
                                                    }
                                                    this.selected_version = None;
                                                }
                                                Err(error) => {
                                                    this.submitting = false;
                                                    if let Some(info) = info {
                                                        info.set_status(UpdateStatus::Failed {
                                                            error: error.to_string(),
                                                        });
                                                    }
                                                }
                                            }
                                            cx.notify();
                                        });
                                    })
                                    .detach();
                                })),
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
                                        .child(format!("v{version} is ready")),
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
                                    match result {
                                        Ok(()) => {
                                            let _ = this.update(cx, |_this, cx| {
                                                crate::installer::restart_app(cx);
                                            });
                                        }
                                        Err(error) => {
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
        if let Some(release) = self.selected_release() {
            return self.render_confirmation(release, &t, cx);
        }

        let mut content = v_flex()
            .child(section_header("Auto Update", &t, cx))
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
                    let count = catalog.releases.len();
                    let mut releases = section_container(&t);
                    for (index, release) in catalog.releases.iter().enumerate() {
                        releases = releases.child(self.render_release_row(
                            release,
                            index + 1 == count,
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
