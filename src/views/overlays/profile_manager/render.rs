use crate::keybindings::Cancel;
use crate::theme::{theme, with_alpha};
use crate::ui::tokens::{ui_text, ui_text_sm, ui_text_md, ui_text_ms, ui_text_xl};
use crate::views::components::{modal_backdrop, modal_content, modal_header, SimpleInput};
use gpui::*;
use gpui_component::{h_flex, v_flex};
use gpui::prelude::*;

use super::ProfileManager;

impl ProfileManager {
    pub(super) fn render_profile_row(&self, entry: &okena_core::profiles::ProfileEntry, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = theme(cx);
        let id = entry.id.clone();
        let display_name = entry.display_name.clone();
        let is_active = id == self.active_id;
        let is_deleting = self.show_delete_confirmation.as_deref() == Some(&id);
        let is_default = id == self.default_profile_id;

        let id_for_switch = id.clone();
        let id_for_delete = id.clone();
        let id_for_delete_confirm = id.clone();

        h_flex()
            .justify_between()
            .px(px(12.0))
            .py(px(10.0))
            .border_b_1()
            .border_color(rgb(t.border))
            .when(is_active, |d| d.bg(with_alpha(t.button_primary_bg, 0.08)))
            .when(is_deleting, |d| {
                d.bg(with_alpha(t.error, 0.1)).child(
                    h_flex()
                        .justify_between()
                        .w_full()
                        .child(
                            v_flex()
                                .gap(px(2.0))
                                .child(
                                    div()
                                        .text_size(ui_text(13.0, cx))
                                        .text_color(rgb(t.error))
                                        .child(format!("Delete '{display_name}'?")),
                                )
                                .child(
                                    div()
                                        .text_size(ui_text_sm(cx))
                                        .text_color(rgb(t.text_muted))
                                        .child("Claude credentials are preserved."),
                                ),
                        )
                        .child(
                            h_flex()
                                .gap(px(8.0))
                                .child(
                                    div()
                                        .id(SharedString::from(format!("profile-delete-confirm-{id}")))
                                        .cursor_pointer()
                                        .px(px(10.0))
                                        .py(px(4.0))
                                        .rounded(px(4.0))
                                        .bg(rgb(t.error))
                                        .text_size(ui_text_md(cx))
                                        .text_color(rgb(0xFFFFFF))
                                        .child("Delete")
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _window, cx| {
                                                this.delete_profile(&id_for_delete_confirm, cx);
                                            }),
                                        ),
                                )
                                .child(
                                    div()
                                        .id(SharedString::from(format!("profile-delete-cancel-{id}")))
                                        .cursor_pointer()
                                        .px(px(10.0))
                                        .py(px(4.0))
                                        .rounded(px(4.0))
                                        .bg(rgb(t.bg_secondary))
                                        .hover(|s| s.bg(rgb(t.bg_hover)))
                                        .text_size(ui_text_md(cx))
                                        .text_color(rgb(t.text_primary))
                                        .child("Cancel")
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|this, _, _window, cx| {
                                                this.cancel_delete(cx);
                                            }),
                                        ),
                                ),
                        ),
                )
            })
            .when(!is_deleting, |d| {
                d.child(
                    v_flex()
                        .gap(px(2.0))
                        .child(
                            h_flex()
                                .gap(px(8.0))
                                .child(
                                    div()
                                        .text_size(ui_text_xl(cx))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(rgb(t.text_primary))
                                        .child(display_name.clone()),
                                )
                                .when(is_active, |d| {
                                    d.child(
                                        div()
                                            .px(px(6.0))
                                            .py(px(1.0))
                                            .rounded(px(4.0))
                                            .bg(with_alpha(t.button_primary_bg, 0.2))
                                            .text_size(ui_text_ms(cx))
                                            .text_color(rgb(t.button_primary_bg))
                                            .child("active"),
                                    )
                                }),
                        )
                        .child(
                            div()
                                .text_size(ui_text_ms(cx))
                                .text_color(rgb(t.text_muted))
                                .child(id.clone()),
                        ),
                )
                .child(
                    h_flex()
                        .gap(px(6.0))
                        .child(
                            div()
                                .id(SharedString::from(format!("profile-switch-{id}")))
                                .when(!is_active, |d| d.cursor_pointer())
                                .when(is_active, |d| d.opacity(0.4))
                                .px(px(10.0))
                                .py(px(4.0))
                                .rounded(px(4.0))
                                .bg(rgb(t.button_primary_bg))
                                .hover(|s| if !is_active { s.bg(rgb(t.button_primary_hover)) } else { s })
                                .text_size(ui_text_md(cx))
                                .text_color(rgb(t.button_primary_fg))
                                .child("Switch")
                                .when(!is_active, |d| {
                                    d.on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _window, cx| {
                                            this.switch_to(id_for_switch.clone(), cx);
                                        }),
                                    )
                                }),
                        )
                        .child(
                            div()
                                .id(SharedString::from(format!("profile-delete-{id}")))
                                .when(!is_active && !is_default, |d| d.cursor_pointer())
                                .when(is_active || is_default, |d| d.opacity(0.4))
                                .px(px(8.0))
                                .py(px(4.0))
                                .rounded(px(4.0))
                                .bg(rgb(t.bg_secondary))
                                .when(!is_active && !is_default, |d| {
                                    d.hover(|s| s.bg(with_alpha(t.error, 0.2)))
                                })
                                .text_size(ui_text_md(cx))
                                .text_color(rgb(t.error))
                                .child("Delete")
                                .when(!is_active && !is_default, |d| {
                                    d.on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _window, cx| {
                                            this.confirm_delete(&id_for_delete, cx);
                                        }),
                                    )
                                }),
                        ),
                )
            })
    }
}

