use okena_core::client::RemoteConnectionConfig;
use okena_terminal::session_backend::SessionBackend;
use okena_terminal::shell_config::ShellType;
use okena_core::theme::ThemeMode;
pub use okena_core::types::DiffViewMode;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
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

// Hook configuration types live in `okena-state` to keep them GPUI-free.
pub use okena_state::{HooksConfig, ProjectHooks, TerminalHooks, WorktreeHooks};

/// Configuration for worktree creation and close defaults
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorktreeConfig {
    /// Path template for new worktrees.
    /// Supports relative paths (resolved from project dir). Variables: {repo} = repo folder name, {branch} = branch name
    #[serde(default = "default_worktree_path_template")]
    pub path_template: String,
    /// Default: enable merge on close
    #[serde(default)]
    pub default_merge: bool,
    /// Default: enable stash on close
    #[serde(default)]
    pub default_stash: bool,
    /// Default: enable fetch on close
    #[serde(default = "default_true")]
    pub default_fetch: bool,
    /// Default: enable push on close
    #[serde(default)]
    pub default_push: bool,
    /// Default: enable delete branch on close
    #[serde(default)]
    pub default_delete_branch: bool,
}

impl Default for WorktreeConfig {
    fn default() -> Self {
        Self {
            path_template: default_worktree_path_template(),
            default_merge: false,
            default_stash: false,
            default_fetch: true,
            default_push: false,
            default_delete_branch: false,
        }
    }
}

fn default_worktree_path_template() -> String {
    "../{repo}-wt/{branch}".to_string()
}

fn default_true() -> bool {
    true
}

/// Window state for a detached overlay (windowed / maximized / fullscreen).
/// The bounds in `DetachedWindowBounds` are the *restore* bounds — what the
/// window snaps back to when leaving maximized or fullscreen mode.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum DetachedWindowState {
    #[default]
    Windowed,
    Maximized,
    Fullscreen,
}

/// Last-used bounds of a detached overlay window. Persisted so the window
/// reopens at the same position, size, and state (incl. maximized/fullscreen)
/// instead of resetting to a small default each time.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct DetachedWindowBounds {
    pub origin_x: f32,
    pub origin_y: f32,
    pub width: f32,
    pub height: f32,
    #[serde(default)]
    pub state: DetachedWindowState,
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
pub const SETTINGS_VERSION: u32 = 3;

