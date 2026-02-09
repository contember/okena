use okena_core::client::RemoteConnectionConfig;
use crate::terminal::session_backend::SessionBackend;
use crate::terminal::shell_config::ShellType;
use crate::theme::ThemeMode;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

/// Terminal cursor shape.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum CursorShape {
    /// Full-cell block cursor (default, Linux-style)
    #[default]
    Block,
    /// Thin vertical bar cursor (editor-style)
    Bar,
    /// Horizontal underline cursor
    Underline,
}

impl CursorShape {
    pub fn display_name(self) -> &'static str {
        match self {
            CursorShape::Block => "Block",
            CursorShape::Bar => "Bar",
            CursorShape::Underline => "Underline",
        }
    }

    pub fn all_variants() -> &'static [CursorShape] {
        &[CursorShape::Block, CursorShape::Bar, CursorShape::Underline]
    }
}

/// Display mode for the diff viewer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiffViewMode {
    #[default]
    Unified,
    SideBySide,
}

impl DiffViewMode {
    /// Toggle between view modes.
    pub fn toggle(self) -> Self {
        match self {
            DiffViewMode::Unified => DiffViewMode::SideBySide,
            DiffViewMode::SideBySide => DiffViewMode::Unified,
        }
    }
}

/// Configuration for project lifecycle hooks
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct HooksConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_project_open: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_project_close: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_worktree_create: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_worktree_close: Option<String>,
}

/// Default sidebar width in pixels.
pub const DEFAULT_SIDEBAR_WIDTH: f32 = 250.0;
/// Minimum sidebar width in pixels.
pub const MIN_SIDEBAR_WIDTH: f32 = 150.0;
/// Maximum sidebar width in pixels.
pub const MAX_SIDEBAR_WIDTH: f32 = 500.0;

fn default_sidebar_width() -> f32 {
    DEFAULT_SIDEBAR_WIDTH
}

/// Sidebar settings
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SidebarSettings {
    /// Whether the sidebar is open
    #[serde(default)]
    pub is_open: bool,
    /// Whether auto-hide mode is enabled
    #[serde(default)]
    pub auto_hide: bool,
    /// Sidebar width in pixels
    #[serde(default = "default_sidebar_width")]
    pub width: f32,
}

impl Default for SidebarSettings {
    fn default() -> Self {
        Self {
            is_open: false,
            auto_hide: false,
            width: DEFAULT_SIDEBAR_WIDTH,
        }
    }
}

/// Current settings schema version - increment when making breaking changes
pub const SETTINGS_VERSION: u32 = 1;

/// App settings (persisted separately from workspace)
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AppSettings {
    /// Settings schema version for migration support
    #[serde(default = "default_settings_version")]
    pub version: u32,
    #[serde(default)]
    pub theme_mode: ThemeMode,
    /// Name of the currently active session (None = default workspace.json)
    #[serde(default)]
    pub active_session: Option<String>,
    /// Sidebar settings
    #[serde(default)]
    pub sidebar: SidebarSettings,
    /// Whether to show border around focused terminal
    #[serde(default = "default_show_focused_border")]
    pub show_focused_border: bool,

    // Font settings
    /// Terminal font size (default: 14.0)
    #[serde(default = "default_font_size")]
    pub font_size: f32,
    /// Terminal font family (default: "JetBrains Mono")
    #[serde(default = "default_font_family")]
    pub font_family: String,
    /// Line height multiplier (default: 1.3)
    #[serde(default = "default_line_height")]
    pub line_height: f32,
    /// UI font size for panels/dialogs (default: 13.0)
    #[serde(default = "default_ui_font_size")]
    pub ui_font_size: f32,
    /// File viewer/diff viewer font size (default: 12.0)
    #[serde(default = "default_file_font_size")]
    pub file_font_size: f32,

    // Terminal settings
    /// Cursor shape: Block, Bar, or Underline (default: Block)
    #[serde(default)]
    pub cursor_style: CursorShape,
    /// Enable cursor blinking (default: false)
    #[serde(default = "default_cursor_blink")]
    pub cursor_blink: bool,
    /// Number of scrollback lines (default: 10000)
    #[serde(default = "default_scrollback_lines")]
    pub scrollback_lines: u32,

    // Shell settings
    /// Default shell type for new terminals
    #[serde(default)]
    pub default_shell: ShellType,
    /// Show shell selector in terminal header (default: false)
    #[serde(default)]
    pub show_shell_selector: bool,

    // Session persistence settings
    /// Session backend for terminal persistence (tmux/screen/none/auto)
    #[serde(default)]
    pub session_backend: SessionBackend,

    // File opener settings
    /// Editor command to open file paths (e.g. "code", "cursor", "zed", "subl", "vim")
    /// Empty string = use system default (open/xdg-open/start)
    #[serde(default = "default_file_opener")]
    pub file_opener: String,

    /// Global lifecycle hooks (can be overridden per-project)
    #[serde(default)]
    pub hooks: HooksConfig,

    /// Diff viewer display mode (unified or side-by-side)
    #[serde(default)]
    pub diff_view_mode: DiffViewMode,

    /// Enable remote control server (default: false)
    #[serde(default)]
    pub remote_server_enabled: bool,

    /// Listen address for the remote server (default: "127.0.0.1")
    #[serde(default = "default_remote_listen_address")]
    pub remote_listen_address: String,

    /// Whether to ignore whitespace changes in diff viewer
    #[serde(default)]
    pub diff_ignore_whitespace: bool,

    /// Enable automatic update checking (default: true)
    #[serde(default = "default_auto_update_enabled")]
    pub auto_update_enabled: bool,

    /// Saved remote connections for the client feature
    #[serde(default)]
    pub remote_connections: Vec<RemoteConnectionConfig>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            version: SETTINGS_VERSION,
            theme_mode: ThemeMode::default(),
            active_session: None,
            sidebar: SidebarSettings::default(),
            show_focused_border: default_show_focused_border(),
            font_size: default_font_size(),
            font_family: default_font_family(),
            line_height: default_line_height(),
            ui_font_size: default_ui_font_size(),
            file_font_size: default_file_font_size(),
            cursor_style: CursorShape::default(),
            cursor_blink: default_cursor_blink(),
            scrollback_lines: default_scrollback_lines(),
            default_shell: ShellType::default(),
            show_shell_selector: false,
            session_backend: SessionBackend::default(),
            file_opener: default_file_opener(),
            hooks: HooksConfig::default(),
            diff_view_mode: DiffViewMode::default(),
            remote_server_enabled: false,
            remote_listen_address: default_remote_listen_address(),
            diff_ignore_whitespace: false,
            auto_update_enabled: default_auto_update_enabled(),
            remote_connections: Vec::new(),
        }
    }
}