impl Render for ProfileManager {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let focus_handle = self.focus_handle.clone();
        let error_message = self.error_message.clone();
        let profiles = self.profiles.clone();
        let new_profile_input = self.new_profile_input.clone();

        if !focus_handle.contains_focused(window, cx) {
            window.focus(&focus_handle, cx);
        }

        modal_backdrop("profile-manager-backdrop", &t)
            .track_focus(&focus_handle)
            .key_context("ProfileManager")
            .items_center()
            .on_action(cx.listener(|this, _: &Cancel, _window, cx| {
                this.close(cx);
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _window, cx| {
                    this.close(cx);
                }),
            )
            .child(
                modal_content("profile-manager-modal", &t)
                    .w(px(520.0))
                    .max_h(px(560.0))
                    .child(modal_header(
                        "Profiles",
                        Some("Switch, create, or delete profiles"),
                        &t,
                        cx,
                        cx.listener(|this, _, _window, cx| this.close(cx)),
                    ))
                    .when(error_message.is_some(), |d| {
                        if let Some(msg) = error_message {
                            d.child(
                                div()
                                    .px(px(16.0))
                                    .py(px(8.0))
                                    .bg(with_alpha(t.error, 0.1))
                                    .border_b_1()
                                    .border_color(rgb(t.border))
                                    .child(
                                        div()
                                            .text_size(ui_text_md(cx))
                                            .text_color(rgb(t.error))
                                            .child(msg),
                                    ),
                            )
                        } else {
                            d
                        }
                    })
                    .child(
                        // Profile list
                        div()
                            .id("profile-list")
                            .flex_1()
                            .overflow_y_scroll()
                            .when(profiles.is_empty(), |d| {
                                d.flex()
                                    .items_center()
                                    .justify_center()
                                    .p(px(32.0))
                                    .child(
                                        div()
                                            .text_size(ui_text_xl(cx))
                                            .text_color(rgb(t.text_muted))
                                            .child("No profiles found"),
                                    )
                            })
                            .when(!profiles.is_empty(), |d| {
                                d.children(
                                    profiles.iter().map(|p| self.render_profile_row(p, cx)),
                                )
                            }),
                    )
                    .child(
                        // Create new profile footer
                        div()
                            .px(px(16.0))
                            .py(px(12.0))
                            .border_t_1()
                            .border_color(rgb(t.border))
                            .child(
                                h_flex()
                                    .gap(px(8.0))
                                    .child(
                                        div()
                                            .id("new-profile-input-wrapper")
                                            .flex_1()
                                            .bg(rgb(t.bg_secondary))
                                            .rounded(px(4.0))
                                            .border_1()
                                            .border_color(rgb(t.border))
                                            .child(SimpleInput::new(&new_profile_input).text_size(ui_text(13.0, cx)))
                                            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                                cx.stop_propagation();
                                            })
                                            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                                                cx.stop_propagation();
                                                if event.keystroke.key.as_str() == "enter" {
                                                    this.create_profile(cx);
                                                }
                                            })),
                                    )
                                    .child(
                                        div()
                                            .id("create-profile-btn")
                                            .cursor_pointer()
                                            .px(px(12.0))
                                            .py(px(8.0))
                                            .rounded(px(4.0))
                                            .bg(rgb(t.button_primary_bg))
                                            .hover(|s| s.bg(rgb(t.button_primary_hover)))
                                            .text_size(ui_text(13.0, cx))
                                            .text_color(rgb(t.button_primary_fg))
                                            .child("Create")
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(|this, _, _window, cx| {
                                                    this.create_profile(cx);
                                                }),
                                            ),
                                    ),
                            ),
                    ),
            )
    }
}
