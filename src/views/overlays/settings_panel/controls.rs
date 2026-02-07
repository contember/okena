use crate::settings::{settings_entity, SettingsState};
use crate::terminal::session_backend::SessionBackend;
use crate::terminal::shell_config::ShellType;
use crate::theme::theme;
use crate::views::components::{dropdown_button, dropdown_option, dropdown_overlay};
use gpui::*;
use gpui_component::h_flex;

use super::components::*;
use super::SettingsPanel;

impl SettingsPanel {
    pub(super) fn render_number_stepper(
        &self,
        id: &str,
        label: &str,
        value: f32,
        format: &str,
        step: f32,
        width: f32,
        has_border: bool,
        update_fn: impl Fn(&mut SettingsState, f32, &mut Context<SettingsState>) + 'static + Clone,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = theme(cx);
        let dec_fn = update_fn.clone();
        let inc_fn = update_fn;

        settings_row(id.to_string(), label, &t, has_border).child(
            h_flex()
                .gap(px(4.0))
                .child(
                    stepper_button(format!("{}-dec", id), "-", &t)
                        .on_mouse_down(MouseButton::Left, cx.listener(move |_, _, _, cx| {
                            let dec_fn = dec_fn.clone();
                            settings_entity(cx).update(cx, |state, cx| {
                                dec_fn(state, value - step, cx);
                            });
                        })),
                )
                .child(value_display(format.replace("{}", &format!("{:.1}", value)), width, &t))
                .child(
                    stepper_button(format!("{}-inc", id), "+", &t)
                        .on_mouse_down(MouseButton::Left, cx.listener(move |_, _, _, cx| {
                            let inc_fn = inc_fn.clone();
                            settings_entity(cx).update(cx, |state, cx| {
                                inc_fn(state, value + step, cx);
                            });
                        })),
                ),
        )
    }

    pub(super) fn render_integer_stepper(
        &self,
        id: &str,
        label: &str,
        value: u32,
        step: u32,
        width: f32,
        has_border: bool,
        update_fn: impl Fn(&mut SettingsState, u32, &mut Context<SettingsState>) + 'static + Clone,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = theme(cx);
        let dec_fn = update_fn.clone();
        let inc_fn = update_fn;

        settings_row(id.to_string(), label, &t, has_border).child(
            h_flex()
                .gap(px(4.0))
                .child(
                    stepper_button(format!("{}-dec", id), "-", &t)
                        .on_mouse_down(MouseButton::Left, cx.listener(move |_, _, _, cx| {
                            let dec_fn = dec_fn.clone();
                            settings_entity(cx).update(cx, |state, cx| {
                                dec_fn(state, value.saturating_sub(step), cx);
                            });
                        })),
                )
                .child(value_display(format!("{}", value), width, &t))
                .child(
                    stepper_button(format!("{}-inc", id), "+", &t)
                        .on_mouse_down(MouseButton::Left, cx.listener(move |_, _, _, cx| {
                            let inc_fn = inc_fn.clone();
                            settings_entity(cx).update(cx, |state, cx| {
                                inc_fn(state, value + step, cx);
                            });
                        })),
                ),
        )
    }

    pub(super) fn render_toggle(
        &self,
        id: &str,
        label: &str,
        enabled: bool,
        has_border: bool,
        update_fn: impl Fn(&mut SettingsState, bool, &mut Context<SettingsState>) + 'static + Clone,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = theme(cx);

        settings_row(id.to_string(), label, &t, has_border).child(
            toggle_switch(format!("{}-toggle", id), enabled, &t)
                .on_mouse_down(MouseButton::Left, cx.listener(move |_, _, _, cx| {
                    let update_fn = update_fn.clone();
                    settings_entity(cx).update(cx, |state, cx| {
                        update_fn(state, !enabled, cx);
                    });
                })),
        )
    }

