use crate::keybindings::Cancel;
use crate::settings::settings_entity;
use crate::theme::{
    get_themes_dir, load_custom_themes, theme, theme_entity, with_alpha, ThemeColors, ThemeInfo, ThemeMode,
    DARK_THEME, HIGH_CONTRAST_THEME, LIGHT_THEME, PASTEL_DARK_THEME,
};
use crate::views::components::{
    badge, handle_list_overlay_key, modal_backdrop, modal_content, modal_header, ListOverlayAction,
    ListOverlayConfig, ListOverlayState,
};
use gpui::*;
use gpui_component::h_flex;
use gpui::prelude::*;

/// Theme selection entry with preview and info
#[derive(Clone)]
struct ThemeEntry {
    info: ThemeInfo,
    colors: ThemeColors,
}

/// Theme selector overlay for choosing and previewing themes
pub struct ThemeSelector {
    focus_handle: FocusHandle,
    state: ListOverlayState<ThemeEntry>,
}

impl ThemeSelector {
    pub fn new(cx: &mut Context<Self>) -> Self {
        // Build theme list: built-in + custom
        let mut themes = Vec::new();

        // Add built-in themes
        themes.push(ThemeEntry {
            info: ThemeInfo {
                id: "auto".to_string(),
                name: "Auto".to_string(),
                description: "Follow system appearance".to_string(),
                is_dark: true,
            },
            colors: DARK_THEME, // Preview with dark theme
        });

        themes.push(ThemeEntry {
            info: ThemeInfo {
                id: "dark".to_string(),
                name: "Dark".to_string(),
                description: "Default dark theme (VSCode-like)".to_string(),
                is_dark: true,
            },
            colors: DARK_THEME,
        });

        themes.push(ThemeEntry {
            info: ThemeInfo {
                id: "light".to_string(),
                name: "Light".to_string(),
                description: "Clean light theme".to_string(),
                is_dark: false,
            },
            colors: LIGHT_THEME,
        });

        themes.push(ThemeEntry {
            info: ThemeInfo {
                id: "pastel-dark".to_string(),
                name: "Pastel Dark".to_string(),
                description: "Soft pastel colors on dark background".to_string(),
                is_dark: true,
            },
            colors: PASTEL_DARK_THEME,
        });

        themes.push(ThemeEntry {
            info: ThemeInfo {
                id: "high-contrast".to_string(),
                name: "High Contrast".to_string(),
                description: "High contrast for better visibility".to_string(),
                is_dark: true,
            },
            colors: HIGH_CONTRAST_THEME,
        });

        // Add custom themes
        for (info, colors) in load_custom_themes() {
            themes.push(ThemeEntry { info, colors });
        }

        // Find current theme index
        let current_mode = theme_entity(cx).read(cx).mode;
        let selected_index = match current_mode {
            ThemeMode::Auto => 0,
            ThemeMode::Dark => 1,
            ThemeMode::Light => 2,
            ThemeMode::PastelDark => 3,
            ThemeMode::HighContrast => 4,
            ThemeMode::Custom => {
                // Try to find matching custom theme
                themes.iter().position(|t| t.info.id.starts_with("custom:")).unwrap_or(0)
            }
        };

        let config = ListOverlayConfig::new("Theme")
            .subtitle("Select a color theme for the application")
            .size(480.0, 550.0)
            .centered()
            .key_context("ThemeSelector");

        let state = ListOverlayState::with_selected(themes, config, selected_index, cx);
        let focus_handle = state.focus_handle.clone();

        Self { focus_handle, state }
    }

    fn close(&self, cx: &mut Context<Self>) {
        // Clear any preview before closing
        theme_entity(cx).update(cx, |theme, _cx| {
            theme.clear_preview();
        });
        cx.emit(ThemeSelectorEvent::Close);
    }

