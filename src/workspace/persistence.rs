use crate::terminal::session_backend::SessionBackend;
use crate::theme::ThemeMode;
use crate::workspace::state::{LayoutNode, ProjectData, WorkspaceData};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Sidebar settings
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SidebarSettings {
    /// Whether the sidebar is open
    #[serde(default)]
    pub is_open: bool,
    /// Whether auto-hide mode is enabled
    #[serde(default)]
    pub auto_hide: bool,
}

impl Default for SidebarSettings {
    fn default() -> Self {
        Self {
            is_open: false,
            auto_hide: false,
        }
    }
}

/// App settings (persisted separately from workspace)
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AppSettings {
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

    // Terminal settings
    /// Enable cursor blinking (default: false)
    #[serde(default = "default_cursor_blink")]
    pub cursor_blink: bool,
    /// Number of scrollback lines (default: 10000)
    #[serde(default = "default_scrollback_lines")]
    pub scrollback_lines: u32,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            theme_mode: ThemeMode::default(),
            active_session: None,
            sidebar: SidebarSettings::default(),
            show_focused_border: default_show_focused_border(),
            font_size: default_font_size(),
            font_family: default_font_family(),
            line_height: default_line_height(),
            ui_font_size: default_ui_font_size(),
            cursor_blink: default_cursor_blink(),
            scrollback_lines: default_scrollback_lines(),
        }
    }
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

fn default_cursor_blink() -> bool {
    false
}

fn default_scrollback_lines() -> u32 {
    10000
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

/// Load app settings from disk
pub fn load_settings() -> AppSettings {
    let path = get_settings_path();
    if path.exists() {
        if let Ok(content) = std::fs::read_to_string(&path) {
            if let Ok(settings) = serde_json::from_str(&content) {
                return settings;
            }
        }
    }
    AppSettings::default()
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
pub fn load_workspace() -> Result<WorkspaceData> {
    let path = get_workspace_path();

    if path.exists() {
        let content = std::fs::read_to_string(&path)?;
        let mut data: WorkspaceData = serde_json::from_str(&content)?;

        // Only clear terminal IDs if session persistence is not enabled
        // With tmux/screen backend, sessions survive app restarts
        let session_backend = SessionBackend::from_env().resolve();
        if !session_backend.supports_persistence() {
            for project in &mut data.projects {
                project.layout.clear_terminal_ids();
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
            layout: LayoutNode::new_terminal(),
            terminal_names: HashMap::new(),
            hidden_terminals: HashMap::new(),
            worktree_info: None,
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
pub fn load_session(name: &str) -> Result<WorkspaceData> {
    let path = get_session_path(name);

    if !path.exists() {
        anyhow::bail!("Session '{}' not found", name);
    }

    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read session file: {}", path.display()))?;
    let mut data: WorkspaceData = serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse session file: {}", path.display()))?;

    // Only clear terminal IDs if session persistence is not enabled
    let session_backend = SessionBackend::from_env().resolve();
    if !session_backend.supports_persistence() {
        for project in &mut data.projects {
            project.layout.clear_terminal_ids();
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
            project.layout.clear_terminal_ids();
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
        project.layout.clear_terminal_ids();
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
