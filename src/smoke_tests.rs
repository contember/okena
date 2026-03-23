//! Smoke tests — verify that the app initializes correctly after crate extraction.
//!
//! These tests catch integration issues like missing GPUI globals,
//! mismatched action types, or broken re-exports.

#[cfg(test)]
mod tests {
    use gpui::AppContext as _;

    /// Helper: register all GPUI globals that view crates depend on.
    fn init_globals(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            // Settings entity
            let settings_entity = cx.new(|_cx| {
                crate::settings::SettingsState::new(Default::default())
            });
            cx.set_global(crate::settings::GlobalSettings(settings_entity));

            // Theme — AppTheme is a GPUI Entity, not a Global
            let theme_entity = cx.new(|_cx| crate::theme::AppTheme::new(
                okena_core::theme::ThemeMode::Dark,
                false,
            ));
            cx.set_global(crate::theme::GlobalTheme(theme_entity));

            // Theme provider for view crates
            cx.set_global(okena_ui::theme::GlobalThemeProvider(|cx| {
                crate::theme::theme(cx)
            }));

            // Extension settings store (used by terminal and git view crates)
            cx.set_global(okena_extensions::ExtensionSettingsStore::new(
                |namespace, cx| {
                    let s = crate::settings::settings_entity(cx).read(cx);
                    match namespace {
                        "terminal" => serde_json::to_value(&okena_views_terminal::TerminalViewSettings {
                            font_size: s.settings.font_size,
                            line_height: s.settings.line_height,
                            font_family: s.settings.font_family.clone(),
                            cursor_style: s.settings.cursor_style,
                            cursor_blink: s.settings.cursor_blink,
                            show_focused_border: s.settings.show_focused_border,
                            show_shell_selector: s.settings.show_shell_selector,
                            idle_timeout_secs: s.settings.idle_timeout_secs,
                            color_tinted_background: s.settings.color_tinted_background,
                            file_opener: s.settings.file_opener.clone(),
                            default_shell: s.settings.default_shell.clone(),
                            hooks: s.settings.hooks.clone(),
                        }).ok(),
                        "git" => serde_json::to_value(&okena_views_git::settings::GitViewSettings {
                            diff_view_mode: s.settings.diff_view_mode,
                            diff_ignore_whitespace: s.settings.diff_ignore_whitespace,
                            file_font_size: s.settings.file_font_size,
                            is_dark: true,
                        }).ok(),
                        _ => s.settings.extension_settings.get(namespace).cloned(),
                    }
                },
                |_namespace, _value, _cx| {
                    // no-op for tests
                },
            ));
        });
    }

    #[gpui::test]
    fn smoke_terminal_view_settings_readable(cx: &mut gpui::TestAppContext) {
        init_globals(cx);
        cx.update(|cx| {
            let settings = okena_views_terminal::terminal_view_settings(cx);
            assert!(settings.font_size > 0.0);
            assert!(!settings.font_family.is_empty());
        });
    }

    #[gpui::test]
    fn smoke_git_view_settings_readable(cx: &mut gpui::TestAppContext) {
        init_globals(cx);
        cx.update(|cx| {
            let settings = okena_views_git::settings::git_settings(cx);
            assert!(settings.file_font_size > 0.0);
        });
    }

    #[gpui::test]
    fn smoke_theme_provider_returns_colors(cx: &mut gpui::TestAppContext) {
        init_globals(cx);
        cx.update(|cx| {
            let colors = okena_files::theme::theme(cx);
            // Just verify it doesn't panic and returns valid colors
            assert!(colors.bg_primary != 0 || colors.text_primary != 0);
        });
    }

    #[gpui::test]
    fn smoke_workspace_entity_creates(cx: &mut gpui::TestAppContext) {
        init_globals(cx);
        let _workspace = cx.new(|_cx| {
            okena_workspace::state::Workspace::new(okena_workspace::state::WorkspaceData {
                version: 1,
                projects: vec![],
                project_order: vec![],
                project_widths: Default::default(),
                folders: vec![],
                service_panel_heights: Default::default(),
            })
        });
    }

    #[gpui::test]
    fn smoke_keybinding_actions_are_crate_types(cx: &mut gpui::TestAppContext) {
        init_globals(cx);
        cx.update(|cx| {
            // Register keybindings (this exercises the action type mapping)
            crate::keybindings::register_keybindings(cx);
        });
    }
}
