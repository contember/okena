//! Global observable settings module
//!
//! Provides app-wide access to settings through the GlobalSettings global.
//! Settings are automatically persisted to disk with debouncing.

use crate::terminal::session_backend::SessionBackend;
use crate::terminal::shell_config::ShellType;
use crate::workspace::persistence::{load_settings, save_settings, get_settings_path, AppSettings};
use gpui::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Global settings wrapper for app-wide access
#[derive(Clone)]
pub struct GlobalSettings(pub Entity<SettingsState>);

impl Global for GlobalSettings {}

/// Settings state that can be observed and updated
pub struct SettingsState {
    pub settings: AppSettings,
    save_pending: Arc<AtomicBool>,
}

/// Macro to generate setter methods with clamping and auto-save
macro_rules! setting_setter {
    // For f32 values with min/max clamping
    ($fn_name:ident, $field:ident, f32, $min:expr, $max:expr) => {
        pub fn $fn_name(&mut self, value: f32, cx: &mut Context<Self>) {
            self.settings.$field = value.clamp($min, $max);
            self.save_and_notify(cx);
        }
    };
    // For u32 values with min/max clamping
    ($fn_name:ident, $field:ident, u32, $min:expr, $max:expr) => {
        pub fn $fn_name(&mut self, value: u32, cx: &mut Context<Self>) {
            self.settings.$field = value.clamp($min, $max);
            self.save_and_notify(cx);
        }
    };
    // For bool values (no clamping)
    ($fn_name:ident, $field:ident, bool) => {
        pub fn $fn_name(&mut self, value: bool, cx: &mut Context<Self>) {
            self.settings.$field = value;
            self.save_and_notify(cx);
        }
    };
    // For String values (no clamping)
    ($fn_name:ident, $field:ident, String) => {
        pub fn $fn_name(&mut self, value: String, cx: &mut Context<Self>) {
            self.settings.$field = value;
            self.save_and_notify(cx);
        }
    };
}

impl SettingsState {
    pub fn new(settings: AppSettings) -> Self {
        Self {
            settings,
            save_pending: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn get(&self) -> &AppSettings {
        &self.settings
    }

    // Generate all setters using the macro
    setting_setter!(set_font_size, font_size, f32, 8.0, 48.0);
    setting_setter!(set_font_family, font_family, String);
    setting_setter!(set_line_height, line_height, f32, 1.0, 3.0);
    setting_setter!(set_ui_font_size, ui_font_size, f32, 8.0, 24.0);
    setting_setter!(set_file_font_size, file_font_size, f32, 8.0, 24.0);
    setting_setter!(set_cursor_blink, cursor_blink, bool);
    setting_setter!(set_scrollback_lines, scrollback_lines, u32, 100, 100000);
    setting_setter!(set_show_focused_border, show_focused_border, bool);
    setting_setter!(set_show_shell_selector, show_shell_selector, bool);
    /// Set the default shell type for new terminals
    pub fn set_default_shell(&mut self, value: ShellType, cx: &mut Context<Self>) {
        self.settings.default_shell = value;
        self.save_and_notify(cx);
    }

    /// Set the session backend for terminal persistence
    pub fn set_session_backend(&mut self, value: SessionBackend, cx: &mut Context<Self>) {
        self.settings.session_backend = value;
        self.save_and_notify(cx);
    }

    /// Set the file opener command
    pub fn set_file_opener(&mut self, value: String, cx: &mut Context<Self>) {
        self.settings.file_opener = value;
        self.save_and_notify(cx);
    }

    /// Set hook: on_project_open
    pub fn set_hook_on_project_open(&mut self, value: Option<String>, cx: &mut Context<Self>) {
        self.settings.hooks.on_project_open = value;
        self.save_and_notify(cx);
    }

    /// Set hook: on_project_close
    pub fn set_hook_on_project_close(&mut self, value: Option<String>, cx: &mut Context<Self>) {
        self.settings.hooks.on_project_close = value;
        self.save_and_notify(cx);
    }

    /// Set hook: on_worktree_create
    pub fn set_hook_on_worktree_create(&mut self, value: Option<String>, cx: &mut Context<Self>) {
        self.settings.hooks.on_worktree_create = value;
        self.save_and_notify(cx);
    }

    /// Set hook: on_worktree_close
    pub fn set_hook_on_worktree_close(&mut self, value: Option<String>, cx: &mut Context<Self>) {
        self.settings.hooks.on_worktree_close = value;
        self.save_and_notify(cx);
    }

    /// Save and notify - common logic for all setters
    fn save_and_notify(&mut self, cx: &mut Context<Self>) {
        self.save_debounced(cx);
        cx.notify();
    }

    /// Save settings with debouncing to avoid excessive writes
    fn save_debounced(&mut self, cx: &mut Context<Self>) {
        self.save_pending.store(true, Ordering::Relaxed);
        let save_pending = self.save_pending.clone();
        let settings = self.settings.clone();

        cx.spawn(async move |_, _cx| {
            smol::Timer::after(std::time::Duration::from_millis(300)).await;

            if save_pending.swap(false, Ordering::Relaxed) {
                if let Err(e) = save_settings(&settings) {
                    log::error!("Failed to save settings: {}", e);
                }
            }
        })
        .detach();
    }
}

/// Get the global settings entity
pub fn settings_entity(cx: &App) -> Entity<SettingsState> {
    cx.global::<GlobalSettings>().0.clone()
}

/// Get a copy of the current settings
pub fn settings(cx: &App) -> AppSettings {
    settings_entity(cx).read(cx).settings.clone()
}

/// Open the settings file in the default editor
pub fn open_settings_file() {
    let path = get_settings_path();

    if !path.exists() {
        let settings = load_settings();
        let _ = save_settings(&settings);
    }

    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open")
            .arg("-t")
            .arg(&path)
            .spawn();
    }

    #[cfg(target_os = "linux")]
    {
        let _ = std::process::Command::new("xdg-open")
            .arg(&path)
            .spawn();
    }

    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("notepad")
            .arg(&path)
            .spawn();
    }
}

/// Initialize global settings - call this at app startup
pub fn init_settings(cx: &mut App) -> Entity<SettingsState> {
    let settings = load_settings();
    let entity = cx.new(|_cx| SettingsState::new(settings));
    cx.set_global(GlobalSettings(entity.clone()));
    entity
}