fn default_settings_version() -> u32 {
    // Return 0 for settings files without version field (pre-versioning)
    0
}

fn default_show_focused_border() -> bool {
    false
}

fn default_auto_update_enabled() -> bool {
    true
}

fn default_font_size() -> f32 {
    14.0
}

fn default_font_family() -> String {
    "JetBrains Mono".to_string()
}

fn default_line_height() -> f32 {
    1.3
}

fn default_ui_font_size() -> f32 {
    13.0
}

fn default_file_font_size() -> f32 {
    12.0
}

fn default_cursor_blink() -> bool {
    false
}

fn default_scrollback_lines() -> u32 {
    10000
}

fn default_file_opener() -> String {
    String::new()
}

fn default_remote_listen_address() -> String {
    "127.0.0.1".to_string()
}

/// Get the settings file path
pub fn get_settings_path() -> std::path::PathBuf {
    super::persistence::get_config_dir().join("settings.json")
}

/// Load app settings from disk with robust error handling and migration support
pub fn load_settings() -> AppSettings {
    let path = get_settings_path();

    if !path.exists() {
        log::info!("Settings file not found at {}, using defaults", path.display());
        return AppSettings::default();
    }

    let content = match std::fs::read_to_string(&path) {
        Ok(content) => content,
        Err(e) => {
            log::error!("Failed to read settings file {}: {}", path.display(), e);
            return AppSettings::default();
        }
    };

    // First, try direct deserialization (fast path for valid settings)
    match serde_json::from_str::<AppSettings>(&content) {
        Ok(mut settings) => {
            // Run migrations if needed
            settings = migrate_settings(settings);
            return settings;
        }
        Err(e) => {
            log::warn!("Failed to parse settings directly: {}, attempting partial recovery", e);
        }
    }

    // Fallback: partial recovery using serde_json::Value
    match recover_settings_from_json(&content) {
        Ok(mut settings) => {
            log::info!("Successfully recovered settings with partial data");
            settings = migrate_settings(settings);
            // Save the recovered settings to fix the file
            if let Err(e) = save_settings(&settings) {
                log::warn!("Failed to save recovered settings: {}", e);
            }
            settings
        }
        Err(e) => {
            log::error!("Failed to recover settings from {}: {}", path.display(), e);
            log::error!("Using default settings. Your old settings file has been preserved.");
            AppSettings::default()
        }
    }
}

