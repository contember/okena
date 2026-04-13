//! Persistent workspace data — projects, folders, layouts.

use crate::hooks_config::HooksConfig;
use okena_core::theme::FolderColor;
use okena_layout::LayoutNode;
use okena_terminal::shell_config::ShellType;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// A folder that groups projects in the sidebar
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FolderData {
    pub id: String,
    pub name: String,
    /// Ordered project IDs inside this folder
    pub project_ids: Vec<String>,
    #[serde(default)]
    pub collapsed: bool,
    #[serde(default)]
    pub folder_color: FolderColor,
}

/// The main workspace data structure (serializable)
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkspaceData {
    /// Schema version for migration support
    #[serde(default = "default_workspace_version")]
    pub version: u32,
    pub projects: Vec<ProjectData>,
    pub project_order: Vec<String>,
    /// Project column widths as percentages (project_id -> width %)
    #[serde(default)]
    pub project_widths: HashMap<String, f32>,
    /// Folders for grouping projects
    #[serde(default)]
    pub folders: Vec<FolderData>,
    /// Service panel heights in pixels (project_id -> height)
    #[serde(default)]
    pub service_panel_heights: HashMap<String, f32>,
    /// Hook panel heights in pixels (project_id -> height)
    #[serde(default)]
    pub hook_panel_heights: HashMap<String, f32>,
}

impl WorkspaceData {
    /// Return a copy with all remote projects, remote folders, and their
    /// associated widths/heights stripped out (for saving to disk).
    pub fn without_remote_projects(&self) -> Self {
        let remote_ids: HashSet<&str> = self.projects.iter()
            .filter(|p| p.is_remote)
            .map(|p| p.id.as_str())
            .collect();

        if remote_ids.is_empty() {
            return self.clone();
        }

        Self {
            version: self.version,
            projects: self.projects.iter().filter(|p| !p.is_remote).cloned().collect(),
            project_order: self.project_order.iter()
                .filter(|id| !id.starts_with("remote:") && !remote_ids.contains(id.as_str()))
                .cloned().collect(),
            project_widths: self.project_widths.iter()
                .filter(|(id, _)| !remote_ids.contains(id.as_str()))
                .map(|(k, v)| (k.clone(), *v)).collect(),
            service_panel_heights: self.service_panel_heights.iter()
                .filter(|(id, _)| !remote_ids.contains(id.as_str()))
                .map(|(k, v)| (k.clone(), *v)).collect(),
            hook_panel_heights: self.hook_panel_heights.iter()
                .filter(|(id, _)| !remote_ids.contains(id.as_str()))
                .map(|(k, v)| (k.clone(), *v)).collect(),
            folders: self.folders.iter()
                .filter(|f| !f.id.starts_with("remote:"))
                .cloned().collect(),
        }
    }
}

/// Metadata for worktree projects.
///
/// Only `parent_project_id` is actively used. The other fields are kept for
/// backward-compatible deserialization of old workspace.json files but are no
/// longer written on save. All derived data (main repo path, branch, worktree
/// path) is resolved dynamically from the parent project and git at runtime.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorktreeMetadata {
    /// ID of the main repo project
    pub parent_project_id: String,
    /// Optional color override for this worktree (when None, inherits parent's color)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_override: Option<FolderColor>,
    /// Deprecated: resolved dynamically from parent project path.
    #[serde(default, skip_serializing)]
    #[allow(dead_code)]
    pub main_repo_path: String,
    /// Deprecated: same as project.path.
    #[serde(default, skip_serializing)]
    #[allow(dead_code)]
    pub worktree_path: String,
    /// Deprecated: read from git at runtime.
    #[serde(default, skip_serializing)]
    #[allow(dead_code)]
    pub branch_name: String,
}

/// Status of a hook terminal in the service panel.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum HookTerminalStatus {
    Running,
    Succeeded,
    Failed { exit_code: i32 },
}

/// Entry for a hook terminal displayed in the service panel.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HookTerminalEntry {
    pub label: String,
    pub status: HookTerminalStatus,
    /// Which hook triggered this terminal (e.g. "on_project_open").
    pub hook_type: String,
    /// The full command string with env vars baked in (ready to re-execute).
    pub command: String,
    /// Working directory for the hook command.
    pub cwd: String,
}