    pub(super) fn render_font_dropdown_row(&mut self, current_family: &str, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        settings_row("font-family".to_string(), "Font Family", &t, true).child(
            dropdown_button("font-family-btn", current_family, self.font_dropdown_open, &t)
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                    this.font_dropdown_open = !this.font_dropdown_open;
                    this.shell_dropdown_open = false;
                    this.session_backend_dropdown_open = false;
                    this.project_dropdown_open = false;
                    cx.notify();
                })),
        )
    }

    pub(super) fn render_font_dropdown_overlay(&self, current: &str, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        dropdown_overlay("font-family-dropdown-list", 140.0, 32.0, &t)
            .children(FONT_FAMILIES.iter().map(|family| {
                let is_selected = *family == current;
                let family_str = family.to_string();

                dropdown_option(format!("font-opt-{}", family), family, is_selected, &t)
                    .on_mouse_down(MouseButton::Left, cx.listener({
                        let family = family_str.clone();
                        move |this, _, _, cx| {
                            let family = family.clone();
                            settings_entity(cx).update(cx, |state, cx| {
                                state.set_font_family(family, cx);
                            });
                            this.font_dropdown_open = false;
                            cx.notify();
                        }
                    }))
            }))
    }

    pub(super) fn render_shell_dropdown_row(&mut self, current_shell: &ShellType, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let display_name = current_shell.display_name();

        settings_row("default-shell".to_string(), "Default Shell", &t, true).child(
            dropdown_button("default-shell-btn", &display_name, self.shell_dropdown_open, &t)
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                    this.shell_dropdown_open = !this.shell_dropdown_open;
                    this.font_dropdown_open = false;
                    this.session_backend_dropdown_open = false;
                    this.project_dropdown_open = false;
                    cx.notify();
                })),
        )
    }

    pub(super) fn render_shell_dropdown_overlay(&self, current_shell: &ShellType, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        let available: Vec<_> = self.available_shells.iter()
            .filter(|s| s.available)
            .collect();

        dropdown_overlay("shell-dropdown-list", 290.0, 32.0, &t)
            .min_w(px(180.0))
            .max_h(px(250.0))
            .children(available.into_iter().map(|shell_info| {
                let is_selected = &shell_info.shell_type == current_shell;
                let shell_type = shell_info.shell_type.clone();
                let name = shell_info.name.clone();

                dropdown_option(format!("shell-opt-{}", name), &name, is_selected, &t)
                    .on_mouse_down(MouseButton::Left, cx.listener({
                        let shell_type = shell_type.clone();
                        move |this, _, _, cx| {
                            let shell = shell_type.clone();
                            settings_entity(cx).update(cx, |state, cx| {
                                state.set_default_shell(shell, cx);
                            });
                            this.shell_dropdown_open = false;
                            cx.notify();
                        }
                    }))
            }))
    }

    pub(super) fn render_session_backend_dropdown_row(&mut self, current_backend: &SessionBackend, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let display_name = current_backend.display_name();

        settings_row_with_desc("session-backend".to_string(), "Session Backend", "Requires restart", &t, true).child(
            dropdown_button("session-backend-btn", display_name, self.session_backend_dropdown_open, &t)
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                    this.session_backend_dropdown_open = !this.session_backend_dropdown_open;
                    this.font_dropdown_open = false;
                    this.shell_dropdown_open = false;
                    this.project_dropdown_open = false;
                    cx.notify();
                })),
        )
    }

    pub(super) fn render_session_backend_dropdown_overlay(&self, current_backend: &SessionBackend, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        dropdown_overlay("session-backend-dropdown-list", 290.0, 70.0, &t)
            .min_w(px(180.0))
            .children(SessionBackend::all_variants().iter().map(|backend| {
                let is_selected = backend == current_backend;
                let backend_copy = *backend;
                let name = backend.display_name();

                dropdown_option(format!("backend-opt-{:?}", backend), name, is_selected, &t)
                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                        settings_entity(cx).update(cx, |state, cx| {
                            state.set_session_backend(backend_copy, cx);
                        });
                        this.session_backend_dropdown_open = false;
                        cx.notify();
                    }))
            }))
    }
}