/// Attempt to recover settings from a potentially malformed JSON file
/// This extracts valid fields and uses defaults for invalid/missing ones
fn recover_settings_from_json(content: &str) -> Result<AppSettings> {
    use anyhow::Context;
    use crate::theme::ThemeMode;

    let value: serde_json::Value = serde_json::from_str(content)
        .context("Settings file is not valid JSON")?;

    let obj = value.as_object()
        .context("Settings file root is not a JSON object")?;

    let mut settings = AppSettings::default();

    // Try to recover each field individually
    if let Some(v) = obj.get("version").and_then(|v| v.as_u64()) {
        settings.version = v as u32;
    }

    if let Some(v) = obj.get("theme_mode") {
        if let Ok(theme) = serde_json::from_value::<ThemeMode>(v.clone()) {
            settings.theme_mode = theme;
        } else {
            log::warn!("Could not parse theme_mode, using default");
        }
    }

    if let Some(v) = obj.get("active_session") {
        if let Ok(session) = serde_json::from_value::<Option<String>>(v.clone()) {
            settings.active_session = session;
        }
    }

    if let Some(v) = obj.get("sidebar") {
        if let Ok(sidebar) = serde_json::from_value::<SidebarSettings>(v.clone()) {
            settings.sidebar = sidebar;
        } else {
            log::warn!("Could not parse sidebar settings, using default");
        }
    }

    if let Some(v) = obj.get("show_focused_border").and_then(|v| v.as_bool()) {
        settings.show_focused_border = v;
    }

    if let Some(v) = obj.get("font_size").and_then(|v| v.as_f64()) {
        settings.font_size = (v as f32).clamp(8.0, 48.0);
    }

    if let Some(v) = obj.get("font_family").and_then(|v| v.as_str()) {
        settings.font_family = v.to_string();
    }

    if let Some(v) = obj.get("line_height").and_then(|v| v.as_f64()) {
        settings.line_height = (v as f32).clamp(1.0, 3.0);
    }

    if let Some(v) = obj.get("ui_font_size").and_then(|v| v.as_f64()) {
        settings.ui_font_size = (v as f32).clamp(8.0, 24.0);
    }

    if let Some(v) = obj.get("file_font_size").and_then(|v| v.as_f64()) {
        settings.file_font_size = (v as f32).clamp(8.0, 24.0);
    }

    if let Some(v) = obj.get("cursor_style") {
        if let Ok(style) = serde_json::from_value::<CursorShape>(v.clone()) {
            settings.cursor_style = style;
        }
    }

    if let Some(v) = obj.get("cursor_blink").and_then(|v| v.as_bool()) {
        settings.cursor_blink = v;
    }

    if let Some(v) = obj.get("scrollback_lines").and_then(|v| v.as_u64()) {
        settings.scrollback_lines = (v as u32).clamp(100, 100000);
    }

    if let Some(v) = obj.get("file_opener").and_then(|v| v.as_str()) {
        settings.file_opener = v.to_string();
    }

    if let Some(v) = obj.get("auto_update_enabled").and_then(|v| v.as_bool()) {
        settings.auto_update_enabled = v;
    }

    Ok(settings)
}

/// Migrate settings from older versions to the current version
fn migrate_settings(mut settings: AppSettings) -> AppSettings {
    let original_version = settings.version;

    // Migration from version 0 (pre-versioning) to version 1
    if settings.version == 0 {
        log::info!("Migrating settings from pre-versioning (v0) to v1");
        // No structural changes needed for v0 -> v1, just mark as migrated
        settings.version = 1;
    }

    // Future migrations would go here:
    // if settings.version == 1 {
    //     log::info!("Migrating settings from v1 to v2");
    //     // Perform v1 -> v2 migration
    //     settings.version = 2;
    // }

    // Ensure version is current
    if settings.version < SETTINGS_VERSION {
        log::warn!(
            "Settings version {} is older than current version {}, some settings may use defaults",
            original_version,
            SETTINGS_VERSION
        );
        settings.version = SETTINGS_VERSION;
    }

    // Save if we migrated
    if original_version != settings.version {
        log::info!("Settings migrated from v{} to v{}", original_version, settings.version);
        if let Err(e) = save_settings(&settings) {
            log::warn!("Failed to save migrated settings: {}", e);
        }
    }

    settings
}

/// Save app settings to disk
pub fn save_settings(settings: &AppSettings) -> Result<()> {
    let path = get_settings_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let content = serde_json::to_string_pretty(settings)?;
    std::fs::write(&path, content)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// Process-level mutex for settings file access.
static SETTINGS_LOCK: Mutex<()> = Mutex::new(());

/// Atomically load, update, and save the `remote_connections` field in settings.
///
/// Uses a process-level mutex to prevent concurrent read-modify-write races.
/// On Unix, also uses file locking (flock) for cross-process safety.
pub fn update_remote_connections<F>(updater: F) -> Result<()>
where
    F: FnOnce(&mut Vec<RemoteConnectionConfig>),
{
    let _guard = SETTINGS_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    let path = get_settings_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    #[cfg(unix)]
    {
        use std::io::{Read, Write, Seek};
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)?;

        // Acquire exclusive file lock
        unsafe { libc::flock(std::os::unix::io::AsRawFd::as_raw_fd(&file), libc::LOCK_EX) };

        let mut content = String::new();
        file.read_to_string(&mut content)?;

        let mut settings: AppSettings = if content.is_empty() {
            AppSettings::default()
        } else {
            serde_json::from_str(&content).unwrap_or_default()
        };

        updater(&mut settings.remote_connections);

        let new_content = serde_json::to_string_pretty(&settings)?;
        file.seek(std::io::SeekFrom::Start(0))?;
        file.set_len(0)?;
        file.write_all(new_content.as_bytes())?;

        // Set restrictive permissions
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));

        // Lock is released automatically when `file` is dropped
        return Ok(());
    }

    #[cfg(not(unix))]
    {
        let mut settings = load_settings();
        updater(&mut settings.remote_connections);
        save_settings(&settings)?;
        Ok(())
    }
}