    fn select_theme(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.state.items.len() {
            return;
        }

        let theme_entry = &self.state.items[index];
        let theme_ent = theme_entity(cx);

        // Determine the mode from the theme ID
        let (mode, custom_colors) = match theme_entry.info.id.as_str() {
            "auto" => (ThemeMode::Auto, None),
            "dark" => (ThemeMode::Dark, None),
            "light" => (ThemeMode::Light, None),
            "pastel-dark" => (ThemeMode::PastelDark, None),
            "high-contrast" => (ThemeMode::HighContrast, None),
            id if id.starts_with("custom:") => (ThemeMode::Custom, Some(theme_entry.colors)),
            _ => (ThemeMode::Dark, None),
        };

        // Apply the theme
        theme_ent.update(cx, |theme, cx| {
            theme.clear_preview();
            if let Some(colors) = custom_colors {
                theme.set_custom_colors(colors);
            }
            theme.set_mode(mode);
            cx.notify();
        });

        // Save to settings via SettingsState (ensures in-memory and disk stay in sync)
        settings_entity(cx).update(cx, |s, cx| s.set_theme_mode(mode, cx));

        self.state.selected_index = index;
        cx.notify();

        // Close the dialog
        cx.emit(ThemeSelectorEvent::Close);
    }

    fn preview_theme(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.state.items.len() {
            return;
        }

        let theme_entry = &self.state.items[index];
        let theme_ent = theme_entity(cx);

        // Determine the mode for preview
        let mode = match theme_entry.info.id.as_str() {
            "auto" => ThemeMode::Auto,
            "dark" => ThemeMode::Dark,
            "light" => ThemeMode::Light,
            "pastel-dark" => ThemeMode::PastelDark,
            "high-contrast" => ThemeMode::HighContrast,
            id if id.starts_with("custom:") => {
                // For custom themes, set the preview colors directly
                theme_ent.update(cx, |theme, cx| {
                    theme.set_preview_colors(theme_entry.colors);
                    cx.notify();
                });
                return;
            }
            _ => ThemeMode::Dark,
        };

        // Set preview for built-in themes
        theme_ent.update(cx, |theme, cx| {
            theme.set_preview(mode);
            cx.notify();
        });
    }

    fn render_theme_preview(&self, colors: &ThemeColors) -> impl IntoElement {
        // Mini terminal preview with the theme colors
        div()
            .w(px(80.0))
            .h(px(50.0))
            .rounded(px(4.0))
            .bg(rgb(colors.bg_primary))
            .border_1()
            .border_color(rgb(colors.border))
            .p(px(4.0))
            .flex()
            .flex_col()
            .gap(px(2.0))
            .overflow_hidden()
            .child(
                // Fake title bar
                div()
                    .h(px(8.0))
                    .rounded(px(2.0))
                    .bg(rgb(colors.bg_header))
                    .flex()
                    .items_center()
                    .gap(px(2.0))
                    .px(px(2.0))
                    .child(div().w(px(4.0)).h(px(4.0)).rounded_full().bg(rgb(colors.term_red)))
                    .child(div().w(px(4.0)).h(px(4.0)).rounded_full().bg(rgb(colors.term_yellow)))
                    .child(div().w(px(4.0)).h(px(4.0)).rounded_full().bg(rgb(colors.term_green))),
            )
            .child(
                // Fake terminal content
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap(px(1.0))
                    .child(
                        h_flex()
                            .gap(px(2.0))
                            .child(
                                div()
                                    .text_size(px(6.0))
                                    .text_color(rgb(colors.term_green))
                                    .child("$"),
                            )
                            .child(
                                div()
                                    .text_size(px(6.0))
                                    .text_color(rgb(colors.text_primary))
                                    .child("ls"),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(px(4.0))
                            .child(
                                div()
                                    .text_size(px(5.0))
                                    .text_color(rgb(colors.term_blue))
                                    .child("src"),
                            )
                            .child(
                                div()
                                    .text_size(px(5.0))
                                    .text_color(rgb(colors.text_primary))
                                    .child("Cargo.toml"),
                            ),
                    ),
            )
    }

    fn render_theme_row(&self, index: usize, entry: &ThemeEntry, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = theme(cx);
        let is_selected = index == self.state.selected_index;
        let colors = entry.colors;
        let name = entry.info.name.clone();
        let description = entry.info.description.clone();
        let is_custom = entry.info.id.starts_with("custom:");

        div()
            .id(ElementId::Name(format!("theme-{}", index).into()))
            .cursor_pointer()
            .flex()
            .items_center()
            .gap(px(12.0))
            .px(px(12.0))
            .py(px(10.0))
            .border_b_1()
            .border_color(rgb(t.border))
            .when(is_selected, |d| d.bg(with_alpha(t.border_active, 0.15)))
            .hover(|s| s.bg(rgb(t.bg_hover)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _window, cx| {
                    // Preview on click before selection
                    this.preview_theme(index, cx);
                    this.select_theme(index, cx);
                }),
            )
            .child(self.render_theme_preview(&colors))
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(
                        h_flex()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .text_size(px(14.0))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(rgb(t.text_primary))
                                    .child(name),
                            )
                            .when(is_custom, |d| {
                                d.child(badge("Custom", &t))
                            })
                            .when(is_selected, |d| {
                                d.child(
                                    div()
                                        .text_size(px(12.0))
                                        .text_color(rgb(t.border_active))
                                        .child("✓"),
                                )
                            }),
                    )
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(rgb(t.text_muted))
                            .child(description),
                    ),
            )
    }
}

