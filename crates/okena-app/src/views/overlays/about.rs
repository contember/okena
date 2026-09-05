use crate::keybindings::Cancel;
use crate::theme::{ThemeColors, theme};
use crate::ui::tokens::{ui_text, ui_text_md, ui_text_ms, ui_text_sm};
use crate::views::components::{modal_backdrop, modal_content};
use gpui::prelude::*;
use gpui::*;
use okena_ext_updater::{GlobalLocalBuild, GlobalUpdateInfo, UpdateStatus};

const WEBSITE_URL: &str = "https://okena.dev";
const GITHUB_URL: &str = "https://github.com/contember/okena";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum UpdateAction {
    Check,
    Install,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum UpdateTone {
    Neutral,
    Good,
    Warning,
    Error,
}

#[derive(Debug, PartialEq, Eq)]
struct UpdatePresentation {
    title: String,
    detail: String,
    tone: UpdateTone,
    action: Option<UpdateAction>,
}

fn update_presentation(status: &UpdateStatus, confirmed_current: bool) -> UpdatePresentation {
    match status {
        UpdateStatus::Idle if confirmed_current => UpdatePresentation {
            title: "Okena is up to date".to_string(),
            detail: "You're running the latest published release.".to_string(),
            tone: UpdateTone::Good,
            action: Some(UpdateAction::Check),
        },
        UpdateStatus::Idle => UpdatePresentation {
            title: "Update status not checked".to_string(),
            detail: "Check GitHub Releases for a newer Okena build.".to_string(),
            tone: UpdateTone::Neutral,
            action: Some(UpdateAction::Check),
        },
        UpdateStatus::Checking => UpdatePresentation {
            title: "Checking for updates...".to_string(),
            detail: "Looking for the latest published release.".to_string(),
            tone: UpdateTone::Neutral,
            action: None,
        },
        UpdateStatus::Available { version, .. } => UpdatePresentation {
            title: format!("Okena v{version} is available"),
            detail: "The update is being prepared for download.".to_string(),
            tone: UpdateTone::Good,
            action: None,
        },
        UpdateStatus::Downloading { version, progress } => UpdatePresentation {
            title: format!("Downloading Okena v{version}"),
            detail: format!("{progress}% complete"),
            tone: UpdateTone::Warning,
            action: None,
        },
        UpdateStatus::Ready { version, .. } => UpdatePresentation {
            title: format!("Okena v{version} is ready to install"),
            detail: "Install now, then restart when prompted.".to_string(),
            tone: UpdateTone::Good,
            action: Some(UpdateAction::Install),
        },
        UpdateStatus::Installing { version } => UpdatePresentation {
            title: format!("Installing Okena v{version}"),
            detail: "The new build is being installed.".to_string(),
            tone: UpdateTone::Warning,
            action: None,
        },
        UpdateStatus::ReadyToRestart { version, .. } => UpdatePresentation {
            title: format!("Okena v{version} is ready"),
            detail: "Restart from the status bar to finish the update.".to_string(),
            tone: UpdateTone::Good,
            action: None,
        },
        UpdateStatus::BrewUpdate { version } => UpdatePresentation {
            title: format!("Okena v{version} is available"),
            detail: "Run `brew upgrade --cask okena` to update.".to_string(),
            tone: UpdateTone::Good,
            action: None,
        },
        UpdateStatus::Failed { error } => UpdatePresentation {
            title: "Couldn't check for updates".to_string(),
            detail: error.clone(),
            tone: UpdateTone::Error,
            action: Some(UpdateAction::Check),
        },
    }
}

pub struct AboutModal {
    focus_handle: FocusHandle,
    confirmed_current: bool,
}

impl AboutModal {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            confirmed_current: false,
        }
    }

    fn close(&self, cx: &mut Context<Self>) {
        cx.emit(AboutModalEvent::Close);
    }

    fn check_for_updates(&mut self, cx: &mut Context<Self>) {
        let Some(global) = cx.try_global::<GlobalUpdateInfo>() else {
            return;
        };
        let info = global.0.clone();
        self.confirmed_current = false;
        info.set_status(UpdateStatus::Checking);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let confirmed_current =
                match smol::unblock(okena_ext_updater::daemon_client::request_check).await {
                    Ok(snapshot) => {
                        let confirmed_current = matches!(&snapshot.status, UpdateStatus::Idle);
                        info.apply_snapshot(snapshot);
                        confirmed_current
                    }
                    Err(error) => {
                        info.set_status(UpdateStatus::Failed {
                            error: error.to_string(),
                        });
                        false
                    }
                };
            let _ = this.update(cx, |this, cx| {
                this.confirmed_current = confirmed_current;
                cx.notify();
            });
        })
        .detach();
    }

    fn install_update(&mut self, cx: &mut Context<Self>) {
        let Some(global) = cx.try_global::<GlobalUpdateInfo>() else {
            return;
        };
        let info = global.0.clone();
        cx.spawn(async move |this, cx| {
            match smol::unblock(okena_ext_updater::daemon_client::request_install).await {
                Ok(snapshot) => info.apply_snapshot(snapshot),
                Err(error) => info.set_status(UpdateStatus::Failed {
                    error: error.to_string(),
                }),
            }
            let _ = this.update(cx, |_, cx| cx.notify());
        })
        .detach();
    }

    fn render_link(
        &self,
        id: &'static str,
        label: &'static str,
        detail: &'static str,
        url: &'static str,
        t: &ThemeColors,
        cx: &App,
    ) -> Stateful<Div> {
        div()
            .id(id)
            .flex_1()
            .cursor_pointer()
            .p(px(12.0))
            .flex()
            .items_center()
            .justify_between()
            .gap(px(12.0))
            .rounded(px(6.0))
            .border_1()
            .border_color(rgb(t.border))
            .bg(rgb(t.bg_secondary))
            .hover(|style| style.bg(rgb(t.bg_hover)))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(
                        div()
                            .text_size(ui_text_md(cx))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(t.text_primary))
                            .child(label),
                    )
                    .child(
                        div()
                            .text_size(ui_text_sm(cx))
                            .text_color(rgb(t.text_muted))
                            .child(detail),
                    ),
            )
            .child(
                svg()
                    .path("icons/external-link.svg")
                    .size(px(14.0))
                    .text_color(rgb(t.text_muted)),
            )
            .on_click(move |_, _, _| okena_core::process::open_url(url))
    }

    fn render_update_action(
        &self,
        action: UpdateAction,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let label = match action {
            UpdateAction::Check => "Check now",
            UpdateAction::Install => "Install update",
        };

        div()
            .id(match action {
                UpdateAction::Check => "about-check-update",
                UpdateAction::Install => "about-install-update",
            })
            .cursor_pointer()
            .px(px(10.0))
            .h(px(28.0))
            .flex()
            .items_center()
            .rounded(px(4.0))
            .bg(rgb(t.bg_hover))
            .hover(|style| style.bg(rgb(t.border)))
            .text_size(ui_text_ms(cx))
            .font_weight(FontWeight::MEDIUM)
            .text_color(rgb(t.text_primary))
            .child(label)
            .on_click(cx.listener(move |this, _, _, cx| match action {
                UpdateAction::Check => this.check_for_updates(cx),
                UpdateAction::Install => this.install_update(cx),
            }))
    }

    fn render_update_card(
        &self,
        update: UpdatePresentation,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let color = match update.tone {
            UpdateTone::Neutral => t.text_muted,
            UpdateTone::Good => t.term_green,
            UpdateTone::Warning => t.term_yellow,
            UpdateTone::Error => t.term_red,
        };

        div()
            .mx(px(24.0))
            .p(px(14.0))
            .flex()
            .items_center()
            .justify_between()
            .gap(px(16.0))
            .rounded(px(6.0))
            .border_1()
            .border_color(rgb(t.border))
            .bg(rgb(t.bg_secondary))
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .flex()
                    .items_start()
                    .gap(px(10.0))
                    .child(
                        div()
                            .mt(px(5.0))
                            .size(px(7.0))
                            .flex_shrink_0()
                            .rounded_full()
                            .bg(rgb(color)),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap(px(3.0))
                            .child(
                                div()
                                    .text_size(ui_text_md(cx))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(rgb(t.text_primary))
                                    .child(update.title),
                            )
                            .child(
                                div()
                                    .text_size(ui_text_sm(cx))
                                    .text_color(rgb(t.text_muted))
                                    .child(update.detail),
                            ),
                    ),
            )
            .when_some(update.action, |card, action| {
                card.child(self.render_update_action(action, t, cx))
            })
            .into_any_element()
    }
}

