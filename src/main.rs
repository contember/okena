mod app;
mod assets;
mod elements;
mod git;
mod keybindings;
mod terminal;
mod theme;
mod views;
mod workspace;

use gpui::*;
use gpui_component::theme::{Theme as GpuiComponentTheme, ThemeMode as GpuiThemeMode};
use gpui_component::Root;
use std::sync::Arc;

use crate::app::TermManager;
use crate::assets::{Assets, embedded_fonts};
use crate::terminal::pty_manager::PtyManager;
use crate::theme::{AppTheme, GlobalTheme};
use crate::views::split_pane::init_split_drag_context;
use crate::workspace::persistence;

fn main() {
    env_logger::init();

    Application::new().with_assets(Assets).run(|cx: &mut App| {
        // Register embedded JetBrains Mono font
        cx.text_system()
            .add_fonts(embedded_fonts())
            .expect("Failed to register embedded fonts");

        // Register keybindings
        keybindings::register_keybindings(cx);

        // Initialize split drag context for resize handling
        init_split_drag_context(cx);

        // Load or create workspace
        let workspace_data = persistence::load_workspace().unwrap_or_else(|e| {
            log::warn!("Failed to load workspace: {}, using default", e);
            persistence::default_workspace()
        });

        // Load settings and create theme entity
        let settings = persistence::load_settings();
        let theme_entity = cx.new(|_cx| AppTheme::new(settings.theme_mode, true)); // Default to dark for initial
        cx.set_global(GlobalTheme(theme_entity.clone()));

        // Create PTY manager
        let (pty_manager, pty_events) = PtyManager::new();
        let pty_manager = Arc::new(pty_manager);

        // Create the main window
        cx.open_window(
            WindowOptions {
                titlebar: Some(TitlebarOptions {
                    title: Some("Term Manager".into()),
                    appears_transparent: true,
                    ..Default::default()
                }),
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Point::default(),
                    size: size(px(1200.0), px(800.0)),
                })),
                is_resizable: true,
                window_decorations: Some(WindowDecorations::Server),
                window_min_size: Some(Size {
                    width: px(400.0),
                    height: px(300.0),
                }),
                app_id: Some("term-manager".to_string()),
                ..Default::default()
            },
            |window, cx| {
                // Detect initial system appearance
                let is_dark = matches!(
                    window.appearance(),
                    WindowAppearance::Dark | WindowAppearance::VibrantDark
                );
                theme_entity.update(cx, |theme, _cx| {
                    theme.set_system_appearance(is_dark);
                });

                // Initialize gpui-component with correct theme from start
                gpui_component::init(cx);
                let gpui_mode = if is_dark { GpuiThemeMode::Dark } else { GpuiThemeMode::Light };
                GpuiComponentTheme::change(gpui_mode, Some(window), cx);

                // Set up appearance change observer
                let theme_for_observer = theme_entity.clone();
                window
                    .observe_window_appearance(move |window: &mut Window, cx: &mut App| {
                        let is_dark = matches!(
                            window.appearance(),
                            WindowAppearance::Dark | WindowAppearance::VibrantDark
                        );
                        theme_for_observer.update(cx, |theme, cx| {
                            theme.set_system_appearance(is_dark);
                            cx.notify();
                        });
                        // Sync gpui-component theme
                        let gpui_mode = if is_dark { GpuiThemeMode::Dark } else { GpuiThemeMode::Light };
                        GpuiComponentTheme::change(gpui_mode, Some(window), cx);
                    })
                    .detach();

                // Create the main app view wrapped in Root (required for gpui_component inputs)
                let term_manager = cx.new(|cx| {
                    TermManager::new(workspace_data, pty_manager.clone(), pty_events, settings.show_focused_border, window, cx)
                });
                cx.new(|cx| Root::new(term_manager, window, cx))
            },
        )
        .unwrap();
    });
}