pub enum ThemeSelectorEvent {
    Close,
}

impl EventEmitter<ThemeSelectorEvent> for ThemeSelector {}

impl Render for ThemeSelector {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let focus_handle = self.focus_handle.clone();
        let themes_dir = get_themes_dir();
        let config_width = self.state.config.width;
        let config_max_height = self.state.config.max_height;
        let config_title = self.state.config.title.clone();
        let config_subtitle = self.state.config.subtitle.clone();

        if !focus_handle.is_focused(window) {
            window.focus(&focus_handle, cx);
        }

        modal_backdrop("theme-selector-backdrop", &t)
            .track_focus(&focus_handle)
            .key_context("ThemeSelector")
            .items_center()
            .on_action(cx.listener(|this, _: &Cancel, _window, cx| {
                this.close(cx);
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                match handle_list_overlay_key(&mut this.state, event, &[]) {
                    ListOverlayAction::Close => this.close(cx),
                    ListOverlayAction::SelectPrev | ListOverlayAction::SelectNext => {
                        this.preview_theme(this.state.selected_index, cx);
                        cx.notify();
                    }
                    ListOverlayAction::Confirm => {
                        let index = this.state.selected_index;
                        this.select_theme(index, cx);
                    }
                    _ => {}
                }
            }))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _window, cx| {
                this.close(cx);
            }))
            .child(
                modal_content("theme-selector-modal", &t)
                    .w(px(config_width))
                    .max_h(px(config_max_height))
                    .child(modal_header(
                        config_title,
                        config_subtitle,
                        &t,
                        cx.listener(|this, _, _window, cx| this.close(cx)),
                    ))
                    .child(
                        // Theme list
                        div()
                            .id("theme-list")
                            .flex_1()
                            .overflow_y_scroll()
                            .children(
                                self.state.filtered.iter().enumerate().map(|(i, filter_result)| {
                                    let entry = &self.state.items[filter_result.index];
                                    self.render_theme_row(i, entry, cx)
                                }),
                            ),
                    )
                    .child(
                        // Footer - custom themes info
                        div()
                            .px(px(16.0))
                            .py(px(10.0))
                            .border_t_1()
                            .border_color(rgb(t.border))
                            .flex()
                            .flex_col()
                            .gap(px(4.0))
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(rgb(t.text_muted))
                                    .child("Add custom themes by placing JSON files in:"),
                            )
                            .child(
                                div()
                                    .text_size(px(10.0))
                                    .font_family("monospace")
                                    .text_color(rgb(t.text_secondary))
                                    .child(themes_dir.display().to_string()),
                            ),
                    ),
            )
    }
}

impl_focusable!(ThemeSelector);
