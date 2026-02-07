use crate::keybindings::{
    format_keystroke, get_action_descriptions, get_config, get_keybindings_path, reset_to_defaults,
    Cancel, ShowKeybindings,
};
use crate::theme::theme;
use crate::views::components::{modal_backdrop, modal_content, modal_header};
use gpui::*;
use gpui_component::{h_flex, v_flex};
use gpui::prelude::*;

/// Keybindings help overlay
pub struct KeybindingsHelp {
    focus_handle: FocusHandle,
    show_reset_confirmation: bool,
}

impl KeybindingsHelp {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        Self {
            focus_handle,
            show_reset_confirmation: false,
        }
    }

    fn close(&self, cx: &mut Context<Self>) {
        cx.emit(KeybindingsHelpEvent::Close);
    }

    fn handle_reset_to_defaults(&mut self, cx: &mut Context<Self>) {
        if self.show_reset_confirmation {
            // User confirmed - actually reset
            if let Err(e) = reset_to_defaults() {
                log::error!("Failed to reset keybindings: {}", e);
            }
            self.show_reset_confirmation = false;
            cx.notify();
        } else {
            // Show confirmation
            self.show_reset_confirmation = true;
            cx.notify();
        }
    }

    fn cancel_reset(&mut self, cx: &mut Context<Self>) {
        self.show_reset_confirmation = false;
        cx.notify();
    }

    fn render_category(
        &self,
        category: &str,
        bindings: &[(String, String, bool)], // (action_name, keystroke, is_customized)
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = theme(cx);
        let descriptions = get_action_descriptions();
        let category_string = category.to_string();

        div()
            .mb(px(16.0))
            .child(
                // Category header
                div()
                    .text_size(px(13.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(t.text_primary))
                    .mb(px(8.0))
                    .child(category_string),
            )
            .child(
                // Bindings list
                div()
                    .bg(rgb(t.bg_secondary))
                    .rounded(px(6.0))
                    .border_1()
                    .border_color(rgb(t.border))
                    .children(bindings.iter().enumerate().map(
                        |(i, (action, keystroke, is_customized))| {
                            let description = descriptions
                                .get(action.as_str())
                                .map(|d| d.description)
                                .unwrap_or("Unknown action");
                            let name = descriptions
                                .get(action.as_str())
                                .map(|d| d.name)
                                .unwrap_or(action.as_str());

                            h_flex()
                                .justify_between()
                                .px(px(12.0))
                                .py(px(8.0))
                                .when(i > 0, |d| {
                                    d.border_t_1().border_color(rgb(t.border))
                                })
                                .child(
                                    v_flex()
                                        .gap(px(2.0))
                                        .child(
                                            h_flex()
                                                .gap(px(8.0))
                                                .child(
                                                    div()
                                                        .text_size(px(13.0))
                                                        .text_color(rgb(t.text_primary))
                                                        .child(name.to_string()),
                                                )
                                                .when(*is_customized, |d| {
                                                    d.child(
                                                        div()
                                                            .text_size(px(10.0))
                                                            .px(px(4.0))
                                                            .py(px(1.0))
                                                            .rounded(px(3.0))
                                                            .bg(rgb(t.border_active))
                                                            .text_color(rgb(0xFFFFFF))
                                                            .child("Custom"),
                                                    )
                                                }),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(11.0))
                                                .text_color(rgb(t.text_muted))
                                                .child(description.to_string()),
                                        ),
                                )
                                .child(
                                    div()
                                        .px(px(8.0))
                                        .py(px(4.0))
                                        .rounded(px(4.0))
                                        .bg(rgb(t.bg_primary))
                                        .border_1()
                                        .border_color(rgb(t.border))
                                        .text_size(px(12.0))
                                        .font_family("monospace")
                                        .text_color(rgb(t.text_secondary))
                                        .child(keystroke.clone()),
                                )
                        },
                    )),
            )
    }
}

pub enum KeybindingsHelpEvent {
    Close,
}

impl EventEmitter<KeybindingsHelpEvent> for KeybindingsHelp {}