/// App settings (persisted separately from workspace)
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AppSettings {
    /// Settings schema version for migration support
    #[serde(default = "default_settings_version")]
    pub version: u32,
    #[serde(default)]
    pub theme_mode: ThemeMode,
    /// Custom theme file stem (e.g. "example-theme" for themes/example-theme.json).
    /// Only used when `theme_mode` is `Custom`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_theme_id: Option<String>,
    /// Name of the currently active session (None = default workspace.json)
    #[serde(default)]
    pub active_session: Option<String>,
    /// Sidebar settings
    #[serde(default)]
    pub sidebar: SidebarSettings,
    /// Whether to show border around focused terminal
    #[serde(default = "default_show_focused_border")]
    pub show_focused_border: bool,
    /// Tint project backgrounds with the folder color
    #[serde(default)]
    pub color_tinted_background: bool,

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

    /// Minimum project column width in pixels (default: 400)
    #[serde(default = "default_min_column_width")]
    pub min_column_width: f32,

    /// Whether to ignore whitespace changes in diff viewer
    #[serde(default)]
    pub diff_ignore_whitespace: bool,

    /// When true, file viewer / diff viewer (and other detachable overlays)
    /// open directly in a separate OS window instead of as a modal.
    #[serde(default)]
    pub detached_overlays_by_default: bool,

    /// Last bounds used by a detached overlay window. Restored on next open
    /// so the window doesn't reset to a small default each time.
    #[serde(default)]
    pub detached_overlay_bounds: Option<DetachedWindowBounds>,

    /// Legacy: auto_update_enabled flag. Migrated to enabled_extensions.
    #[serde(default = "default_auto_update_enabled", skip_serializing)]
    auto_update_enabled: bool,

    /// Set of enabled extension IDs (replaces per-extension bool flags).
    #[serde(default)]
    pub enabled_extensions: HashSet<String>,

    /// Per-extension settings (keyed by extension ID).
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub extension_settings: HashMap<String, serde_json::Value>,

    /// Legacy: Claude Code integration flag. Migrated to enabled_extensions.
    #[serde(default, skip_serializing)]
    claude_code_integration: bool,

    /// Legacy: Codex integration flag. Migrated to enabled_extensions.
    #[serde(default, skip_serializing)]
    codex_integration: bool,

    /// Idle timeout in seconds for "waiting for input" detection (default: 5, 0 = disabled)
    #[serde(default = "default_idle_timeout_secs")]
    pub idle_timeout_secs: u32,

    /// Worktree creation and close defaults
    #[serde(default)]
    pub worktree: WorktreeConfig,

    /// Saved remote connections for the client feature
    #[serde(default)]
    pub remote_connections: Vec<RemoteConnectionConfig>,

    /// When true, Ctrl+C in a terminal pane copies the active selection (and clears it)
    /// instead of sending SIGINT. Ctrl+C without a selection still sends SIGINT.
    /// Ctrl+Shift+C continues to copy unconditionally regardless of this setting.
    /// Default: false (Ctrl+C always sends SIGINT — matches GNOME Terminal / Kitty).
    #[serde(default)]
    pub terminal_ctrl_c_copies_selection: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            version: SETTINGS_VERSION,
            custom_theme_id: None,
            theme_mode: ThemeMode::default(),
            active_session: None,
            sidebar: SidebarSettings::default(),
            show_focused_border: default_show_focused_border(),
            color_tinted_background: false,
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
            min_column_width: default_min_column_width(),
            diff_ignore_whitespace: false,
            detached_overlays_by_default: false,
            detached_overlay_bounds: None,
            auto_update_enabled: default_auto_update_enabled(),
            enabled_extensions: HashSet::new(),
            extension_settings: HashMap::new(),
            claude_code_integration: false,
            codex_integration: false,
            idle_timeout_secs: default_idle_timeout_secs(),
            worktree: WorktreeConfig::default(),
            remote_connections: Vec::new(),
            terminal_ctrl_c_copies_selection: false,
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

fn default_min_column_width() -> f32 {
    400.0
}

fn default_idle_timeout_secs() -> u32 {
    0
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
    log::info!("[settings] loading from {}", path.display());

    if !path.exists() {
        log::warn!("[settings] file not found at {}, using defaults", path.display());
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
            let old_version = settings.version;
            settings = migrate_settings(settings);
            if settings.version != old_version {
                log::info!("Settings migrated from v{} to v{}", old_version, settings.version);
                if let Err(e) = save_settings(&settings) {
                    log::warn!("Failed to save migrated settings: {}", e);
                }
            }
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
    use okena_core::theme::ThemeMode;

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
    if let Some(v) = obj.get("color_tinted_background").and_then(|v| v.as_bool()) {
        settings.color_tinted_background = v;
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

    if let Some(v) = obj.get("worktree") {
        if let Ok(wt) = serde_json::from_value::<WorktreeConfig>(v.clone()) {
            settings.worktree = wt;
        }
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

    // v1 -> v2: flat HooksConfig → grouped (project/terminal/worktree).
    // The custom Deserialize impl on HooksConfig handles the actual conversion;
    // this just bumps the version so the grouped format is written on next save.
    if settings.version == 1 {
        log::info!("Migrating settings from v1 to v2 (grouped hooks)");
        settings.version = 2;
    }

    // v2 -> v3: migrate per-extension bool flags to enabled_extensions set.
    if settings.version == 2 {
        log::info!("Migrating settings from v2 to v3 (extension system)");
        if settings.claude_code_integration {
            settings.enabled_extensions.insert("claude-code".to_string());
        }
        if settings.codex_integration {
            settings.enabled_extensions.insert("codex".to_string());
        }
        if settings.auto_update_enabled {
            settings.enabled_extensions.insert("updater".to_string());
        }
        settings.claude_code_integration = false;
        settings.codex_integration = false;
        settings.auto_update_enabled = false;
        settings.version = 3;
    }

    // Ensure version is current
    if settings.version < SETTINGS_VERSION {
        log::warn!(
            "Settings version {} is older than current version {}, some settings may use defaults",
            original_version,
            SETTINGS_VERSION
        );
        settings.version = SETTINGS_VERSION;
    }

    settings
}

/// Process-level mutex for settings file access.
static SETTINGS_LOCK: Mutex<()> = Mutex::new(());

/// Save app settings to disk.
///
/// `remote_connections` are managed separately via `update_remote_connections()`,
/// so this function preserves whatever is on disk rather than overwriting with
/// the (potentially stale) in-memory copy.
pub fn save_settings(settings: &AppSettings) -> Result<()> {
    let _guard = SETTINGS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    save_settings_locked(settings)
}

/// Inner save — caller MUST already hold `SETTINGS_LOCK`.
fn save_settings_locked(settings: &AppSettings) -> Result<()> {
    let path = get_settings_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Preserve remote_connections from disk (they are managed out-of-band
    // by update_remote_connections and not kept in SettingsState's in-memory copy).
    let mut to_save = settings.clone();
    if let Ok(content) = std::fs::read_to_string(&path) {
        if let Ok(on_disk) = serde_json::from_str::<AppSettings>(&content) {
            to_save.remote_connections = on_disk.remote_connections;
        }
    }

    let content = serde_json::to_string_pretty(&to_save)?;

    // Atomic write: write to temp file, set permissions, then rename
    let tmp_path = path.with_extension("json.tmp");
    std::fs::write(&tmp_path, &content)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp_path, std::fs::Permissions::from_mode(0o600));
    }
    std::fs::rename(&tmp_path, &path)?;
    Ok(())
}

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
        let path = get_settings_path();
        let mut settings: AppSettings = std::fs::read_to_string(&path)
            .ok()
            .and_then(|c| serde_json::from_str(&c).ok())
            .unwrap_or_default();
        updater(&mut settings.remote_connections);

        let content = serde_json::to_string_pretty(&settings)?;
        let tmp_path = path.with_extension("json.tmp");
        std::fs::write(&tmp_path, &content)?;
        std::fs::rename(&tmp_path, &path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hooks_config_grouped_round_trip() {
        let config = HooksConfig {
            project: ProjectHooks {
                on_open: Some("echo open".into()),
                on_close: Some("echo close".into()),
            },
            terminal: TerminalHooks {
                on_create: Some("echo create".into()),
                on_close: Some("echo exit".into()),
                shell_wrapper: Some("devcontainer exec -- {shell}".into()),
            },
            worktree: WorktreeHooks {
                on_create: Some("npm install".into()),
                on_close: Some("cleanup".into()),
                pre_merge: Some("lint".into()),
                post_merge: Some("notify".into()),
                before_remove: Some("backup".into()),
                after_remove: Some("log".into()),
                on_rebase_conflict: Some("terminal: claude -p \"fix\"".into()),
                on_dirty_close: Some("echo dirty".into()),
            },
        };
        let json = serde_json::to_string(&config).unwrap();
        let deserialized: HooksConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.project.on_open, Some("echo open".into()));
        assert_eq!(deserialized.terminal.shell_wrapper, Some("devcontainer exec -- {shell}".into()));
        assert_eq!(deserialized.worktree.pre_merge, Some("lint".into()));
        assert_eq!(deserialized.worktree.after_remove, Some("log".into()));
    }

    #[test]
    fn hooks_config_old_flat_format_migrates() {
        let json = r#"{
            "on_project_open": "echo open",
            "on_project_close": "echo close",
            "pre_merge": "lint",
            "worktree_removed": "log",
            "before_worktree_remove": "backup"
        }"#;
        let config: HooksConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.project.on_open, Some("echo open".into()));
        assert_eq!(config.project.on_close, Some("echo close".into()));
        assert_eq!(config.worktree.pre_merge, Some("lint".into()));
        assert_eq!(config.worktree.after_remove, Some("log".into()));
        assert_eq!(config.worktree.before_remove, Some("backup".into()));
        // Terminal hooks not present in old format
        assert!(config.terminal.on_create.is_none());
        assert!(config.terminal.shell_wrapper.is_none());
    }

    #[test]
    fn hooks_config_old_flat_partial() {
        let json = r#"{"on_project_open": "echo open"}"#;
        let config: HooksConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.project.on_open, Some("echo open".into()));
        assert!(config.worktree.pre_merge.is_none());
        assert!(config.worktree.after_remove.is_none());
    }

    #[test]
    fn hooks_config_empty_json_deserializes_to_defaults() {
        let json = "{}";
        let config: HooksConfig = serde_json::from_str(json).unwrap();
        assert!(config.project.on_open.is_none());
        assert!(config.terminal.on_create.is_none());
        assert!(config.worktree.pre_merge.is_none());
    }

    #[test]
    fn hooks_config_grouped_serializes_cleanly() {
        // Empty config should serialize to just {}
        let config = HooksConfig::default();
        let json = serde_json::to_string(&config).unwrap();
        assert_eq!(json, "{}");
    }

    #[test]
    fn migrate_v2_claude_code_integration_to_enabled_extensions() {
        let json = r#"{"version": 2, "claude_code_integration": true}"#;
        let settings: AppSettings = serde_json::from_str(json).unwrap();
        let migrated = migrate_settings(settings);
        assert_eq!(migrated.version, SETTINGS_VERSION);
        assert!(migrated.enabled_extensions.contains("claude-code"));
        assert!(!migrated.enabled_extensions.contains("codex"));
    }

    #[test]
    fn migrate_v2_codex_integration_to_enabled_extensions() {
        let json = r#"{"version": 2, "codex_integration": true}"#;
        let settings: AppSettings = serde_json::from_str(json).unwrap();
        let migrated = migrate_settings(settings);
        assert!(migrated.enabled_extensions.contains("codex"));
    }

    #[test]
    fn migrate_v2_both_integrations() {
        let json = r#"{"version": 2, "claude_code_integration": true, "codex_integration": true}"#;
        let settings: AppSettings = serde_json::from_str(json).unwrap();
        let migrated = migrate_settings(settings);
        assert!(migrated.enabled_extensions.contains("claude-code"));
        assert!(migrated.enabled_extensions.contains("codex"));
    }

    #[test]
    fn migrate_v2_no_integrations_only_updater() {
        // auto_update_enabled defaults to true, so updater is migrated
        let json = r#"{"version": 2}"#;
        let settings: AppSettings = serde_json::from_str(json).unwrap();
        let migrated = migrate_settings(settings);
        assert_eq!(migrated.enabled_extensions.len(), 1);
        assert!(migrated.enabled_extensions.contains("updater"));
        assert!(!migrated.enabled_extensions.contains("claude-code"));
    }

    #[test]
    fn migrate_v2_auto_update_disabled() {
        let json = r#"{"version": 2, "auto_update_enabled": false}"#;
        let settings: AppSettings = serde_json::from_str(json).unwrap();
        let migrated = migrate_settings(settings);
        assert!(!migrated.enabled_extensions.contains("updater"));
    }

    #[test]
    fn enabled_extensions_not_serialized_with_legacy_fields() {
        let mut settings = AppSettings::default();
        settings.enabled_extensions.insert("claude-code".to_string());
        let json = serde_json::to_string_pretty(&settings).unwrap();
        // Legacy bool fields should not appear in serialized output
        assert!(!json.contains("claude_code_integration"));
        assert!(!json.contains("codex_integration"));
        // enabled_extensions should be present
        assert!(json.contains("enabled_extensions"));
        assert!(json.contains("claude-code"));
    }
}
