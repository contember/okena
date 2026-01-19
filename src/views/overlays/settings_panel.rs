//! Settings panel for visual settings configuration
//!
//! Provides a Zed-style settings dialog with sections for font, terminal, and appearance settings.

use crate::settings::{open_settings_file, settings_entity, SettingsState};
use crate::terminal::shell_config::{available_shells, AvailableShell, ShellType};
use crate::theme::{theme, ThemeColors};
use crate::views::components::{dropdown_button, dropdown_option, dropdown_overlay, modal_backdrop, modal_content, modal_header};
use crate::workspace::persistence::get_settings_path;
use gpui::*;
use gpui::prelude::*;

/// Available monospace font families
const FONT_FAMILIES: &[&str] = &[
    "JetBrains Mono",
    "Menlo",
    "SF Mono",
    "Monaco",
    "Fira Code",
    "Source Code Pro",
    "Consolas",
    "DejaVu Sans Mono",
    "Ubuntu Mono",
    "Hack",
];

// ============================================================================
// Reusable UI Components
// ============================================================================

/// Render a section header
fn section_header(title: &str, t: &ThemeColors) -> impl IntoElement {
    div()
        .px(px(16.0))
        .py(px(8.0))
        .text_size(px(11.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(t.text_muted))
        .child(title.to_uppercase())
}

/// Render a settings section container
fn section_container(t: &ThemeColors) -> Div {
    div()
        .mx(px(16.0))
        .mb(px(12.0))
        .rounded(px(6.0))
        .border_1()
        .border_color(rgb(t.border))
        .overflow_hidden()
}

/// Render a settings row container
fn settings_row(id: impl Into<SharedString>, label: &str, t: &ThemeColors, has_border: bool) -> Stateful<Div> {
    let row = div()
        .id(ElementId::Name(id.into()))
        .px(px(12.0))
        .py(px(8.0))
        .flex()
        .items_center()
        .justify_between()
        .child(
            div()
                .text_size(px(13.0))
                .text_color(rgb(t.text_primary))
                .child(label.to_string()),
        );

    if has_border {
        row.border_b_1().border_color(rgb(t.border))
    } else {
        row
    }
}

/// Render a +/- stepper button
fn stepper_button(id: impl Into<SharedString>, label: &str, t: &ThemeColors) -> Stateful<Div> {
    div()
        .id(ElementId::Name(id.into()))
        .cursor_pointer()
        .w(px(24.0))
        .h(px(24.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.0))
        .bg(rgb(t.bg_secondary))
        .hover(|s| s.bg(rgb(t.bg_hover)))
        .text_size(px(14.0))
        .text_color(rgb(t.text_secondary))
        .child(label.to_string())
}

/// Render a value display box
fn value_display(value: String, width: f32, t: &ThemeColors) -> Div {
    div()
        .w(px(width))
        .h(px(24.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.0))
        .bg(rgb(t.bg_secondary))
        .text_size(px(13.0))
        .font_family("monospace")
        .text_color(rgb(t.text_primary))
        .child(value)
}

/// Render a toggle switch
fn toggle_switch(id: impl Into<SharedString>, enabled: bool, t: &ThemeColors) -> Stateful<Div> {
    div()
        .id(ElementId::Name(id.into()))
        .cursor_pointer()
        .w(px(40.0))
        .h(px(22.0))
        .rounded(px(11.0))
        .bg(if enabled { rgb(t.border_active) } else { rgb(t.bg_secondary) })
        .flex()
        .items_center()
        .child(
            div()
                .w(px(18.0))
                .h(px(18.0))
                .rounded_full()
                .bg(rgb(t.text_primary))
                .ml(if enabled { px(20.0) } else { px(2.0) }),
        )
}

// ============================================================================
// Settings Panel
// ============================================================================

/// Settings panel overlay for configuring app settings
pub struct SettingsPanel {
    focus_handle: FocusHandle,
    font_dropdown_open: bool,
    shell_dropdown_open: bool,
    /// Cache of available shells (detected once)
    available_shells: Vec<AvailableShell>,
}

impl SettingsPanel {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            font_dropdown_open: false,
            shell_dropdown_open: false,
            available_shells: available_shells(),
        }
    }

    fn close(&self, cx: &mut Context<Self>) {
        cx.emit(SettingsPanelEvent::Close);
    }

    fn render_number_stepper(
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
            div()
                .flex()
                .items_center()
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

    fn render_integer_stepper(
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
            div()
                .flex()
                .items_center()
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

    fn render_toggle(
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

    fn render_font_dropdown_row(&mut self, current_family: &str, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        settings_row("font-family".to_string(), "Font Family", &t, true).child(
            dropdown_button("font-family-btn", current_family, self.font_dropdown_open, &t)
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                    this.font_dropdown_open = !this.font_dropdown_open;
                    cx.notify();
                })),
        )
    }

    fn render_font_dropdown_overlay(&self, current: &str, cx: &mut Context<Self>) -> impl IntoElement {
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

    fn render_shell_dropdown_row(&mut self, current_shell: &ShellType, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let display_name = current_shell.display_name();

        settings_row("default-shell".to_string(), "Default Shell", &t, true).child(
            dropdown_button("default-shell-btn", &display_name, self.shell_dropdown_open, &t)
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                    this.shell_dropdown_open = !this.shell_dropdown_open;
                    this.font_dropdown_open = false;
                    cx.notify();
                })),
        )
    }

    fn render_shell_dropdown_overlay(&self, current_shell: &ShellType, cx: &mut Context<Self>) -> impl IntoElement {
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

    fn render_content(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let settings = settings_entity(cx);
        let s = settings.read(cx).settings.clone();

        div()
            .id("settings-content")
            .flex_1()
            .overflow_y_scroll()
            // Font section
            .child(section_header("Font", &t))
            .child(
                section_container(&t)
                    .child(self.render_number_stepper(
                        "font-size", "Font Size", s.font_size, "{}", 1.0, 50.0, true,
                        |state, val, cx| state.set_font_size(val, cx), cx,
                    ))
                    .child(self.render_font_dropdown_row(&s.font_family, cx))
                    .child(self.render_number_stepper(
                        "line-height", "Line Height", s.line_height, "{}", 0.1, 50.0, true,
                        |state, val, cx| state.set_line_height(val, cx), cx,
                    ))
                    .child(self.render_number_stepper(
                        "ui-font-size", "UI Font Size", s.ui_font_size, "{}", 1.0, 50.0, false,
                        |state, val, cx| state.set_ui_font_size(val, cx), cx,
                    )),
            )
            // Terminal section
            .child(section_header("Terminal", &t))
            .child(
                section_container(&t)
                    .child(self.render_shell_dropdown_row(&s.default_shell, cx))
                    .child(self.render_toggle(
                        "cursor-blink", "Cursor Blink", s.cursor_blink, true,
                        |state, val, cx| state.set_cursor_blink(val, cx), cx,
                    ))
                    .child(self.render_integer_stepper(
                        "scrollback", "Scrollback Lines", s.scrollback_lines, 1000, 70.0, false,
                        |state, val, cx| state.set_scrollback_lines(val, cx), cx,
                    )),
            )
            // Appearance section
            .child(section_header("Appearance", &t))
            .child(
                section_container(&t)
                    .child(self.render_toggle(
                        "focus-border", "Show Focus Border", s.show_focused_border, false,
                        |state, val, cx| state.set_show_focused_border(val, cx), cx,
                    )),
            )
    }
}

