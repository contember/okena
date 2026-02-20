#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[macro_use]
mod macros;
mod action_dispatch;
mod app;
mod assets;
mod elements;
mod git;
mod keybindings;
mod process;
mod remote;
mod remote_client;
mod settings;
#[cfg(target_os = "linux")]
mod simple_root;
mod terminal;
mod theme;
mod ui;
mod updater;
mod views;
mod workspace;

use gpui::*;
use gpui_component::theme::{Theme as GpuiComponentTheme, ThemeMode as GpuiThemeMode};
#[cfg(not(target_os = "linux"))]
use gpui_component::Root;
#[cfg(target_os = "linux")]
use crate::simple_root::SimpleRoot as Root;
use std::sync::Arc;

use std::net::IpAddr;

use crate::app::Okena;
use crate::app::headless::HeadlessApp;
use crate::assets::{Assets, embedded_fonts};
use crate::keybindings::{About, Quit, ShowSettings, ShowCommandPalette, ShowThemeSelector, ShowKeybindings};
use crate::settings::GlobalSettings;
use crate::terminal::pty_manager::PtyManager;
use crate::theme::{AppTheme, GlobalTheme};
use crate::views::panels::toast::{Toast, ToastManager};
use crate::workspace::persistence;
use crate::workspace::state::GlobalWorkspace;

/// Quit action handler - flushes pending saves before exiting
fn quit(_: &Quit, cx: &mut App) {
    // Flush pending settings save
    if let Some(gs) = cx.try_global::<GlobalSettings>() {
        gs.0.read(cx).flush_pending_save();
    }

    // Flush pending workspace save
    if let Some(gw) = cx.try_global::<GlobalWorkspace>() {
        if let Err(e) = persistence::save_workspace(gw.0.read(cx).data()) {
            log::error!("Failed to flush workspace on quit: {}", e);
        }
    }

    cx.quit();
}

/// About action handler - shows native macOS about panel
#[cfg(target_os = "macos")]
fn about(_: &About, _cx: &mut App) {
    use std::ffi::c_void;

    // Non-variadic objc_msgSend trampolines — ARM64 requires the standard
    // (non-variadic) calling convention; declaring `...` misplaces arguments.
    #[allow(clashing_extern_declarations)]
    unsafe extern "C" {
        fn objc_getClass(name: *const u8) -> *mut c_void;
        fn sel_registerName(name: *const u8) -> *mut c_void;

        #[link_name = "objc_msgSend"]
        fn msg(obj: *mut c_void, sel: *mut c_void) -> *mut c_void;

        #[link_name = "objc_msgSend"]
        fn msg_str(obj: *mut c_void, sel: *mut c_void, s: *const u8) -> *mut c_void;

        #[link_name = "objc_msgSend"]
        fn msg_id(obj: *mut c_void, sel: *mut c_void, a: *mut c_void) -> *mut c_void;

        #[link_name = "objc_msgSend"]
        fn msg_id2(obj: *mut c_void, sel: *mut c_void, a: *mut c_void, b: *mut c_void) -> *mut c_void;

        #[link_name = "objc_msgSend"]
        fn msg_bytes_len(obj: *mut c_void, sel: *mut c_void, bytes: *const u8, len: usize) -> *mut c_void;
    }

    unsafe {
        let alloc = sel_registerName(b"alloc\0".as_ptr());
        let init_utf8 = sel_registerName(b"initWithUTF8String:\0".as_ptr());
        let ns_string = objc_getClass(b"NSString\0".as_ptr());

        // Helper: create NSString from null-terminated bytes
        let nsstring = |s: &[u8]| -> *mut c_void {
            msg_str(msg(ns_string, alloc), init_utf8, s.as_ptr())
        };

        // Build options dictionary with version
        let dict = msg(
            objc_getClass(b"NSMutableDictionary\0".as_ptr()),
            sel_registerName(b"new\0".as_ptr()),
        );
        let set_obj = sel_registerName(b"setObject:forKey:\0".as_ptr());
        let version_cstr = concat!(env!("CARGO_PKG_VERSION"), "\0");
        msg_id2(
            dict,
            set_obj,
            nsstring(version_cstr.as_bytes()),
            nsstring(b"ApplicationVersion\0"),
        );
        // Set build number to empty to hide the "(x.y.z)" parenthetical
        msg_id2(dict, set_obj, nsstring(b"\0"), nsstring(b"Version\0"));
        // Override copyright from Info.plist to ensure it's always current
        msg_id2(
            dict,
            set_obj,
            nsstring(b"Copyright \xC2\xA9 2026 Contember. All rights reserved.\0"),
            nsstring(b"Copyright\0"),
        );

        // Load embedded app icon as NSImage
        let icon_png = include_bytes!("../assets/logo.png");
        let ns_data = msg_bytes_len(
            objc_getClass(b"NSData\0".as_ptr()),
            sel_registerName(b"dataWithBytes:length:\0".as_ptr()),
            icon_png.as_ptr(),
            icon_png.len(),
        );
        let ns_image = msg_id(
            msg(objc_getClass(b"NSImage\0".as_ptr()), alloc),
            sel_registerName(b"initWithData:\0".as_ptr()),
            ns_data,
        );
        if !ns_image.is_null() {
            msg_id2(dict, set_obj, ns_image, nsstring(b"ApplicationIcon\0"));
        }

        // Credits as attributed string from HTML (supports clickable link)
        let html = b"<div style=\"text-align:center; font-family:-apple-system; font-size:11px;\">Created by Contember Ltd.<br><a href=\"https://contember.com\">contember.com</a></div>";
        let html_data = msg_bytes_len(
            objc_getClass(b"NSData\0".as_ptr()),
            sel_registerName(b"dataWithBytes:length:\0".as_ptr()),
            html.as_ptr(),
            html.len(),
        );
        let credits = msg_id2(
            msg(objc_getClass(b"NSAttributedString\0".as_ptr()), alloc),
            sel_registerName(b"initWithHTML:documentAttributes:\0".as_ptr()),
            html_data,
            std::ptr::null_mut::<c_void>(),
        );
        if !credits.is_null() {
            msg_id2(dict, set_obj, credits, nsstring(b"Credits\0"));
        }

        // [[NSApplication sharedApplication] orderFrontStandardAboutPanelWithOptions:dict]
        let app = msg(
            objc_getClass(b"NSApplication\0".as_ptr()),
            sel_registerName(b"sharedApplication\0".as_ptr()),
        );
        msg_id(
            app,
            sel_registerName(b"orderFrontStandardAboutPanelWithOptions:\0".as_ptr()),
            dict,
        );
    }
}

