use crate::terminal::session_backend::SessionBackend;
use crate::terminal::shell_config::ShellType;
use crate::theme::{FolderColor, ThemeMode};
use crate::views::overlays::DiffViewMode;
use crate::workspace::state::{LayoutNode, ProjectData, WorkspaceData};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

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
            cursor_blink: default_cursor_blink(),
            scrollback_lines: default_scrollback_lines(),
            default_shell: ShellType::default(),
            show_shell_selector: false,
            session_backend: SessionBackend::default(),
            file_opener: default_file_opener(),
            hooks: HooksConfig::default(),
            diff_view_mode: DiffViewMode::default(),
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

/// Metadata about a saved session
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionInfo {
    pub name: String,
    pub created_at: String,
    pub modified_at: String,
    pub project_count: usize,
}

/// Wrapper for exported workspace (includes metadata for import validation)
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExportedWorkspace {
    pub version: u32,
    pub exported_at: String,
    pub workspace: WorkspaceData,
}

/// Get the config directory path
fn get_config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("term-manager-rs")
}

/// Get the workspace file path
pub fn get_workspace_path() -> PathBuf {
    get_config_dir().join("workspace.json")
}

/// Get the settings file path
pub fn get_settings_path() -> PathBuf {
    get_config_dir().join("settings.json")
}

/// Get the sessions directory path
fn get_sessions_dir() -> PathBuf {
    get_config_dir().join("sessions")
}

/// Get path for a named session
fn get_session_path(name: &str) -> PathBuf {
    get_sessions_dir().join(format!("{}.json", sanitize_session_name(name)))
}

/// Sanitize session name for use as filename
fn sanitize_session_name(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect()
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

    if let Some(v) = obj.get("cursor_blink").and_then(|v| v.as_bool()) {
        settings.cursor_blink = v;
    }

    if let Some(v) = obj.get("scrollback_lines").and_then(|v| v.as_u64()) {
        settings.scrollback_lines = (v as u32).clamp(100, 100000);
    }

    if let Some(v) = obj.get("file_opener").and_then(|v| v.as_str()) {
        settings.file_opener = v.to_string();
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
    Ok(())
}

/// Load workspace from disk
pub fn load_workspace(backend: SessionBackend) -> Result<WorkspaceData> {
    let path = get_workspace_path();

    if path.exists() {
        let content = std::fs::read_to_string(&path)?;
        let mut data: WorkspaceData = serde_json::from_str(&content)?;

        // Only clear terminal IDs if session persistence is not enabled
        // With tmux/screen backend, sessions survive app restarts
        let session_backend = backend.resolve();
        if !session_backend.supports_persistence() {
            for project in &mut data.projects {
                if let Some(ref mut layout) = project.layout {
                    layout.clear_terminal_ids();
                }
            }
        }

        // Ensure project_order contains all project IDs
        for project in &data.projects {
            if !data.project_order.contains(&project.id) {
                data.project_order.push(project.id.clone());
            }
        }

        Ok(data)
    } else {
        Ok(default_workspace())
    }
}

/// Save workspace to disk
pub fn save_workspace(data: &WorkspaceData) -> Result<()> {
    let path = get_workspace_path();

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let content = serde_json::to_string_pretty(data)?;
    std::fs::write(&path, content)?;

    Ok(())
}

/// Create a default workspace with one project
pub fn default_workspace() -> WorkspaceData {
    let project_id = uuid::Uuid::new_v4().to_string();
    let home_dir = dirs::home_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| "/".to_string());

    WorkspaceData {
        projects: vec![ProjectData {
            id: project_id.clone(),
            name: "Default".to_string(),
            path: home_dir,
            is_visible: true,
            layout: Some(LayoutNode::new_terminal()),
            terminal_names: HashMap::new(),
            hidden_terminals: HashMap::new(),
            worktree_info: None,
            folder_color: FolderColor::default(),
            hooks: HooksConfig::default(),
        }],
        project_order: vec![project_id],
        project_widths: HashMap::new(),
    }
}

// =============================================================================
// Session Management (Multiple Named Workspaces)
// =============================================================================

/// List all saved sessions
pub fn list_sessions() -> Result<Vec<SessionInfo>> {
    let sessions_dir = get_sessions_dir();

    if !sessions_dir.exists() {
        return Ok(Vec::new());
    }

    let mut sessions = Vec::new();

    for entry in std::fs::read_dir(&sessions_dir)? {
        let entry = entry?;
        let path = entry.path();

        if path.extension().map_or(false, |ext| ext == "json") {
            if let Some(name) = path.file_stem().and_then(|s| s.to_str()) {
                // Read file metadata for timestamps
                let metadata = std::fs::metadata(&path)?;
                let modified = metadata.modified().ok();
                let created = metadata.created().ok();

                // Try to read workspace to get project count
                let project_count = if let Ok(content) = std::fs::read_to_string(&path) {
                    if let Ok(data) = serde_json::from_str::<WorkspaceData>(&content) {
                        data.projects.len()
                    } else {
                        0
                    }
                } else {
                    0
                };

                sessions.push(SessionInfo {
                    name: name.to_string(),
                    created_at: created
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| format_timestamp(d.as_secs()))
                        .unwrap_or_else(|| "Unknown".to_string()),
                    modified_at: modified
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| format_timestamp(d.as_secs()))
                        .unwrap_or_else(|| "Unknown".to_string()),
                    project_count,
                });
            }
        }
    }

    // Sort by modification time (most recent first)
    sessions.sort_by(|a, b| b.modified_at.cmp(&a.modified_at));

    Ok(sessions)
}

