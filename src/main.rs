#[macro_use]
mod macros;
mod app;
mod assets;
mod elements;
mod git;
mod keybindings;
mod remote;
mod settings;
#[cfg(target_os = "linux")]
mod simple_root;
mod terminal;
mod theme;
mod ui;
mod views;
mod workspace;

use gpui::*;
use gpui_component::theme::{Theme as GpuiComponentTheme, ThemeMode as GpuiThemeMode};
#[cfg(not(target_os = "linux"))]
use gpui_component::Root;
#[cfg(target_os = "linux")]
use crate::simple_root::SimpleRoot as Root;
use std::sync::Arc;

use crate::app::Muxy;
use crate::assets::{Assets, embedded_fonts};
use crate::keybindings::{About, Quit};
use crate::terminal::pty_manager::PtyManager;
use crate::theme::{AppTheme, GlobalTheme};
use crate::views::split_pane::init_split_drag_context;
use crate::workspace::persistence;

/// Quit action handler
fn quit(_: &Quit, cx: &mut App) {
    cx.quit();
}

/// About action handler - shows about dialog
fn about(_: &About, _cx: &mut App) {
    // TODO: Show about dialog when GPUI supports it
    log::info!("Muxy - A fast, native terminal multiplexer");
}

/// Set up macOS application menu
fn set_app_menus(cx: &mut App) {
    cx.set_menus(vec![
        Menu {
            name: "Muxy".into(),
            items: vec![
                MenuItem::action("About Muxy", About),
                MenuItem::separator(),
                MenuItem::os_submenu("Services", SystemMenuType::Services),
                MenuItem::separator(),
                MenuItem::action("Quit Muxy", Quit),
            ],
        },
        Menu {
            name: "Edit".into(),
            items: vec![
                MenuItem::os_action("Undo", crate::keybindings::Copy, OsAction::Undo), // Using Copy as placeholder since we need an action
                MenuItem::os_action("Redo", crate::keybindings::Copy, OsAction::Redo),
                MenuItem::separator(),
                MenuItem::os_action("Cut", crate::keybindings::Copy, OsAction::Cut),
                MenuItem::os_action("Copy", crate::keybindings::Copy, OsAction::Copy),
                MenuItem::os_action("Paste", crate::keybindings::Paste, OsAction::Paste),
                MenuItem::os_action("Select All", crate::keybindings::Copy, OsAction::SelectAll),
            ],
        },
    ]);
}

fn main() {
    env_logger::init();

    Application::new().with_assets(Assets).run(|cx: &mut App| {
        // Quit the app when the last window is closed (default on macOS is to keep running)
        cx.set_quit_mode(QuitMode::LastWindowClosed);

        // Register action handlers for menu items
        cx.on_action(quit);
        cx.on_action(about);

        // Set up macOS application menu
        set_app_menus(cx);

        // Register embedded JetBrains Mono font
        cx.text_system()
            .add_fonts(embedded_fonts())
            .expect("Failed to register embedded fonts");

        // Register keybindings
        keybindings::register_keybindings(cx);

        // Initialize split drag context for resize handling
        init_split_drag_context(cx);

        // Initialize global settings entity (must be before workspace load)
        let settings_entity = settings::init_settings(cx);
        let app_settings = settings_entity.read(cx).get().clone();

        // Load or create workspace
        let workspace_data = persistence::load_workspace(app_settings.session_backend).unwrap_or_else(|e| {
            log::warn!("Failed to load workspace: {}, using default", e);
            persistence::default_workspace()
        });

        // Create theme entity from settings
        let theme_entity = cx.new(|_cx| AppTheme::new(app_settings.theme_mode, true)); // Default to dark for initial
        cx.set_global(GlobalTheme(theme_entity.clone()));

        // Create PTY manager with session backend from settings
        let (pty_manager, pty_events) = PtyManager::new(app_settings.session_backend);
        let pty_manager = Arc::new(pty_manager);

        // Create the main window
        cx.open_window(
            WindowOptions {
                // On Windows, disable platform titlebar entirely for custom titlebar
                // On macOS, use transparent titlebar with native traffic lights
                titlebar: if cfg!(target_os = "windows") {
                    None
                } else {
                    Some(TitlebarOptions {
                        title: Some("Muxy".into()),
                        appears_transparent: true,
                        ..Default::default()
                    })
                },
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Point::default(),
                    size: size(px(1200.0), px(800.0)),
                })),
                is_resizable: true,
                // On Windows, use client-side decorations for custom window controls
                window_decorations: Some(if cfg!(target_os = "windows") {
                    WindowDecorations::Client
                } else {
                    WindowDecorations::Server
                }),
                window_min_size: Some(Size {
                    width: px(400.0),
                    height: px(300.0),
                }),
                app_id: Some("muxy".to_string()),
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
                let muxy = cx.new(|cx| {
                    Muxy::new(workspace_data, pty_manager.clone(), pty_events, window, cx)
                });
                cx.new(|cx| Root::new(muxy, window, cx))
            },
        )
        .expect("Failed to create main window");
    });
}