#[cfg(not(target_os = "macos"))]
fn about(_: &About, _cx: &mut App) {
    log::info!("Okena v{}", env!("CARGO_PKG_VERSION"));
}

/// Set up macOS application menu
fn set_app_menus(cx: &mut App) {
    cx.set_menus(vec![
        Menu {
            name: "Okena".into(),
            items: vec![
                MenuItem::action("About Okena", About),
                MenuItem::separator(),
                MenuItem::action("Settings...", ShowSettings),
                MenuItem::separator(),
                MenuItem::os_submenu("Services", SystemMenuType::Services),
                MenuItem::separator(),
                MenuItem::action("Quit Okena", Quit),
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
        Menu {
            name: "View".into(),
            items: vec![
                MenuItem::action("Command Palette", ShowCommandPalette),
                MenuItem::action("Select Theme", ShowThemeSelector),
                MenuItem::separator(),
                MenuItem::action("Keyboard Shortcuts", ShowKeybindings),
            ],
        },
    ]);
}

/// `okena pair` — generate a pairing code and write it to a file for the running server to validate.
fn cli_pair() -> i32 {
    use crate::remote::auth::{generate_pairing_code, pair_code_path};

    let code = generate_pairing_code();
    let path = pair_code_path();

    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            eprintln!("Failed to create config directory: {e}");
            return 1;
        }
    }

    if let Err(e) = std::fs::write(&path, &code) {
        eprintln!("Failed to write pairing code: {e}");
        return 1;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        if let Err(e) = std::fs::set_permissions(&path, perms) {
            eprintln!("Warning: failed to set file permissions: {e}");
        }
    }

    println!("Pairing code: {code}");
    println!("Expires in 60s — run `okena pair` again for a fresh code.");
    0
}