/// A single project with its layout tree
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProjectData {
    pub id: String,
    pub name: String,
    pub path: String,
    #[serde(default = "default_true", alias = "is_visible")]
    pub show_in_overview: bool,
    /// Layout tree for terminal panes. None means project is a bookmark without terminals.
    pub layout: Option<LayoutNode>,
    #[serde(default)]
    pub terminal_names: HashMap<String, String>,
    #[serde(default)]
    pub hidden_terminals: HashMap<String, bool>,
    /// Optional worktree metadata (only set for worktree projects)
    #[serde(default)]
    pub worktree_info: Option<WorktreeMetadata>,
    /// Ordered list of worktree child project IDs (for parent projects)
    #[serde(default)]
    pub worktree_ids: Vec<String>,
    /// Folder icon color for this project
    #[serde(default)]
    pub folder_color: FolderColor,
    /// Per-project lifecycle hooks (overrides global settings)
    #[serde(default)]
    pub hooks: HooksConfig,
    /// Whether this is a remote project (materialized from a remote connection)
    #[serde(default)]
    pub is_remote: bool,
    /// Connection ID for remote projects (links to RemoteConnectionManager)
    #[serde(default)]
    pub connection_id: Option<String>,
    /// Saved terminal IDs for services (service_name -> terminal_id)
    /// Used to reconnect to persistent sessions across restarts
    #[serde(default)]
    pub service_terminals: HashMap<String, String>,
    /// Per-project default shell (overrides global default when ShellType::Default is used)
    #[serde(default)]
    pub default_shell: Option<ShellType>,
    /// Hook terminals displayed in the service panel (persisted across restarts)
    #[serde(default)]
    pub hook_terminals: HashMap<String, HookTerminalEntry>,
}

impl ProjectData {
    /// Get the display name for a terminal.
    /// Priority: user-set custom name > non-prompt OSC title > directory-based fallback.
    /// OSC titles matching bash prompt format (user@host:...) are ignored in favor
    /// of the directory name. Explicit titles (e.g. from printf) are shown.
    pub fn terminal_display_name(&self, terminal_id: &str, osc_title: Option<String>) -> String {
        if let Some(custom_name) = self.terminal_names.get(terminal_id) {
            return custom_name.clone();
        }
        if let Some(ref title) = osc_title {
            if !is_bash_prompt_title(title) {
                return title.clone();
            }
        }
        self.directory_name()
    }