pub enum AboutModalEvent {
    Close,
}

impl EventEmitter<AboutModalEvent> for AboutModal {}

impl Render for AboutModal {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let focus_handle = self.focus_handle.clone();

        if !focus_handle.is_focused(window) {
            window.focus(&focus_handle, cx);
        }

        let update_info = cx.try_global::<GlobalUpdateInfo>().map(|global| &global.0);
        let version = update_info
            .map(|info| info.app_version())
            .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string());
        let source = if cx.has_global::<GlobalLocalBuild>() {
            "Local source build"
        } else if update_info.is_some_and(|info| info.is_homebrew()) {
            "Homebrew"
        } else {
            "Direct install"
        };
        let close_button = div()
            .id("about-close")
            .absolute()
            .top(px(12.0))
            .right(px(12.0))
            .size(px(28.0))
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(4.0))
            .hover(|style| style.bg(rgb(t.bg_hover)))
            .child(
                svg()
                    .path("icons/close.svg")
                    .size(px(13.0))
                    .text_color(rgb(t.text_muted)),
            )
            .on_click(cx.listener(|this, _, _, cx| this.close(cx)))
            .into_any_element();

        let hero = div()
            .px(px(24.0))
            .pt(px(24.0))
            .pb(px(20.0))
            .flex()
            .items_center()
            .gap(px(18.0))
            .child(
                div()
                    .size(px(82.0))
                    .flex_shrink_0()
                    .rounded(px(18.0))
                    .overflow_hidden()
                    .border_1()
                    .border_color(rgb(t.border))
                    .child(img("logo.png").size_full()),
            )
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(
                        div()
                            .text_size(ui_text(25.0, cx))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(t.text_primary))
                            .child("Okena"),
                    )
                    .child(
                        div()
                            .max_w(px(330.0))
                            .text_size(ui_text_md(cx))
                            .text_color(rgb(t.text_secondary))
                            .child(
                                "A fast native terminal workspace for projects, panes, and persistent sessions.",
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .px(px(7.0))
                                    .h(px(20.0))
                                    .flex()
                                    .items_center()
                                    .rounded(px(3.0))
                                    .bg(rgb(t.bg_secondary))
                                    .text_size(ui_text_sm(cx))
                                    .font_family("JetBrains Mono")
                                    .text_color(rgb(t.text_secondary))
                                    .child(format!("v{version}")),
                            )
                            .child(
                                div()
                                    .text_size(ui_text_sm(cx))
                                    .text_color(rgb(t.text_muted))
                                    .child(source),
                            ),
                    ),
            )
            .into_any_element();

        let update_card = update_info
            .map(|info| update_presentation(&info.status(), self.confirmed_current))
            .map(|update| self.render_update_card(update, &t, cx));

        let links = div()
            .px(px(24.0))
            .py(px(16.0))
            .flex()
            .gap(px(10.0))
            .child(self.render_link("about-website", "Website", "okena.dev", WEBSITE_URL, &t, cx))
            .child(self.render_link(
                "about-github",
                "GitHub",
                "Source and releases",
                GITHUB_URL,
                &t,
                cx,
            ))
            .into_any_element();

        let footer = div()
            .px(px(24.0))
            .py(px(12.0))
            .border_t_1()
            .border_color(rgb(t.border))
            .flex()
            .items_center()
            .justify_between()
            .text_size(ui_text_sm(cx))
            .text_color(rgb(t.text_muted))
            .child("Built in Rust with GPUI")
            .child("Copyright 2026 Contember")
            .into_any_element();

        let content = modal_content("about-modal", &t)
            .relative()
            .w(px(520.0))
            .overflow_hidden()
            .child(close_button)
            .child(hero)
            .children(update_card)
            .child(links)
            .child(footer)
            .into_any_element();

        modal_backdrop("about-backdrop", &t)
            .track_focus(&focus_handle)
            .key_context("AboutModal")
            .items_center()
            .on_action(cx.listener(|this, _: &Cancel, _, cx| this.close(cx)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.close(cx)),
            )
            .child(content)
            .into_any_element()
    }
}

