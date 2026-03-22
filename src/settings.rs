//! Global observable settings module
//!
//! Provides app-wide access to settings through the GlobalSettings global.
//! Settings are automatically persisted to disk with debouncing.

use crate::terminal::session_backend::SessionBackend;
use crate::terminal::shell_config::ShellType;
use crate::theme::ThemeMode;
use crate::views::panels::toast::ToastManager;
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
    /// The worktree path template that was active when settings were loaded or last migrated.
    /// Used to detect meaningful changes and suggest worktree migration.
    worktree_template_baseline: String,
    /// Debounced task for template migration toast (dropped/replaced on each keystroke)
    template_migration_task: Option<gpui::Task<()>>,
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
        let baseline = settings.worktree.path_template.clone();
        Self {
            settings,
            save_pending: Arc::new(AtomicBool::new(false)),
            worktree_template_baseline: baseline,
            template_migration_task: None,
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
    /// Set the cursor style (Block, Bar, Underline)
    pub fn set_cursor_style(&mut self, value: crate::workspace::settings::CursorShape, cx: &mut Context<Self>) {
        self.settings.cursor_style = value;
        self.save_and_notify(cx);
    }

    setting_setter!(set_cursor_blink, cursor_blink, bool);
    setting_setter!(set_scrollback_lines, scrollback_lines, u32, 100, 100000);
    setting_setter!(set_show_focused_border, show_focused_border, bool);
    setting_setter!(set_color_tinted_background, color_tinted_background, bool);
    setting_setter!(set_show_shell_selector, show_shell_selector, bool);
    setting_setter!(set_min_column_width, min_column_width, f32, 100.0, 2000.0);
    setting_setter!(set_idle_timeout_secs, idle_timeout_secs, u32, 0, 300);
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

    /// Set remote server enabled/disabled
    pub fn set_remote_server_enabled(&mut self, value: bool, cx: &mut Context<Self>) {
        self.settings.remote_server_enabled = value;
        self.save_and_notify(cx);
    }

    /// Set the remote server listen address
    pub fn set_remote_listen_address(&mut self, value: String, cx: &mut Context<Self>) {
        self.settings.remote_listen_address = value;
        self.save_and_notify(cx);
    }


    /// Set per-extension settings blob (opaque JSON value).
    pub fn set_extension_setting(&mut self, extension_id: &str, value: serde_json::Value, cx: &mut Context<Self>) {
        self.settings.extension_settings.insert(extension_id.to_string(), value);
        self.save_and_notify(cx);
    }

    /// Enable or disable an extension by ID.
    pub fn set_extension_enabled(&mut self, extension_id: &str, enabled: bool, cx: &mut Context<Self>) {
        if enabled {
            self.settings.enabled_extensions.insert(extension_id.to_string());
        } else {
            self.settings.enabled_extensions.remove(extension_id);
        }
        self.save_and_notify(cx);
    }

    /// Set sidebar open state
    pub fn set_sidebar_open(&mut self, value: bool, cx: &mut Context<Self>) {
        self.settings.sidebar.is_open = value;
        self.save_and_notify(cx);
    }

    /// Set sidebar auto-hide mode
    pub fn set_sidebar_auto_hide(&mut self, value: bool, cx: &mut Context<Self>) {
        self.settings.sidebar.auto_hide = value;
        self.save_and_notify(cx);
    }

    /// Set sidebar width (clamped to min/max bounds)
    pub fn set_sidebar_width(&mut self, value: f32, cx: &mut Context<Self>) {
        use crate::workspace::persistence::{MIN_SIDEBAR_WIDTH, MAX_SIDEBAR_WIDTH};
        self.settings.sidebar.width = value.clamp(MIN_SIDEBAR_WIDTH, MAX_SIDEBAR_WIDTH);
        self.save_and_notify(cx);
    }

    /// Set the theme mode and optional custom theme ID.
    pub fn set_theme_mode(&mut self, value: ThemeMode, cx: &mut Context<Self>) {
        self.settings.theme_mode = value;
        if value != ThemeMode::Custom {
            self.settings.custom_theme_id = None;
        }
        self.save_and_notify(cx);
    }

    /// Set the custom theme ID (file stem, e.g. "example-theme").
    pub fn set_custom_theme_id(&mut self, id: Option<String>, cx: &mut Context<Self>) {
        self.settings.custom_theme_id = id;
        self.save_and_notify(cx);
    }

    /// Set the file opener command
    pub fn set_file_opener(&mut self, value: String, cx: &mut Context<Self>) {
        self.settings.file_opener = value;
        self.save_and_notify(cx);
    }

    // Project hooks
    pub fn set_hook_project_on_open(&mut self, value: Option<String>, cx: &mut Context<Self>) {
        self.settings.hooks.project.on_open = value;
        self.save_and_notify(cx);
    }
    pub fn set_hook_project_on_close(&mut self, value: Option<String>, cx: &mut Context<Self>) {
        self.settings.hooks.project.on_close = value;
        self.save_and_notify(cx);
    }

    // Terminal hooks
    pub fn set_hook_terminal_on_create(&mut self, value: Option<String>, cx: &mut Context<Self>) {
        self.settings.hooks.terminal.on_create = value;
        self.save_and_notify(cx);
    }
    pub fn set_hook_terminal_on_close(&mut self, value: Option<String>, cx: &mut Context<Self>) {
        self.settings.hooks.terminal.on_close = value;
        self.save_and_notify(cx);
    }
    pub fn set_hook_terminal_shell_wrapper(&mut self, value: Option<String>, cx: &mut Context<Self>) {
        self.settings.hooks.terminal.shell_wrapper = value;
        self.save_and_notify(cx);
    }

    // Worktree hooks
    pub fn set_hook_worktree_on_create(&mut self, value: Option<String>, cx: &mut Context<Self>) {
        self.settings.hooks.worktree.on_create = value;
        self.save_and_notify(cx);
    }
    pub fn set_hook_worktree_on_close(&mut self, value: Option<String>, cx: &mut Context<Self>) {
        self.settings.hooks.worktree.on_close = value;
        self.save_and_notify(cx);
    }
    pub fn set_hook_worktree_pre_merge(&mut self, value: Option<String>, cx: &mut Context<Self>) {
        self.settings.hooks.worktree.pre_merge = value;
        self.save_and_notify(cx);
    }
    pub fn set_hook_worktree_post_merge(&mut self, value: Option<String>, cx: &mut Context<Self>) {
        self.settings.hooks.worktree.post_merge = value;
        self.save_and_notify(cx);
    }
    pub fn set_hook_worktree_before_remove(&mut self, value: Option<String>, cx: &mut Context<Self>) {
        self.settings.hooks.worktree.before_remove = value;
        self.save_and_notify(cx);
    }
    pub fn set_hook_worktree_after_remove(&mut self, value: Option<String>, cx: &mut Context<Self>) {
        self.settings.hooks.worktree.after_remove = value;
        self.save_and_notify(cx);
    }
    pub fn set_hook_worktree_on_rebase_conflict(&mut self, value: Option<String>, cx: &mut Context<Self>) {
        self.settings.hooks.worktree.on_rebase_conflict = value;
        self.save_and_notify(cx);
    }
    pub fn set_hook_worktree_on_dirty_close(&mut self, value: Option<String>, cx: &mut Context<Self>) {
        self.settings.hooks.worktree.on_dirty_close = value;
        self.save_and_notify(cx);
    }

    /// Set diff view mode (unified or side-by-side)
    #[allow(dead_code)] // Will be wired via settings_saver callback
    pub fn set_diff_view_mode(&mut self, value: crate::workspace::persistence::DiffViewMode, cx: &mut Context<Self>) {
        self.settings.diff_view_mode = value;
        self.save_and_notify(cx);
    }

    /// Set diff ignore whitespace option
    #[allow(dead_code)] // Will be wired via settings_saver callback
    pub fn set_diff_ignore_whitespace(&mut self, value: bool, cx: &mut Context<Self>) {
        self.settings.diff_ignore_whitespace = value;
        self.save_and_notify(cx);
    }

    /// Set worktree path template.
    /// Shows a migration suggestion toast when the template changes from its baseline value.
    pub fn set_worktree_path_template(&mut self, value: String, cx: &mut Context<Self>) {
        self.settings.worktree.path_template = value.clone();
        self.save_and_notify(cx);

        // Debounced check: after user stops typing, compare with baseline and suggest migration.
        // Storing the task handle cancels the previous debounce timer on each keystroke.
        let baseline = self.worktree_template_baseline.clone();
        self.template_migration_task = Some(cx.spawn(async move |_this, cx| {
            smol::Timer::after(std::time::Duration::from_millis(1500)).await;
            let _ = cx.update(|cx| {
                let current = crate::settings::settings(cx).worktree.path_template.clone();
                if current == value && current != baseline && !baseline.is_empty() {
                    ToastManager::info(
                        "Worktree path template changed. Existing worktrees can be migrated from the context menu.",
                        cx,
                    );
                }
            });
        }));
    }

    /// Set worktree default merge
    pub fn set_worktree_default_merge(&mut self, value: bool, cx: &mut Context<Self>) {
        self.settings.worktree.default_merge = value;
        self.save_and_notify(cx);
    }

    /// Set worktree default stash
    pub fn set_worktree_default_stash(&mut self, value: bool, cx: &mut Context<Self>) {
        self.settings.worktree.default_stash = value;
        self.save_and_notify(cx);
    }

    /// Set worktree default fetch
    pub fn set_worktree_default_fetch(&mut self, value: bool, cx: &mut Context<Self>) {
        self.settings.worktree.default_fetch = value;
        self.save_and_notify(cx);
    }

    /// Set worktree default push
    pub fn set_worktree_default_push(&mut self, value: bool, cx: &mut Context<Self>) {
        self.settings.worktree.default_push = value;
        self.save_and_notify(cx);
    }

    /// Set worktree default delete branch
    pub fn set_worktree_default_delete_branch(&mut self, value: bool, cx: &mut Context<Self>) {
        self.settings.worktree.default_delete_branch = value;
        self.save_and_notify(cx);
    }

    /// Synchronously flush any pending settings save (called on quit)
    pub fn flush_pending_save(&self) {
        if self.save_pending.swap(false, Ordering::Relaxed) {
            if let Err(e) = save_settings(&self.settings) {
                log::error!("Failed to flush settings on quit: {}", e);
            }
        }
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

        cx.spawn(async move |this, cx| {
            smol::Timer::after(std::time::Duration::from_millis(300)).await;

            if save_pending.swap(false, Ordering::Relaxed) {
                let settings = cx.update(|cx| {
                    this.upgrade().map(|e| e.read(cx).settings.clone())
                });
                if let Some(settings) = settings {
                    if let Err(e) = save_settings(&settings) {
                        log::error!("Failed to save settings: {}", e);
                        let _ = cx.update(|cx| {
                            ToastManager::error(format!("Failed to save settings: {}", e), cx);
                        });
                    }
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
        let _ = crate::process::command("open")
            .arg("-t")
            .arg(&path)
            .spawn();
    }

    #[cfg(target_os = "linux")]
    {
        let _ = crate::process::command("xdg-open")
            .arg(&path)
            .spawn();
    }

    #[cfg(target_os = "windows")]
    {
        let _ = crate::process::command("notepad")
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