pub enum SettingsPanelEvent {
    Close,
}

impl EventEmitter<SettingsPanelEvent> for SettingsPanel {}

impl Render for SettingsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let focus_handle = self.focus_handle.clone();
        let config_path = get_settings_path();

        window.focus(&focus_handle, cx);

        modal_backdrop("settings-panel-backdrop", &t)
            .track_focus(&focus_handle)
            .key_context("SettingsPanel")
            .items_center()
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key.as_str() == "escape" {
                    if this.font_dropdown_open {
                        this.font_dropdown_open = false;
                        cx.notify();
                    } else if this.shell_dropdown_open {
                        this.shell_dropdown_open = false;
                        cx.notify();
                    } else {
                        this.close(cx);
                    }
                }
            }))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                if this.font_dropdown_open {
                    this.font_dropdown_open = false;
                    cx.notify();
                } else if this.shell_dropdown_open {
                    this.shell_dropdown_open = false;
                    cx.notify();
                } else {
                    this.close(cx);
                }
            }))
            .child(
                modal_content("settings-panel-modal", &t)
                    .relative()
                    .w(px(480.0))
                    .max_h(px(600.0))
                    .child(modal_header(
                        "Settings",
                        None::<&str>,
                        &t,
                        cx.listener(|this, _, _, cx| this.close(cx)),
                    ))
                    // Content
                    .child(self.render_content(cx))
                    // Footer
                    .child(
                        div()
                            .px(px(16.0))
                            .py(px(10.0))
                            .border_t_1()
                            .border_color(rgb(t.border))
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(2.0))
                                    .child(
                                        div()
                                            .text_size(px(11.0))
                                            .text_color(rgb(t.text_muted))
                                            .child("Config:"),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(10.0))
                                            .font_family("monospace")
                                            .text_color(rgb(t.text_secondary))
                                            .child(config_path.display().to_string()),
                                    ),
                            )
                            .child(
                                div()
                                    .id("open-settings-file-btn")
                                    .cursor_pointer()
                                    .px(px(12.0))
                                    .py(px(6.0))
                                    .rounded(px(4.0))
                                    .bg(rgb(t.bg_secondary))
                                    .hover(|s| s.bg(rgb(t.bg_hover)))
                                    .text_size(px(12.0))
                                    .text_color(rgb(t.text_primary))
                                    .child("Open Settings File")
                                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                                        open_settings_file();
                                        this.close(cx);
                                    })),
                            ),
                    )
                    // Font dropdown overlay (rendered last to be on top)
                    .when(self.font_dropdown_open, |modal| {
                        let settings = settings_entity(cx);
                        let current = settings.read(cx).settings.font_family.clone();
                        modal.child(self.render_font_dropdown_overlay(&current, cx))
                    })
                    // Shell dropdown overlay
                    .when(self.shell_dropdown_open, |modal| {
                        let settings = settings_entity(cx);
                        let current = settings.read(cx).settings.default_shell.clone();
                        modal.child(self.render_shell_dropdown_overlay(&current, cx))
                    }),
            )
    }
}

impl_focusable!(SettingsPanel);
