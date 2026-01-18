use crate::theme::{
    get_themes_dir, load_custom_themes, theme, theme_entity, ThemeColors, ThemeInfo, ThemeMode,
    DARK_THEME, HIGH_CONTRAST_THEME, LIGHT_THEME, PASTEL_DARK_THEME,
};
use crate::workspace::persistence::{load_settings, save_settings};
use gpui::*;
use gpui::prelude::*;

/// Create an hsla color from a hex color with custom alpha
fn with_alpha(hex: u32, alpha: f32) -> Hsla {
    let rgba = rgb(hex);
    Hsla::from(Rgba { a: alpha, ..rgba })
}

/// Theme selection entry with preview and info
#[derive(Clone)]
struct ThemeEntry {
    info: ThemeInfo,
    colors: ThemeColors,
}

/// Theme selector overlay for choosing and previewing themes
pub struct ThemeSelector {
    focus_handle: FocusHandle,
    themes: Vec<ThemeEntry>,
    selected_index: usize,
}

impl ThemeSelector {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();

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

        Self {
            focus_handle,
            themes,
            selected_index,
        }
    }

    fn close(&self, cx: &mut Context<Self>) {
        // Clear any preview before closing
        theme_entity(cx).update(cx, |theme, _cx| {
            theme.clear_preview();
        });
        cx.emit(ThemeSelectorEvent::Close);
    }

    fn select_theme(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.themes.len() {
            return;
        }

        let theme_entry = &self.themes[index];
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

        // Save to settings
        let mut settings = load_settings();
        settings.theme_mode = mode;
        let _ = save_settings(&settings);

        self.selected_index = index;
        cx.notify();

        // Close the dialog
        cx.emit(ThemeSelectorEvent::Close);
    }

    fn preview_theme(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.themes.len() {
            return;
        }

        let theme_entry = &self.themes[index];
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
                        div()
                            .flex()
                            .items_center()
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

    fn render_theme_row(&self, index: usize, entry: &ThemeEntry, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let is_selected = index == self.selected_index;
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
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .text_size(px(14.0))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(rgb(t.text_primary))
                                    .child(name),
                            )
                            .when(is_custom, |d| {
                                d.child(
                                    div()
                                        .px(px(6.0))
                                        .py(px(1.0))
                                        .rounded(px(3.0))
                                        .bg(rgb(t.bg_secondary))
                                        .text_size(px(9.0))
                                        .text_color(rgb(t.text_muted))
                                        .child("Custom"),
                                )
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
        let themes = self.themes.clone();
        let themes_dir = get_themes_dir();

        // Focus on first render
        window.focus(&focus_handle, cx);

        div()
            .track_focus(&focus_handle)
            .key_context("ThemeSelector")
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                match event.keystroke.key.as_str() {
                    "escape" => {
                        this.close(cx);
                    }
                    "up" => {
                        if this.selected_index > 0 {
                            this.selected_index -= 1;
                            this.preview_theme(this.selected_index, cx);
                            cx.notify();
                        }
                    }
                    "down" => {
                        if this.selected_index < this.themes.len() - 1 {
                            this.selected_index += 1;
                            this.preview_theme(this.selected_index, cx);
                            cx.notify();
                        }
                    }
                    "enter" => {
                        let index = this.selected_index;
                        this.select_theme(index, cx);
                    }
                    _ => {}
                }
            }))
            .absolute()
            .inset_0()
            .bg(hsla(0.0, 0.0, 0.0, 0.5))
            .flex()
            .items_center()
            .justify_center()
            .id("theme-selector-backdrop")
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _window, cx| {
                    this.close(cx);
                }),
            )
            .child(
                // Modal content
                div()
                    .id("theme-selector-modal")
                    .w(px(480.0))
                    .max_h(px(550.0))
                    .bg(rgb(t.bg_primary))
                    .rounded(px(8.0))
                    .border_1()
                    .border_color(rgb(t.border))
                    .shadow_xl()
                    .flex()
                    .flex_col()
                    .on_mouse_down(MouseButton::Left, |_, _window, _cx| {})
                    .child(
                        // Header
                        div()
                            .px(px(16.0))
                            .py(px(12.0))
                            .flex()
                            .items_center()
                            .justify_between()
                            .border_b_1()
                            .border_color(rgb(t.border))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(2.0))
                                    .child(
                                        div()
                                            .text_size(px(16.0))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(t.text_primary))
                                            .child("Theme"),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.0))
                                            .text_color(rgb(t.text_muted))
                                            .child("Select a color theme for the application"),
                                    ),
                            )
                            .child(
                                div()
                                    .id("theme-selector-close-btn")
                                    .cursor_pointer()
                                    .px(px(8.0))
                                    .py(px(4.0))
                                    .rounded(px(4.0))
                                    .hover(|s| s.bg(rgb(t.bg_hover)))
                                    .text_size(px(16.0))
                                    .text_color(rgb(t.text_muted))
                                    .child("✕")
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _window, cx| {
                                            this.close(cx);
                                        }),
                                    ),
                            ),
                    )
                    .child(
                        // Theme list
                        div()
                            .id("theme-list")
                            .flex_1()
                            .overflow_y_scroll()
                            .children(
                                themes
                                    .iter()
                                    .enumerate()
                                    .map(|(i, entry)| self.render_theme_row(i, entry, cx)),
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

impl Focusable for ThemeSelector {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}