impl Render for KeybindingsHelp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        // Focus on first render
        window.focus(&self.focus_handle, cx);

        let config = get_config();
        let customized = config.get_customized_actions();
        let conflicts = config.detect_conflicts();

        // Group bindings by category
        let descriptions = get_action_descriptions();
        let mut categories: std::collections::HashMap<&str, Vec<(String, String, bool)>> =
            std::collections::HashMap::new();

        for (action, entries) in &config.bindings {
            let category = descriptions
                .get(action.as_str())
                .map(|d| d.category)
                .unwrap_or("Other");

            for entry in entries {
                if entry.enabled {
                    let is_customized = customized.contains(action);
                    categories
                        .entry(category)
                        .or_insert_with(Vec::new)
                        .push((action.clone(), format_keystroke(&entry.keystroke), is_customized));
                }
            }
        }

        // Sort categories for consistent display
        let category_order = ["Global", "Terminal", "Navigation", "Search", "Fullscreen", "Project", "Other"];

        let focus_handle = self.focus_handle.clone();

        modal_backdrop("keybindings-backdrop", &t)
            .track_focus(&focus_handle)
            .key_context("KeybindingsHelp")
            .items_center()
            .on_action(cx.listener(|this, _: &ShowKeybindings, _window, cx| {
                this.close(cx);
            }))
            .on_action(cx.listener(|this, _: &Cancel, _window, cx| {
                this.close(cx);
            }))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _window, cx| {
                this.close(cx);
            }))
            .child(
                modal_content("keybindings-modal", &t)
                    .w(px(600.0))
                    .max_h(px(700.0))
                    .child(modal_header(
                        "Keyboard Shortcuts",
                        Some("Press ESC to close"),
                        &t,
                        cx.listener(|this, _, _window, cx| this.close(cx)),
                    ))
                    .child(
                        // Conflicts warning
                        div()
                            .when(!conflicts.is_empty(), |d| {
                                d.px(px(16.0))
                                    .py(px(8.0))
                                    .bg(rgb(t.warning))
                                    .border_b_1()
                                    .border_color(rgb(t.border))
                                    .child(
                                        h_flex()
                                            .gap(px(8.0))
                                            .child(
                                                div()
                                                    .text_size(px(14.0))
                                                    .child("⚠️"),
                                            )
                                            .child(
                                                v_flex()
                                                    .gap(px(2.0))
                                                    .child(
                                                        div()
                                                            .text_size(px(12.0))
                                                            .font_weight(FontWeight::MEDIUM)
                                                            .text_color(rgb(t.text_primary))
                                                            .child(format!(
                                                                "{} keybinding conflict{}",
                                                                conflicts.len(),
                                                                if conflicts.len() == 1 { "" } else { "s" }
                                                            )),
                                                    )
                                                    .child(
                                                        div()
                                                            .text_size(px(11.0))
                                                            .text_color(rgb(t.text_secondary))
                                                            .child(
                                                                conflicts
                                                                    .iter()
                                                                    .map(|c| c.to_string())
                                                                    .collect::<Vec<_>>()
                                                                    .join("; "),
                                                            ),
                                                    ),
                                            ),
                                    )
                            }),
                    )
                    .child(
                        // Scrollable content
                        div()
                            .id("keybindings-scroll")
                            .flex_1()
                            .overflow_y_scroll()
                            .px(px(16.0))
                            .py(px(12.0))
                            .children(category_order.iter().filter_map(|category| {
                                categories.get(category).map(|bindings| {
                                    self.render_category(category, bindings, cx)
                                })
                            })),
                    )
                    .child(
                        // Footer
                        div()
                            .px(px(16.0))
                            .py(px(12.0))
                            .border_t_1()
                            .border_color(rgb(t.border))
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                v_flex()
                                    .gap(px(2.0))
                                    .child(
                                        div()
                                            .text_size(px(11.0))
                                            .text_color(rgb(t.text_muted))
                                            .child("Configuration file:"),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(10.0))
                                            .font_family("monospace")
                                            .text_color(rgb(t.text_secondary))
                                            .child(get_keybindings_path().display().to_string()),
                                    ),
                            )
                            .child(
                                // Reset button
                                div()
                                    .when(self.show_reset_confirmation, |d| {
                                        d.flex()
                                            .items_center()
                                            .gap(px(8.0))
                                            .child(
                                                div()
                                                    .text_size(px(12.0))
                                                    .text_color(rgb(t.text_muted))
                                                    .child("Reset all?"),
                                            )
                                            .child(
                                                div()
                                                    .id("reset-confirm-btn")
                                                    .cursor_pointer()
                                                    .px(px(10.0))
                                                    .py(px(6.0))
                                                    .rounded(px(4.0))
                                                    .bg(rgb(t.error))
                                                    .text_size(px(12.0))
                                                    .text_color(rgb(0xFFFFFF))
                                                    .child("Confirm")
                                                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _window, cx| {
                                                        this.handle_reset_to_defaults(cx);
                                                    })),
                                            )
                                            .child(
                                                div()
                                                    .id("reset-cancel-btn")
                                                    .cursor_pointer()
                                                    .px(px(10.0))
                                                    .py(px(6.0))
                                                    .rounded(px(4.0))
                                                    .bg(rgb(t.bg_secondary))
                                                    .hover(|s| s.bg(rgb(t.bg_hover)))
                                                    .text_size(px(12.0))
                                                    .text_color(rgb(t.text_primary))
                                                    .child("Cancel")
                                                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _window, cx| {
                                                        this.cancel_reset(cx);
                                                    })),
                                            )
                                    })
                                    .when(!self.show_reset_confirmation, |d| {
                                        d.child(
                                            div()
                                                .id("reset-defaults-btn")
                                                .cursor_pointer()
                                                .px(px(10.0))
                                                .py(px(6.0))
                                                .rounded(px(4.0))
                                                .bg(rgb(t.bg_secondary))
                                                .hover(|s| s.bg(rgb(t.bg_hover)))
                                                .text_size(px(12.0))
                                                .text_color(rgb(t.text_primary))
                                                .child("Reset to Defaults")
                                                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _window, cx| {
                                                    this.handle_reset_to_defaults(cx);
                                                })),
                                        )
                                    }),
                            ),
                    ),
            )
    }
}

impl_focusable!(KeybindingsHelp);