impl_focusable!(AboutModal);

#[cfg(test)]
mod tests {
    use super::{UpdateAction, UpdateTone, update_presentation};
    use okena_ext_updater::UpdateStatus;

    #[test]
    fn idle_update_can_be_checked() {
        let presentation = update_presentation(&UpdateStatus::Idle, false);
        assert_eq!(presentation.action, Some(UpdateAction::Check));
        assert_eq!(presentation.tone, UpdateTone::Neutral);
    }

    #[test]
    fn completed_idle_update_is_current() {
        let presentation = update_presentation(&UpdateStatus::Idle, true);
        assert_eq!(presentation.title, "Okena is up to date");
        assert_eq!(presentation.tone, UpdateTone::Good);
    }

    #[test]
    fn ready_update_can_be_installed() {
        let presentation = update_presentation(
            &UpdateStatus::Ready {
                version: "1.2.3".to_string(),
                path: "/tmp/okena".into(),
            },
            false,
        );
        assert_eq!(presentation.action, Some(UpdateAction::Install));
        assert!(presentation.title.contains("v1.2.3"));
    }

    #[test]
    fn failed_update_can_be_retried() {
        let presentation = update_presentation(
            &UpdateStatus::Failed {
                error: "offline".to_string(),
            },
            false,
        );
        assert_eq!(presentation.action, Some(UpdateAction::Check));
        assert_eq!(presentation.detail, "offline");
        assert_eq!(presentation.tone, UpdateTone::Error);
    }
}