/// Global handle keeping the headless app entity alive for the process lifetime.
struct GlobalHeadless(#[allow(dead_code)] Entity<HeadlessApp>);
impl Global for GlobalHeadless {}

/// Run the application in headless mode (no GUI, remote server only).
fn run_headless(listen_addr: IpAddr) {
    println!("Starting Okena in headless mode...");

    Application::headless().run(move |cx: &mut App| {
        cx.set_quit_mode(QuitMode::Explicit);

        // Initialize global settings (must be before workspace load)
        let settings_entity = settings::init_settings(cx);
        let app_settings = settings_entity.read(cx).get().clone();

        // Load or create workspace
        let workspace_data = persistence::load_workspace(app_settings.session_backend).unwrap_or_else(|e| {
            log::error!("Failed to load workspace: {}. A backup may have been saved to {:?}. Using default workspace.", e, persistence::get_workspace_path().with_extension("json.bak"));
            persistence::default_workspace()
        });

        // Create PTY manager
        let (pty_manager, pty_events) = PtyManager::new(app_settings.session_backend);
        let pty_manager = Arc::new(pty_manager);

        // Create the headless app entity (starts PTY loop, command loop, and remote server)
        // Must be stored in a global to keep the entity alive — dropping the handle
        // would release the entity and cancel all spawned tasks + drop RemoteServer.
        let headless = cx.new(|cx| {
            HeadlessApp::new(workspace_data, pty_manager, pty_events, listen_addr, cx)
        });
        cx.set_global(GlobalHeadless(headless));
    });
}

fn main() {
    // Handle --version before initializing anything (used by updater validation)
    if std::env::args().any(|a| a == "--version") {
        println!("okena {}", env!("CARGO_PKG_VERSION"));
        return;
    }

    // Handle `okena pair` subcommand before GPUI init
    if std::env::args().nth(1).as_deref() == Some("pair") {
        std::process::exit(cli_pair());
    }

    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn"))
        .init();

    let args: Vec<String> = std::env::args().collect();

    // Parse --remote and --listen flags
    let listen_addr: Option<IpAddr> = {
        if let Some(pos) = args.iter().position(|a| a == "--listen") {
            match args.get(pos + 1) {
                Some(addr_str) => match addr_str.parse::<IpAddr>() {
                    Ok(addr) => Some(addr),
                    Err(_) => {
                        eprintln!("Invalid address for --listen: {addr_str}");
                        eprintln!("Expected an IP address, e.g. --listen 0.0.0.0");
                        std::process::exit(1);
                    }
                },
                None => {
                    eprintln!("--listen requires an address argument, e.g. --listen 0.0.0.0");
                    std::process::exit(1);
                }
            }
        } else if args.iter().any(|a| a == "--remote") {
            // --remote without --listen: force-enable server on localhost
            Some(IpAddr::V4(std::net::Ipv4Addr::LOCALHOST))
        } else {
            None
        }
    };

    // Determine headless mode:
    // 1. Explicit --headless flag
    // 2. Auto-detect on Linux: --listen provided but no DISPLAY/WAYLAND_DISPLAY
    let explicit_headless = args.iter().any(|a| a == "--headless");
    let has_display = std::env::var("DISPLAY").is_ok() || std::env::var("WAYLAND_DISPLAY").is_ok();
    let headless = explicit_headless || (cfg!(target_os = "linux") && listen_addr.is_some() && !has_display);

    if headless {
        if listen_addr.is_none() {
            eprintln!("Headless mode requires --listen <addr>, e.g. --headless --listen 0.0.0.0");
            std::process::exit(1);
        }
        run_headless(listen_addr.unwrap());
        return;
    }

    if !has_display && cfg!(target_os = "linux") {
        eprintln!("No display server found (DISPLAY/WAYLAND_DISPLAY not set).");
        eprintln!("Use --headless --listen <addr> to run without a GUI.");
        std::process::exit(1);
    }

    Application::new().with_assets(Assets).run(move |cx: &mut App| {
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

        // Initialize toast notification system
        cx.set_global(ToastManager::new());

        // Initialize global settings entity (must be before workspace load)
        let settings_entity = settings::init_settings(cx);
        let app_settings = settings_entity.read(cx).get().clone();

        // Load or create workspace
        let workspace_data = persistence::load_workspace(app_settings.session_backend).unwrap_or_else(|e| {
            log::error!("Failed to load workspace: {}. A backup may have been saved to {:?}. Using default workspace.", e, persistence::get_workspace_path().with_extension("json.bak"));
            let backup_path = persistence::get_workspace_path().with_extension("json.bak");
            ToastManager::post(
                Toast::warning(format!("Workspace file was corrupted. A backup was saved to {}. Starting with default workspace.", backup_path.display()))
                    .with_ttl(std::time::Duration::from_secs(15)),
                cx,
            );
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
                        title: Some("Okena".into()),
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
                app_id: Some("okena".to_string()),
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
                let okena = cx.new(|cx| {
                    Okena::new(workspace_data, pty_manager.clone(), pty_events, listen_addr, window, cx)
                });
                cx.new(|cx| Root::new(okena, window, cx))
            },
        )
        .expect("Failed to create main window");
    });
}