/// Save current workspace as a named session
pub fn save_session(name: &str, data: &WorkspaceData) -> Result<()> {
    let sessions_dir = get_sessions_dir();
    std::fs::create_dir_all(&sessions_dir)?;

    let path = get_session_path(name);
    let content = serde_json::to_string_pretty(data)?;
    std::fs::write(&path, content)?;

    Ok(())
}

/// Load a named session
pub fn load_session(name: &str, backend: SessionBackend) -> Result<WorkspaceData> {
    let path = get_session_path(name);

    if !path.exists() {
        anyhow::bail!("Session '{}' not found", name);
    }

    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read session file: {}", path.display()))?;
    let mut data: WorkspaceData = serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse session file: {}", path.display()))?;

    // Only clear terminal IDs if session persistence is not enabled
    let session_backend = backend.resolve();
    if !session_backend.supports_persistence() {
        for project in &mut data.projects {
            if let Some(ref mut layout) = project.layout {
                layout.clear_terminal_ids();
            }
        }
    }

    // Ensure project_order contains all project IDs
    for project in &data.projects {
        if !data.project_order.contains(&project.id) {
            data.project_order.push(project.id.clone());
        }
    }

    Ok(data)
}

/// Delete a named session
pub fn delete_session(name: &str) -> Result<()> {
    let path = get_session_path(name);

    if !path.exists() {
        anyhow::bail!("Session '{}' not found", name);
    }

    std::fs::remove_file(&path)?;
    Ok(())
}

/// Rename a session
pub fn rename_session(old_name: &str, new_name: &str) -> Result<()> {
    let old_path = get_session_path(old_name);
    let new_path = get_session_path(new_name);

    if !old_path.exists() {
        anyhow::bail!("Session '{}' not found", old_name);
    }

    if new_path.exists() {
        anyhow::bail!("Session '{}' already exists", new_name);
    }

    std::fs::rename(&old_path, &new_path)?;
    Ok(())
}

/// Check if a session exists
pub fn session_exists(name: &str) -> bool {
    get_session_path(name).exists()
}

// =============================================================================
// Export/Import Functionality
// =============================================================================

/// Export workspace to a file
pub fn export_workspace(data: &WorkspaceData, path: &std::path::Path) -> Result<()> {
    let exported = ExportedWorkspace {
        version: 1,
        exported_at: current_timestamp(),
        workspace: data.clone(),
    };

    let content = serde_json::to_string_pretty(&exported)?;
    std::fs::write(path, content)?;

    Ok(())
}

/// Import workspace from a file
pub fn import_workspace(path: &std::path::Path) -> Result<WorkspaceData> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read file: {}", path.display()))?;

    // Try to parse as ExportedWorkspace first (has version/metadata)
    if let Ok(exported) = serde_json::from_str::<ExportedWorkspace>(&content) {
        let mut data = exported.workspace;

        // Clear terminal IDs
        for project in &mut data.projects {
            if let Some(ref mut layout) = project.layout {
                layout.clear_terminal_ids();
            }
        }

        // Ensure project_order contains all project IDs
        for project in &data.projects {
            if !data.project_order.contains(&project.id) {
                data.project_order.push(project.id.clone());
            }
        }

        return Ok(data);
    }

    // Fall back to parsing as raw WorkspaceData (for backwards compatibility)
    let mut data: WorkspaceData = serde_json::from_str(&content)
        .with_context(|| "Failed to parse workspace file")?;

    // Clear terminal IDs
    for project in &mut data.projects {
        if let Some(ref mut layout) = project.layout {
            layout.clear_terminal_ids();
        }
    }

    // Ensure project_order contains all project IDs
    for project in &data.projects {
        if !data.project_order.contains(&project.id) {
            data.project_order.push(project.id.clone());
        }
    }

    Ok(data)
}

/// Get the config directory path (public for UI display)
pub fn config_dir() -> PathBuf {
    get_config_dir()
}

// =============================================================================
// Helper Functions
// =============================================================================

/// Format Unix timestamp as ISO 8601 string
fn format_timestamp(secs: u64) -> String {
    // Simple ISO 8601 format without external crate
    let days_since_epoch = secs / 86400;
    let remaining_secs = secs % 86400;
    let hours = remaining_secs / 3600;
    let minutes = (remaining_secs % 3600) / 60;
    let seconds = remaining_secs % 60;

    // Calculate year, month, day from days since epoch (1970-01-01)
    let (year, month, day) = days_to_ymd(days_since_epoch);

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year, month, day, hours, minutes, seconds
    )
}

/// Get current timestamp as ISO 8601 string
fn current_timestamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format_timestamp(secs)
}

/// Convert days since Unix epoch to (year, month, day)
fn days_to_ymd(days: u64) -> (u64, u64, u64) {
    // Simplified calculation - not accounting for leap seconds
    let mut remaining_days = days as i64;
    let mut year = 1970;

    // Find the year
    loop {
        let days_in_year = if is_leap_year(year) { 366 } else { 365 };
        if remaining_days < days_in_year {
            break;
        }
        remaining_days -= days_in_year;
        year += 1;
    }

    // Find the month and day
    let days_in_months = if is_leap_year(year) {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };

    let mut month = 1;
    for &days_in_month in &days_in_months {
        if remaining_days < days_in_month {
            break;
        }
        remaining_days -= days_in_month;
        month += 1;
    }

    (year as u64, month, (remaining_days + 1) as u64)
}

/// Check if a year is a leap year
fn is_leap_year(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}