    /// Get the directory name from the project path (used as terminal name fallback).
    pub fn directory_name(&self) -> String {
        std::path::Path::new(&self.path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("Terminal")
            .to_string()
    }
}

/// Check if an OSC title looks like a bash/zsh prompt title (e.g. "user@host: ~/path").
/// These are auto-set by the shell and should not override the directory-based name.
pub fn is_bash_prompt_title(title: &str) -> bool {
    // Match pattern: non-whitespace@non-whitespace:
    let bytes = title.as_bytes();
    let mut i = 0;
    while i < bytes.len() && bytes[i] != b'@' && !bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    if i == 0 || i >= bytes.len() || bytes[i] != b'@' {
        return false;
    }
    i += 1;
    while i < bytes.len() && bytes[i] != b':' && !bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    i > 1 && i < bytes.len() && bytes[i] == b':'
}

fn default_workspace_version() -> u32 {
    0 // pre-versioning workspace files
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_project(path: &str) -> ProjectData {
        ProjectData {
            id: "test-id".to_string(),
            name: "test".to_string(),
            path: path.to_string(),
            show_in_overview: true,
            layout: None,
            terminal_names: HashMap::new(),
            hidden_terminals: HashMap::new(),
            worktree_info: None,
            worktree_ids: Vec::new(),
            folder_color: Default::default(),
            hooks: Default::default(),
            is_remote: false,
            connection_id: None,
            service_terminals: HashMap::new(),
            default_shell: None,
            hook_terminals: HashMap::new(),
        }
    }

    #[test]
    fn directory_name_from_path() {
        assert_eq!(make_project("/home/user/myproject").directory_name(), "myproject");
        assert_eq!(make_project("/").directory_name(), "Terminal");
    }

    #[test]
    fn terminal_display_name_prefers_custom_name() {
        let mut project = make_project("/home/user/myproject");
        project.terminal_names.insert("t1".to_string(), "My Terminal".to_string());
        assert_eq!(
            project.terminal_display_name("t1", Some("osc-title".to_string())),
            "My Terminal"
        );
    }

    #[test]
    fn terminal_display_name_uses_osc_title_when_no_custom() {
        let project = make_project("/home/user/myproject");
        assert_eq!(
            project.terminal_display_name("t1", Some("osc-title".to_string())),
            "osc-title"
        );
    }

    #[test]
    fn terminal_display_name_falls_back_to_directory() {
        let project = make_project("/home/user/myproject");
        assert_eq!(
            project.terminal_display_name("t1", None),
            "myproject"
        );
    }

    #[test]
    fn terminal_display_name_ignores_bash_prompt_title() {
        let project = make_project("/home/user/myproject");
        assert_eq!(
            project.terminal_display_name("t1", Some("matej21@matej21-hp: ~/projects/myproject".to_string())),
            "myproject"
        );
        assert_eq!(
            project.terminal_display_name("t1", Some("root@server:/var/log".to_string())),
            "myproject"
        );
    }

    #[test]
    fn terminal_display_name_shows_explicit_osc_title() {
        let project = make_project("/home/user/myproject");
        assert_eq!(
            project.terminal_display_name("t1", Some("MOJE_JMENO".to_string())),
            "MOJE_JMENO"
        );
        assert_eq!(
            project.terminal_display_name("t1", Some("my-app dev server".to_string())),
            "my-app dev server"
        );
    }

    #[test]
    fn is_bash_prompt_title_detection() {
        assert!(is_bash_prompt_title("matej21@matej21-hp: ~/projects"));
        assert!(is_bash_prompt_title("root@server:/var/log"));
        assert!(is_bash_prompt_title("user@host:~"));
        assert!(!is_bash_prompt_title("MOJE_JMENO"));
        assert!(!is_bash_prompt_title("my-app dev server"));
        assert!(!is_bash_prompt_title("Terminal 1"));
        assert!(!is_bash_prompt_title(""));
    }

    #[test]
    fn project_data_with_legacy_hooks_migrates_on_load() {
        // Minimal workspace.json shape from a pre-grouped install — the
        // `hooks` block uses the old flat key names and must migrate
        // transparently when ProjectData is deserialized.
        let json = r#"{
            "id": "p1",
            "name": "Test",
            "path": "/tmp/test",
            "layout": null,
            "hooks": {
                "on_project_open": "init.sh",
                "pre_merge": "check.sh",
                "worktree_removed": "cleanup.sh"
            }
        }"#;

        let project: ProjectData = serde_json::from_str(json).unwrap();

        assert_eq!(project.id, "p1");
        assert!(project.layout.is_none());
        // Legacy hooks should be mapped to the new grouped layout.
        assert_eq!(project.hooks.project.on_open.as_deref(), Some("init.sh"));
        assert_eq!(project.hooks.worktree.pre_merge.as_deref(), Some("check.sh"));
        assert_eq!(project.hooks.worktree.after_remove.as_deref(), Some("cleanup.sh"));
        // Untouched fields remain default.
        assert!(project.hooks.project.on_close.is_none());
        assert!(project.hooks.worktree.on_create.is_none());
    }

    #[test]
    fn project_data_legacy_hooks_save_roundtrip_uses_grouped_format() {
        // Load legacy → save → reload. The saved JSON must be in the new
        // grouped format and the reload must preserve the migrated values.
        let legacy_json = r#"{
            "id": "p1",
            "name": "Test",
            "path": "/tmp/test",
            "layout": null,
            "hooks": { "on_project_open": "init.sh" }
        }"#;

        let project: ProjectData = serde_json::from_str(legacy_json).unwrap();
        let saved = serde_json::to_string(&project).unwrap();

        // After saving the migrated config, no legacy keys should remain.
        assert!(!saved.contains("\"on_project_open\""), "legacy key must not survive a save");
        // The grouped key should be present.
        assert!(saved.contains("\"project\""), "expected grouped project key");

        let reloaded: ProjectData = serde_json::from_str(&saved).unwrap();
        assert_eq!(reloaded.hooks.project.on_open.as_deref(), Some("init.sh"));
    }
}
