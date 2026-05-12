//! Persistent workspace data — projects, folders, layouts.

use crate::hooks_config::HooksConfig;
use crate::window_state::WindowState;
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
    /// Folders for grouping projects
    #[serde(default)]
    pub folders: Vec<FolderData>,
    /// Service panel heights in pixels (project_id -> height)
    #[serde(default)]
    pub service_panel_heights: HashMap<String, f32>,
    /// Hook panel heights in pixels (project_id -> height)
    #[serde(default)]
    pub hook_panel_heights: HashMap<String, f32>,
    /// Filter/UI state for the main window. Always present — schema invariant
    /// is that closing main quits the app, so a default `WindowState` is
    /// produced on missing/corrupt input.
    #[serde(default)]
    pub main_window: WindowState,
    /// Filter/UI state for any extra windows open at save time. Empty in the
    /// single-window case.
    #[serde(default)]
    pub extra_windows: Vec<WindowState>,
}

impl WorkspaceData {
    /// Return a copy with all remote projects, remote folders, and their
    /// associated widths/heights stripped out (for saving to disk).
    pub fn without_remote_projects(&self) -> Self {
        let remote_ids: HashSet<String> = self.projects.iter()
            .filter(|p| p.is_remote)
            .map(|p| p.id.clone())
            .collect();
        let remote_folder_ids: HashSet<String> = self.folders.iter()
            .filter(|f| f.id.starts_with("remote:"))
            .map(|f| f.id.clone())
            .collect();

        if remote_ids.is_empty() && remote_folder_ids.is_empty() {
            return self.clone();
        }

        let mut data = Self {
            version: self.version,
            projects: self.projects.iter().filter(|p| !p.is_remote).cloned().collect(),
            project_order: self.project_order.iter()
                .filter(|id| !id.starts_with("remote:") && !remote_ids.contains(*id))
                .cloned().collect(),
            service_panel_heights: self.service_panel_heights.iter()
                .filter(|(id, _)| !remote_ids.contains(*id))
                .map(|(k, v)| (k.clone(), *v)).collect(),
            hook_panel_heights: self.hook_panel_heights.iter()
                .filter(|(id, _)| !remote_ids.contains(*id))
                .map(|(k, v)| (k.clone(), *v)).collect(),
            folders: self.folders.iter()
                .filter(|f| !f.id.starts_with("remote:"))
                .cloned().collect(),
            main_window: self.main_window.clone(),
            extra_windows: self.extra_windows.clone(),
        };

        for project_id in &remote_ids {
            data.delete_project_scrub_all_windows(project_id);
        }
        for folder_id in &remote_folder_ids {
            data.delete_folder_scrub_all_windows(folder_id);
        }

        data
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::window_id::WindowId;
    use crate::window_state::WindowBounds;

    fn make_project(path: &str) -> ProjectData {
        ProjectData {
            id: "test-id".to_string(),
            name: "test".to_string(),
            path: path.to_string(),
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

    fn make_workspace() -> WorkspaceData {
        WorkspaceData {
            version: 1,
            projects: Vec::new(),
            project_order: Vec::new(),
            folders: Vec::new(),
            service_panel_heights: HashMap::new(),
            hook_panel_heights: HashMap::new(),
            main_window: WindowState::default(),
            extra_windows: Vec::new(),
        }
    }

    #[test]
    fn workspace_data_old_shape_loads_with_default_main_window() {
        // Pre-multi-window workspace.json shape — no main_window or
        // extra_windows fields. Schema invariant: load must always produce a
        // default main_window and an empty extras vec.
        let legacy_json = r#"{
            "version": 1,
            "projects": [],
            "project_order": []
        }"#;

        let data: WorkspaceData = serde_json::from_str(legacy_json).unwrap();

        assert!(data.main_window.hidden_project_ids.is_empty());
        assert!(data.main_window.folder_filter.is_none());
        assert!(data.main_window.project_widths.is_empty());
        assert!(data.main_window.folder_collapsed.is_empty());
        assert!(data.main_window.os_bounds.is_none());
        assert!(data.extra_windows.is_empty());
    }

    #[test]
    fn workspace_data_roundtrips_window_state() {
        let mut data = make_workspace();
        data.main_window.hidden_project_ids.insert("p1".to_string());
        data.main_window.folder_filter = Some("f1".to_string());
        data.extra_windows.push(WindowState::default());

        let json = serde_json::to_string(&data).unwrap();
        let reloaded: WorkspaceData = serde_json::from_str(&json).unwrap();

        assert_eq!(reloaded.main_window.hidden_project_ids, data.main_window.hidden_project_ids);
        assert_eq!(reloaded.main_window.folder_filter, data.main_window.folder_filter);
        assert_eq!(reloaded.extra_windows.len(), 1);
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

    #[test]
    fn project_data_has_no_show_in_overview_field() {
        // Per-window visibility lives exclusively on
        // main_window.hidden_project_ids. The legacy ProjectData.show_in_overview
        // field has been removed from the struct entirely (not just tombstoned
        // for save) -- serialization must not produce a "show_in_overview" key.
        let project = make_project("/tmp/test");
        let saved = serde_json::to_string(&project).unwrap();
        let value: serde_json::Value = serde_json::from_str(&saved).unwrap();
        assert!(!value.as_object().unwrap().contains_key("show_in_overview"),
            "ProjectData.show_in_overview must not appear in serialized form (field removed)");
    }

    #[test]
    fn without_remote_projects_scrubs_remote_window_state() {
        let mut data = make_workspace();
        let mut local = make_project("/tmp/local");
        local.id = "local".to_string();
        let mut remote = make_project("/tmp/remote");
        remote.id = "remote:c1:p1".to_string();
        remote.is_remote = true;
        remote.connection_id = Some("c1".to_string());

        data.projects = vec![local, remote];
        data.project_order = vec![
            "local".to_string(),
            "remote:c1:p1".to_string(),
            "remote:c1:f1".to_string(),
        ];
        data.folders = vec![
            FolderData {
                id: "local-folder".to_string(),
                name: "Local".to_string(),
                project_ids: vec!["local".to_string()],
                folder_color: Default::default(),
            },
            FolderData {
                id: "remote:c1:f1".to_string(),
                name: "Remote".to_string(),
                project_ids: vec!["remote:c1:p1".to_string()],
                folder_color: Default::default(),
            },
        ];
        data.service_panel_heights.insert("local".to_string(), 1.0);
        data.service_panel_heights.insert("remote:c1:p1".to_string(), 2.0);
        data.hook_panel_heights.insert("local".to_string(), 3.0);
        data.hook_panel_heights.insert("remote:c1:p1".to_string(), 4.0);

        data.main_window.hidden_project_ids.insert("local".to_string());
        data.main_window.hidden_project_ids.insert("remote:c1:p1".to_string());
        data.main_window.project_widths.insert("local".to_string(), 0.25);
        data.main_window.project_widths.insert("remote:c1:p1".to_string(), 0.75);
        data.main_window.folder_filter = Some("remote:c1:f1".to_string());
        data.main_window.folder_collapsed.insert("local-folder".to_string(), true);
        data.main_window.folder_collapsed.insert("remote:c1:f1".to_string(), true);

        let mut extra = WindowState::default();
        let extra_id = extra.id;
        extra.hidden_project_ids.insert("remote:c1:p1".to_string());
        extra.project_widths.insert("remote:c1:p1".to_string(), 0.50);
        extra.folder_filter = Some("remote:c1:f1".to_string());
        extra.folder_collapsed.insert("remote:c1:f1".to_string(), true);
        data.extra_windows.push(extra);

        let saved = data.without_remote_projects();

        assert_eq!(saved.projects.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(), vec!["local"]);
        assert_eq!(saved.project_order, vec!["local"]);
        assert_eq!(saved.folders.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(), vec!["local-folder"]);
        assert_eq!(saved.service_panel_heights.get("local").copied(), Some(1.0));
        assert!(!saved.service_panel_heights.contains_key("remote:c1:p1"));
        assert_eq!(saved.hook_panel_heights.get("local").copied(), Some(3.0));
        assert!(!saved.hook_panel_heights.contains_key("remote:c1:p1"));

        assert!(saved.main_window.hidden_project_ids.contains("local"));
        assert!(!saved.main_window.hidden_project_ids.contains("remote:c1:p1"));
        assert_eq!(saved.main_window.project_widths.get("local").copied(), Some(0.25));
        assert!(!saved.main_window.project_widths.contains_key("remote:c1:p1"));
        assert!(saved.main_window.folder_filter.is_none());
        assert_eq!(saved.main_window.folder_collapsed.get("local-folder").copied(), Some(true));
        assert!(!saved.main_window.folder_collapsed.contains_key("remote:c1:f1"));

        let saved_extra = saved.window(WindowId::Extra(extra_id)).unwrap();
        assert!(!saved_extra.hidden_project_ids.contains("remote:c1:p1"));
        assert!(!saved_extra.project_widths.contains_key("remote:c1:p1"));
        assert!(saved_extra.folder_filter.is_none());
        assert!(!saved_extra.folder_collapsed.contains_key("remote:c1:f1"));
    }

    #[test]
    fn folder_data_has_no_collapsed_field() {
        // Per-window sidebar collapse state lives exclusively on
        // main_window.folder_collapsed. The legacy FolderData.collapsed
        // field has been removed from the struct entirely (not just
        // tombstoned for save) -- serialization must not produce a
        // "collapsed" key.
        let folder = FolderData {
            id: "f1".to_string(),
            name: "F".to_string(),
            project_ids: Vec::new(),
            folder_color: Default::default(),
        };
        let saved = serde_json::to_string(&folder).unwrap();
        let value: serde_json::Value = serde_json::from_str(&saved).unwrap();
        assert!(!value.as_object().unwrap().contains_key("collapsed"),
            "FolderData.collapsed must not appear in serialized form (field removed)");
    }

    #[test]
    fn window_lookup_main_is_infallible() {
        // WindowId::Main always resolves to &main_window. This is the
        // compile-time invariant that the upcoming window-scoped setters rely
        // on -- main is never "closed" the way an extra can be, so calling
        // `data.window(WindowId::Main)` after construction must always succeed.
        let data = make_workspace();
        let w = data.window(WindowId::Main).expect("main always present");
        assert_eq!(w.id, data.main_window.id);
    }

    #[test]
    fn window_lookup_extra_by_id_round_trips() {
        // Mint an extra, push it into extra_windows, then look it up by its
        // own id. Returns the same WindowState (by id equality).
        let mut data = make_workspace();
        let extra = WindowState::default();
        let extra_id = extra.id;
        data.extra_windows.push(extra);

        let w = data
            .window(WindowId::Extra(extra_id))
            .expect("extra was just pushed");
        assert_eq!(w.id, extra_id);
    }

    #[test]
    fn window_lookup_unknown_extra_returns_none() {
        // The "targeted window was just closed" signal -- window-scoped setters
        // will treat None as a silent no-op rather than an error. Pin the
        // contract so a future refactor that switches the lookup to a
        // panicking variant has to own the breakage.
        let data = make_workspace();
        let unknown = uuid::Uuid::new_v4();
        assert!(data.window(WindowId::Extra(unknown)).is_none());
    }

    #[test]
    fn window_mut_extra_mutates_only_target() {
        // Mutable lookup must mutate the targeted extra without disturbing
        // siblings. Construct two extras, mutate one via window_mut, assert
        // the other is unchanged.
        let mut data = make_workspace();
        let a = WindowState::default();
        let b = WindowState::default();
        let a_id = a.id;
        let b_id = b.id;
        data.extra_windows.push(a);
        data.extra_windows.push(b);

        let target = data
            .window_mut(WindowId::Extra(a_id))
            .expect("extra a was just pushed");
        target.folder_filter = Some("f1".to_string());

        let after_a = data.window(WindowId::Extra(a_id)).unwrap();
        assert_eq!(after_a.folder_filter.as_deref(), Some("f1"));
        let after_b = data.window(WindowId::Extra(b_id)).unwrap();
        assert!(after_b.folder_filter.is_none());
    }

    #[test]
    fn window_mut_main_writes_to_main_slot() {
        // window_mut(WindowId::Main) returns &mut main_window. Pin the
        // contract so the upcoming window-scoped setters can rely on Main
        // always producing a writable handle.
        let mut data = make_workspace();
        let target = data.window_mut(WindowId::Main).expect("main always present");
        target.hidden_project_ids.insert("p1".to_string());
        assert!(data.main_window.hidden_project_ids.contains("p1"));
    }

    #[test]
    fn set_folder_filter_writes_to_main_window() {
        // WindowId::Main routes the write through window_mut to the main slot.
        // Pins the smallest window-scoped setter contract: Main always succeeds
        // and Some(value) lands on main_window.folder_filter.
        let mut data = make_workspace();
        data.set_folder_filter(WindowId::Main, Some("f1".to_string()));
        assert_eq!(data.main_window.folder_filter.as_deref(), Some("f1"));
    }

    #[test]
    fn set_folder_filter_clears_with_none() {
        // Passing None must clear the filter. Without this, callers wanting to
        // exit folder-filter mode would have no API path -- the field would be
        // write-only.
        let mut data = make_workspace();
        data.main_window.folder_filter = Some("f1".to_string());
        data.set_folder_filter(WindowId::Main, None);
        assert!(data.main_window.folder_filter.is_none());
    }

    #[test]
    fn set_folder_filter_writes_to_targeted_extra() {
        // Mint two extras, set filter on one via its WindowId::Extra(uuid).
        // The targeted extra gets the filter; the sibling extra and the main
        // window are untouched. Defends against a regression that ignores the
        // id and writes to main, or scatters the write across all extras.
        let mut data = make_workspace();
        let a = WindowState::default();
        let b = WindowState::default();
        let a_id = a.id;
        let b_id = b.id;
        data.extra_windows.push(a);
        data.extra_windows.push(b);

        data.set_folder_filter(WindowId::Extra(a_id), Some("f1".to_string()));

        assert_eq!(
            data.window(WindowId::Extra(a_id)).unwrap().folder_filter.as_deref(),
            Some("f1"),
        );
        assert!(data.window(WindowId::Extra(b_id)).unwrap().folder_filter.is_none());
        assert!(data.main_window.folder_filter.is_none());
    }

    #[test]
    fn set_folder_filter_unknown_extra_is_silent_noop() {
        // The "targeted window was just closed" race -- the upcoming Workspace
        // entity will treat unknown ids as a silent no-op rather than panic.
        // Mint an extra so there is a sibling to verify is left untouched, then
        // call with a fresh uuid that does not match any window.
        let mut data = make_workspace();
        let extra = WindowState::default();
        let extra_id = extra.id;
        data.extra_windows.push(extra);

        let unknown = uuid::Uuid::new_v4();
        data.set_folder_filter(WindowId::Extra(unknown), Some("f1".to_string()));

        assert!(data.window(WindowId::Extra(extra_id)).unwrap().folder_filter.is_none());
        assert!(data.main_window.folder_filter.is_none());
    }

    #[test]
    fn toggle_hidden_inserts_when_absent() {
        // First-toggle contract: an unhidden project becomes hidden. Pins the
        // smallest leg of the toggle semantics; without this a future
        // refactor that always-removes (or always-inserts) would silently
        // break the "Hide Project" sidebar action when invoked on a visible
        // project.
        let mut data = make_workspace();
        data.toggle_hidden(WindowId::Main, "p1");
        assert!(data.main_window.hidden_project_ids.contains("p1"));
    }

    #[test]
    fn toggle_hidden_removes_when_present() {
        // Second-toggle contract: an already-hidden project becomes visible.
        // Defends against a regression that always-inserts (which would
        // leave the project stuck hidden after the user clicks "Show
        // Project"). Pinned separately from the insert leg because the two
        // halves are easy to break independently.
        let mut data = make_workspace();
        data.main_window.hidden_project_ids.insert("p1".to_string());
        data.toggle_hidden(WindowId::Main, "p1");
        assert!(!data.main_window.hidden_project_ids.contains("p1"));
    }

    #[test]
    fn toggle_hidden_writes_to_targeted_extra() {
        // Mint two extras, toggle on one via WindowId::Extra(uuid). The
        // targeted extra's hidden set gains the project; the sibling extra
        // and the main window are untouched. Defends against a regression
        // that ignores the id and writes to main, or scatters the write
        // across all extras.
        let mut data = make_workspace();
        let a = WindowState::default();
        let b = WindowState::default();
        let a_id = a.id;
        let b_id = b.id;
        data.extra_windows.push(a);
        data.extra_windows.push(b);

        data.toggle_hidden(WindowId::Extra(a_id), "p1");

        assert!(data
            .window(WindowId::Extra(a_id))
            .unwrap()
            .hidden_project_ids
            .contains("p1"));
        assert!(!data
            .window(WindowId::Extra(b_id))
            .unwrap()
            .hidden_project_ids
            .contains("p1"));
        assert!(!data.main_window.hidden_project_ids.contains("p1"));
    }

    #[test]
    fn toggle_hidden_unknown_extra_is_silent_noop() {
        // The "targeted window was just closed" race -- the upcoming Workspace
        // entity will treat unknown ids as a silent no-op rather than panic.
        // Mint an extra to verify it is left untouched, then call with a
        // fresh uuid that does not match any window.
        let mut data = make_workspace();
        let extra = WindowState::default();
        let extra_id = extra.id;
        data.extra_windows.push(extra);

        let unknown = uuid::Uuid::new_v4();
        data.toggle_hidden(WindowId::Extra(unknown), "p1");

        assert!(data
            .window(WindowId::Extra(extra_id))
            .unwrap()
            .hidden_project_ids
            .is_empty());
        assert!(data.main_window.hidden_project_ids.is_empty());
    }

    #[test]
    fn set_project_width_writes_to_main_window() {
        // WindowId::Main routes the write through window_mut to the main slot.
        // Pins the smallest leg of the per-window column-width contract: a
        // single (project_id, width) pair lands on main_window.project_widths.
        let mut data = make_workspace();
        data.set_project_width(WindowId::Main, "p1", 0.42);
        assert_eq!(data.main_window.project_widths.get("p1").copied(), Some(0.42));
    }

    #[test]
    fn set_project_width_overwrites_existing_value() {
        // Re-setting a width for the same project replaces the previous value
        // rather than ignoring or appending. Defends against a regression that
        // uses HashMap::entry().or_insert (which would silently keep the old
        // value on a column-resize).
        let mut data = make_workspace();
        data.set_project_width(WindowId::Main, "p1", 0.25);
        data.set_project_width(WindowId::Main, "p1", 0.75);
        assert_eq!(data.main_window.project_widths.get("p1").copied(), Some(0.75));
    }

    #[test]
    fn set_project_width_writes_to_targeted_extra() {
        // Mint two extras, set width on one via WindowId::Extra(uuid). The
        // targeted extra's project_widths gains the entry; the sibling extra
        // and the main window are untouched. Defends against a regression
        // that ignores the id and writes to main, or scatters the write
        // across all extras.
        let mut data = make_workspace();
        let a = WindowState::default();
        let b = WindowState::default();
        let a_id = a.id;
        let b_id = b.id;
        data.extra_windows.push(a);
        data.extra_windows.push(b);

        data.set_project_width(WindowId::Extra(a_id), "p1", 0.42);

        assert_eq!(
            data.window(WindowId::Extra(a_id)).unwrap().project_widths.get("p1").copied(),
            Some(0.42),
        );
        assert!(data.window(WindowId::Extra(b_id)).unwrap().project_widths.is_empty());
        assert!(data.main_window.project_widths.is_empty());
    }

    #[test]
    fn set_project_width_unknown_extra_is_silent_noop() {
        // The "targeted window was just closed" race -- the upcoming Workspace
        // entity will treat unknown ids as a silent no-op rather than panic.
        // Mint an extra to verify it is left untouched, then call with a
        // fresh uuid that does not match any window.
        let mut data = make_workspace();
        let extra = WindowState::default();
        let extra_id = extra.id;
        data.extra_windows.push(extra);

        let unknown = uuid::Uuid::new_v4();
        data.set_project_width(WindowId::Extra(unknown), "p1", 0.42);

        assert!(data.window(WindowId::Extra(extra_id)).unwrap().project_widths.is_empty());
        assert!(data.main_window.project_widths.is_empty());
    }

    #[test]
    fn set_folder_collapsed_inserts_when_true() {
        // WindowId::Main + collapsed=true routes the write through window_mut to
        // the main slot, inserting (folder_id, true) into folder_collapsed. Pins
        // the smallest leg of the per-window folder-collapse contract.
        let mut data = make_workspace();
        data.set_folder_collapsed(WindowId::Main, "f1", true);
        assert_eq!(data.main_window.folder_collapsed.get("f1").copied(), Some(true));
    }

    #[test]
    fn set_folder_collapsed_false_removes_existing_entry() {
        // The "absence == expanded" runtime convention -- the toggle entry point
        // (Workspace::toggle_folder_collapsed) removes the entry when collapsing
        // back to expanded rather than storing `false`. The pure setter mirrors
        // that convention: collapsed=false removes any existing entry. Defends
        // against a regression that uses `insert(folder_id, collapsed)`
        // unconditionally (which would store explicit `false` entries and
        // diverge from the runtime convention).
        let mut data = make_workspace();
        data.main_window.folder_collapsed.insert("f1".to_string(), true);
        data.set_folder_collapsed(WindowId::Main, "f1", false);
        assert!(!data.main_window.folder_collapsed.contains_key("f1"));
    }

    #[test]
    fn set_folder_collapsed_false_on_missing_entry_is_noop() {
        // Setting collapsed=false for a folder that is not in the map is a
        // no-op (it is already expanded). Defends against a regression that
        // panics or inserts a stub entry.
        let mut data = make_workspace();
        data.set_folder_collapsed(WindowId::Main, "f1", false);
        assert!(data.main_window.folder_collapsed.is_empty());
    }

    #[test]
    fn set_folder_collapsed_writes_to_targeted_extra() {
        // Mint two extras, set collapse on one via WindowId::Extra(uuid). The
        // targeted extra's folder_collapsed gains the entry; the sibling extra
        // and the main window are untouched. Defends against a regression that
        // ignores the id and writes to main, or scatters the write across all
        // extras.
        let mut data = make_workspace();
        let a = WindowState::default();
        let b = WindowState::default();
        let a_id = a.id;
        let b_id = b.id;
        data.extra_windows.push(a);
        data.extra_windows.push(b);

        data.set_folder_collapsed(WindowId::Extra(a_id), "f1", true);

        assert_eq!(
            data.window(WindowId::Extra(a_id)).unwrap().folder_collapsed.get("f1").copied(),
            Some(true),
        );
        assert!(data.window(WindowId::Extra(b_id)).unwrap().folder_collapsed.is_empty());
        assert!(data.main_window.folder_collapsed.is_empty());
    }

    #[test]
    fn set_folder_collapsed_unknown_extra_is_silent_noop() {
        // The "targeted window was just closed" race -- the upcoming Workspace
        // entity will treat unknown ids as a silent no-op rather than panic.
        // Mint an extra to verify it is left untouched, then call with a fresh
        // uuid that does not match any window.
        let mut data = make_workspace();
        let extra = WindowState::default();
        let extra_id = extra.id;
        data.extra_windows.push(extra);

        let unknown = uuid::Uuid::new_v4();
        data.set_folder_collapsed(WindowId::Extra(unknown), "f1", true);

        assert!(data.window(WindowId::Extra(extra_id)).unwrap().folder_collapsed.is_empty());
        assert!(data.main_window.folder_collapsed.is_empty());
    }

    #[test]
    fn set_os_bounds_writes_to_main_window() {
        // WindowId::Main + Some(bounds) routes the write through window_mut to
        // the main slot. Pins the smallest leg of the per-window os-bounds
        // contract: Some(WindowBounds) lands on main_window.os_bounds. Mirrors
        // the set_folder_filter shape since both fields are Option-typed.
        let mut data = make_workspace();
        let bounds = WindowBounds {
            origin_x: 100.0,
            origin_y: 50.0,
            width: 1280.0,
            height: 800.0,
        };
        data.set_os_bounds(WindowId::Main, Some(bounds));
        assert_eq!(data.main_window.os_bounds, Some(bounds));
    }

    #[test]
    fn set_os_bounds_clears_with_none() {
        // Passing None must clear the bounds. Without this, callers wanting to
        // forget a window's last position would have no API path -- the field
        // would be write-only. Mirrors set_folder_filter_clears_with_none.
        let mut data = make_workspace();
        data.main_window.os_bounds = Some(WindowBounds {
            origin_x: 0.0,
            origin_y: 0.0,
            width: 800.0,
            height: 600.0,
        });
        data.set_os_bounds(WindowId::Main, None);
        assert!(data.main_window.os_bounds.is_none());
    }

    #[test]
    fn set_os_bounds_writes_to_targeted_extra() {
        // Mint two extras, set bounds on one via WindowId::Extra(uuid). The
        // targeted extra gets the bounds; the sibling extra and the main
        // window are untouched. Defends against a regression that ignores the
        // id and writes to main, or scatters the write across all extras.
        let mut data = make_workspace();
        let a = WindowState::default();
        let b = WindowState::default();
        let a_id = a.id;
        let b_id = b.id;
        data.extra_windows.push(a);
        data.extra_windows.push(b);

        let bounds = WindowBounds {
            origin_x: 200.0,
            origin_y: 150.0,
            width: 1024.0,
            height: 768.0,
        };
        data.set_os_bounds(WindowId::Extra(a_id), Some(bounds));

        assert_eq!(
            data.window(WindowId::Extra(a_id)).unwrap().os_bounds,
            Some(bounds),
        );
        assert!(data.window(WindowId::Extra(b_id)).unwrap().os_bounds.is_none());
        assert!(data.main_window.os_bounds.is_none());
    }

    #[test]
    fn set_os_bounds_unknown_extra_is_silent_noop() {
        // The "targeted window was just closed" race -- the upcoming Workspace
        // entity will treat unknown ids as a silent no-op rather than panic.
        // Mint an extra to verify it is left untouched, then call with a fresh
        // uuid that does not match any window.
        let mut data = make_workspace();
        let extra = WindowState::default();
        let extra_id = extra.id;
        data.extra_windows.push(extra);

        let unknown = uuid::Uuid::new_v4();
        let bounds = WindowBounds {
            origin_x: 1.0,
            origin_y: 2.0,
            width: 3.0,
            height: 4.0,
        };
        data.set_os_bounds(WindowId::Extra(unknown), Some(bounds));

        assert!(data.window(WindowId::Extra(extra_id)).unwrap().os_bounds.is_none());
        assert!(data.main_window.os_bounds.is_none());
    }

    #[test]
    fn delete_project_scrub_all_windows_removes_from_main_hidden_and_widths() {
        // Pin the smallest leg of the contract: a project's id is removed from
        // both per-project storages on main_window. A regression that scrubbed
        // only one of the two would leave a tombstone in the other (e.g.
        // hidden_project_ids cleared but project_widths still pointing at a
        // gone project) -- a subtle bug that would only surface as orphan
        // entries in workspace.json over time.
        let mut data = make_workspace();
        data.main_window.hidden_project_ids.insert("p1".to_string());
        data.main_window.project_widths.insert("p1".to_string(), 0.42);

        data.delete_project_scrub_all_windows("p1");

        assert!(!data.main_window.hidden_project_ids.contains("p1"));
        assert!(!data.main_window.project_widths.contains_key("p1"));
    }

    #[test]
    fn delete_project_scrub_all_windows_removes_from_every_extra() {
        // Mint two extras with the project id present in both per-project
        // storages on each. After the scrub, every extra is clean. Defends
        // against a regression that scrubs only main, only the first extra,
        // or stops at the first match (a "found one, done" early-return).
        let mut data = make_workspace();
        let mut a = WindowState::default();
        a.hidden_project_ids.insert("p1".to_string());
        a.project_widths.insert("p1".to_string(), 0.30);
        let mut b = WindowState::default();
        b.hidden_project_ids.insert("p1".to_string());
        b.project_widths.insert("p1".to_string(), 0.70);
        let a_id = a.id;
        let b_id = b.id;
        data.extra_windows.push(a);
        data.extra_windows.push(b);

        data.delete_project_scrub_all_windows("p1");

        let after_a = data.window(WindowId::Extra(a_id)).unwrap();
        assert!(!after_a.hidden_project_ids.contains("p1"));
        assert!(!after_a.project_widths.contains_key("p1"));
        let after_b = data.window(WindowId::Extra(b_id)).unwrap();
        assert!(!after_b.hidden_project_ids.contains("p1"));
        assert!(!after_b.project_widths.contains_key("p1"));
    }

    #[test]
    fn delete_project_scrub_all_windows_leaves_other_projects_alone() {
        // A scrub of p1 must not disturb p2's entries on any window. Defends
        // against a regression that clears the entire hidden set / widths map
        // rather than removing the targeted id.
        let mut data = make_workspace();
        data.main_window.hidden_project_ids.insert("p1".to_string());
        data.main_window.hidden_project_ids.insert("p2".to_string());
        data.main_window.project_widths.insert("p1".to_string(), 0.25);
        data.main_window.project_widths.insert("p2".to_string(), 0.75);
        let mut extra = WindowState::default();
        extra.hidden_project_ids.insert("p2".to_string());
        extra.project_widths.insert("p2".to_string(), 0.50);
        let extra_id = extra.id;
        data.extra_windows.push(extra);

        data.delete_project_scrub_all_windows("p1");

        assert!(data.main_window.hidden_project_ids.contains("p2"));
        assert_eq!(data.main_window.project_widths.get("p2").copied(), Some(0.75));
        let after = data.window(WindowId::Extra(extra_id)).unwrap();
        assert!(after.hidden_project_ids.contains("p2"));
        assert_eq!(after.project_widths.get("p2").copied(), Some(0.50));
    }

    #[test]
    fn delete_project_scrub_all_windows_unknown_id_is_noop() {
        // Idempotent contract: a project id absent from every window is a
        // no-op. Defends against a regression that panics on a missing-key
        // remove (HashMap/HashSet remove return Option/bool and never panic,
        // but a hypothetical refactor to a different data structure with
        // stricter semantics would). Pre-populate sibling state so the
        // assertion checks "nothing was touched", not "everything is empty".
        let mut data = make_workspace();
        data.main_window.hidden_project_ids.insert("p2".to_string());
        data.main_window.project_widths.insert("p2".to_string(), 0.42);
        let mut extra = WindowState::default();
        extra.hidden_project_ids.insert("p2".to_string());
        let extra_id = extra.id;
        data.extra_windows.push(extra);

        data.delete_project_scrub_all_windows("unknown_id");

        assert!(data.main_window.hidden_project_ids.contains("p2"));
        assert_eq!(data.main_window.project_widths.get("p2").copied(), Some(0.42));
        let after = data.window(WindowId::Extra(extra_id)).unwrap();
        assert!(after.hidden_project_ids.contains("p2"));
    }

    #[test]
    fn delete_project_scrub_all_windows_does_not_touch_unrelated_per_window_fields() {
        // The scrub is scoped to per-project storage (hidden_project_ids,
        // project_widths). The folder_collapsed map is keyed by folder id (not
        // project id), folder_filter is a folder-id Option, and os_bounds is
        // not per-project. None of these may be cleared as a side-effect of
        // a project delete. Defends against a regression that "clear every
        // map on the targeted window" would silently break window state on
        // every project delete.
        let mut data = make_workspace();
        data.main_window.hidden_project_ids.insert("p1".to_string());
        data.main_window.project_widths.insert("p1".to_string(), 0.42);
        data.main_window.folder_collapsed.insert("f1".to_string(), true);
        data.main_window.folder_filter = Some("f1".to_string());
        data.main_window.os_bounds = Some(WindowBounds {
            origin_x: 1.0,
            origin_y: 2.0,
            width: 3.0,
            height: 4.0,
        });

        data.delete_project_scrub_all_windows("p1");

        assert_eq!(data.main_window.folder_collapsed.get("f1").copied(), Some(true));
        assert_eq!(data.main_window.folder_filter.as_deref(), Some("f1"));
        assert!(data.main_window.os_bounds.is_some());
    }

    #[test]
    fn workspace_data_has_no_top_level_project_widths_field() {
        // Per-window column widths live exclusively on main_window.project_widths.
        // The legacy top-level WorkspaceData.project_widths field has been
        // removed from the struct entirely (not just tombstoned for save) --
        // serialization must not produce a top-level "project_widths" key.
        let data = make_workspace();
        let saved = serde_json::to_string(&data).unwrap();
        let value: serde_json::Value = serde_json::from_str(&saved).unwrap();
        assert!(!value.as_object().unwrap().contains_key("project_widths"),
            "top-level project_widths must not appear in serialized form (field removed)");
    }

    #[test]
    fn spawn_extra_window_starts_with_every_current_project_hidden() {
        // PRD `plans/multi-window.md` line 26: "I want a new window to start
        // empty (no project columns visible) so that I can deliberately
        // curate what goes in it without inheriting noise from elsewhere."
        // Implementation: snapshot every current project ID into the new
        // window's hidden_project_ids set so the grid is empty at spawn.
        // Pins the behavior that the new extra is fully filtered, not a
        // copy of main's filter state and not an empty-hidden window.
        let mut data = make_workspace();
        let mut p1 = make_project("/p1");
        p1.id = "p1".to_string();
        let mut p2 = make_project("/p2");
        p2.id = "p2".to_string();
        data.projects = vec![p1, p2];

        let new_id = data.spawn_extra_window(None);

        let new_window = data
            .window(new_id)
            .expect("spawn_extra_window returns a live id");
        assert!(new_window.hidden_project_ids.contains("p1"));
        assert!(new_window.hidden_project_ids.contains("p2"));
        assert_eq!(new_window.hidden_project_ids.len(), 2);
    }

    #[test]
    fn spawn_extra_window_returns_extra_id_pointing_at_pushed_entry() {
        // Returned WindowId must be `WindowId::Extra(uuid)` (never Main) and
        // must address the just-pushed entry. Pins the contract that callers
        // can immediately use the returned id with `window_mut` to seed
        // bounds, folder_filter, etc. without re-walking `extra_windows` for
        // the entry they just created.
        let mut data = make_workspace();
        let new_id = data.spawn_extra_window(None);

        match new_id {
            WindowId::Main => panic!("spawn_extra_window must not return Main"),
            WindowId::Extra(uuid) => {
                assert_eq!(data.extra_windows.len(), 1);
                assert_eq!(data.extra_windows[0].id, uuid);
            }
        }
    }

    #[test]
    fn spawn_extra_window_appends_distinct_entries_per_call() {
        // Two consecutive spawn calls produce two distinct extras with
        // distinct ids -- not a single coalesced entry, not the same uuid.
        // Pins the contract that the spawn flow can be invoked repeatedly
        // (Cmd+Shift+N, Cmd+Shift+N) and each press yields its own window.
        let mut data = make_workspace();
        let id_a = data.spawn_extra_window(None);
        let id_b = data.spawn_extra_window(None);

        assert_ne!(id_a, id_b);
        assert_eq!(data.extra_windows.len(), 2);
    }

    #[test]
    fn spawn_extra_window_no_spawning_bounds_leaves_os_bounds_none() {
        // When no spawning bounds are supplied (e.g. the action handler
        // could not read its window's live bounds), the new entry's
        // os_bounds stays None so the OS picks a default position. Mirrors
        // the prior "leaves os_bounds at default" contract from before
        // the cascade-offset parameter landed.
        let mut data = make_workspace();
        let new_id = data.spawn_extra_window(None);
        let new_window = data.window(new_id).expect("spawn returns a live id");
        assert!(new_window.os_bounds.is_none());
    }

    #[test]
    fn spawn_extra_window_with_spawning_bounds_cascades_origin_by_30_30_preserves_size() {
        // PRD line 27 + slice 05 cri 2: "I want a new window to cascade-
        // offset from the spawning window's position so that it does not
        // stack invisibly on top." Cascade rule (slice 05 notes line 57):
        // shift origin by +30,+30, keep the same size, persist into the
        // new entry's os_bounds. Pins the cascade arithmetic at the data
        // layer so the observer can pass os_bounds straight into
        // cx.open_window without recomputing.
        let mut data = make_workspace();
        let spawning = WindowBounds {
            origin_x: 100.0,
            origin_y: 200.0,
            width: 1280.0,
            height: 800.0,
        };
        let new_id = data.spawn_extra_window(Some(spawning));
        let new_window = data.window(new_id).expect("spawn returns a live id");
        let bounds = new_window.os_bounds.expect("os_bounds seeded by cascade");
        assert_eq!(bounds.origin_x, 130.0);
        assert_eq!(bounds.origin_y, 230.0);
        assert_eq!(bounds.width, 1280.0);
        assert_eq!(bounds.height, 800.0);
    }

    #[test]
    fn add_project_hide_in_other_windows_main_spawn_inserts_in_extras_only() {
        // PRD user story 14 + slice 06 cri 6: "add_project_from_window with
        // two extras present -- project ID is in both extras' hidden set,
        // not in main_window.hidden_project_ids." Pin the rule's main-spawn
        // direction: id lands in every extra; main stays clean so the new
        // project is visible there.
        let mut data = make_workspace();
        let extra_a = WindowState::default();
        let extra_a_id = extra_a.id;
        let extra_b = WindowState::default();
        let extra_b_id = extra_b.id;
        data.extra_windows = vec![extra_a, extra_b];

        data.add_project_hide_in_other_windows("p1", WindowId::Main);

        assert!(!data.main_window.hidden_project_ids.contains("p1"));
        let after_a = data.window(WindowId::Extra(extra_a_id)).unwrap();
        assert!(after_a.hidden_project_ids.contains("p1"));
        let after_b = data.window(WindowId::Extra(extra_b_id)).unwrap();
        assert!(after_b.hidden_project_ids.contains("p1"));
    }

    #[test]
    fn add_project_hide_in_other_windows_extra_spawn_inserts_in_main_and_other_extras() {
        // PRD user story 14 + slice 06 cri 7: "same call with WindowId::Extra
        // -- main's hidden set gets it, the targeted extra's does not." Pin
        // the extra-spawn direction: id lands in main + every sibling extra,
        // but the spawning extra stays clean so the new project is visible
        // there. Defends against a regression that always writes to main or
        // skips the wrong extra.
        let mut data = make_workspace();
        let extra_a = WindowState::default();
        let extra_a_id = extra_a.id;
        let extra_b = WindowState::default();
        let extra_b_id = extra_b.id;
        data.extra_windows = vec![extra_a, extra_b];

        data.add_project_hide_in_other_windows("p1", WindowId::Extra(extra_a_id));

        assert!(data.main_window.hidden_project_ids.contains("p1"));
        let after_a = data.window(WindowId::Extra(extra_a_id)).unwrap();
        assert!(!after_a.hidden_project_ids.contains("p1"));
        let after_b = data.window(WindowId::Extra(extra_b_id)).unwrap();
        assert!(after_b.hidden_project_ids.contains("p1"));
    }

    #[test]
    fn add_project_hide_in_other_windows_no_extras_main_spawn_is_noop() {
        // Single-window case: zero extras + spawn from Main -> no other
        // window exists to hide in, main stays clean. Slice 06 notes line 41:
        // "If only main exists (zero extras), the helper degenerates to a
        // no-op for the hide-elsewhere step. Single-window users see no
        // behavior change." Pre-populate main with a sibling project's
        // hidden state to ensure the call doesn't accidentally touch other
        // entries on main.
        let mut data = make_workspace();
        data.main_window.hidden_project_ids.insert("sibling".to_string());

        data.add_project_hide_in_other_windows("p1", WindowId::Main);

        assert!(!data.main_window.hidden_project_ids.contains("p1"));
        // Sibling state preserved.
        assert!(data.main_window.hidden_project_ids.contains("sibling"));
    }

    #[test]
    fn add_project_hide_in_other_windows_unknown_extra_hides_everywhere() {
        // Defensive contract: an Extra(uuid) that does not match any live
        // extra (e.g. caller raced a close, or a sentinel id signaling "no
        // spawning window") falls through both the main-skip and the
        // extra-skip and inserts the id into every window. The new project
        // has no live viewport that would benefit from default visibility,
        // so the rule degenerates to fully hidden. Mirrors the silent-no-op
        // shape of window-scoped setters targeting unknown extras.
        let mut data = make_workspace();
        let extra = WindowState::default();
        let extra_id = extra.id;
        data.extra_windows = vec![extra];

        let unknown = uuid::Uuid::new_v4();
        data.add_project_hide_in_other_windows("p1", WindowId::Extra(unknown));

        assert!(data.main_window.hidden_project_ids.contains("p1"));
        let after = data.window(WindowId::Extra(extra_id)).unwrap();
        assert!(after.hidden_project_ids.contains("p1"));
    }

    #[test]
    fn hide_project_in_all_windows_inserts_in_main_and_extras() {
        let mut data = make_workspace();
        let extra_a = WindowState::default();
        let extra_a_id = extra_a.id;
        let extra_b = WindowState::default();
        let extra_b_id = extra_b.id;
        data.extra_windows = vec![extra_a, extra_b];

        data.hide_project_in_all_windows("p1");

        assert!(data.main_window.hidden_project_ids.contains("p1"));
        let after_a = data.window(WindowId::Extra(extra_a_id)).unwrap();
        assert!(after_a.hidden_project_ids.contains("p1"));
        let after_b = data.window(WindowId::Extra(extra_b_id)).unwrap();
        assert!(after_b.hidden_project_ids.contains("p1"));
    }

    #[test]
    fn add_project_hide_in_other_windows_idempotent_on_duplicate_call() {
        // Running the same rule twice for the same id is a no-op on the
        // second pass. Pins the contract that re-applying the rule (e.g.
        // a caller that defensively re-runs after a state-mutation path)
        // doesn't toggle visibility back on. HashSet::insert returns bool
        // but never panics on duplicate; the test pins that we don't rely
        // on first-insert semantics.
        let mut data = make_workspace();
        let extra = WindowState::default();
        let extra_id = extra.id;
        data.extra_windows = vec![extra];

        data.add_project_hide_in_other_windows("p1", WindowId::Main);
        data.add_project_hide_in_other_windows("p1", WindowId::Main);

        let after = data.window(WindowId::Extra(extra_id)).unwrap();
        assert!(after.hidden_project_ids.contains("p1"));
        assert_eq!(after.hidden_project_ids.iter().filter(|id| *id == "p1").count(), 1);
    }

    #[test]
    fn add_project_hide_in_other_windows_does_not_touch_widths_or_filter() {
        // The rule is scoped to hidden_project_ids. Sibling per-window
        // storage (project_widths, folder_collapsed, folder_filter,
        // os_bounds) must not be touched. Defends against a regression that
        // "clear every map on the targeted window" would silently break
        // window state on every project add.
        let mut data = make_workspace();
        let mut extra = WindowState::default();
        extra.project_widths.insert("sibling".to_string(), 0.42);
        extra.folder_collapsed.insert("f1".to_string(), true);
        extra.folder_filter = Some("f1".to_string());
        let extra_id = extra.id;
        data.extra_windows = vec![extra];

        data.add_project_hide_in_other_windows("p1", WindowId::Main);

        let after = data.window(WindowId::Extra(extra_id)).unwrap();
        assert_eq!(after.project_widths.get("sibling").copied(), Some(0.42));
        assert_eq!(after.folder_collapsed.get("f1").copied(), Some(true));
        assert_eq!(after.folder_filter.as_deref(), Some("f1"));
    }

    #[test]
    fn close_extra_window_removes_matching_entry_only() {
        // Slice 07 cri 3: close-extra removes the entry from
        // `extra_windows`. Pin the lookup-by-id contract: the call removes
        // exactly the targeted entry, leaving siblings (and main) untouched.
        // Defends against a regression that walks the Vec by index (which
        // would silently target the wrong sibling after intermediate removes)
        // or that scrubs more than the targeted entry.
        let mut data = make_workspace();
        let id_a = data.spawn_extra_window(None);
        let id_b = data.spawn_extra_window(None);
        let id_c = data.spawn_extra_window(None);
        assert_eq!(data.extra_windows.len(), 3);

        data.close_extra_window(id_b);

        assert_eq!(data.extra_windows.len(), 2);
        assert!(data.window(id_a).is_some(), "sibling A survives");
        assert!(data.window(id_b).is_none(), "targeted B is gone");
        assert!(data.window(id_c).is_some(), "sibling C survives");
    }

    #[test]
    fn close_extra_window_main_is_silent_noop() {
        // PRD line 53 + slice 07 cri 4: main is the always-present slot;
        // closing main quits the app via `LastWindowClosed`, it does NOT
        // delete persisted main state. Targeting `WindowId::Main` at this
        // helper must be a silent no-op so a future caller that
        // unconditionally routes a close event through here cannot
        // accidentally erase main's hidden set / widths / folder filter.
        let mut data = make_workspace();
        data.main_window.hidden_project_ids.insert("p1".to_string());
        let id_extra = data.spawn_extra_window(None);

        data.close_extra_window(WindowId::Main);

        // Main untouched.
        assert!(data.main_window.hidden_project_ids.contains("p1"));
        // Extras untouched.
        assert_eq!(data.extra_windows.len(), 1);
        assert!(data.window(id_extra).is_some());
    }

    #[test]
    fn close_extra_window_unknown_extra_is_silent_noop() {
        // Close-race contract: a fresh uuid that does not match any live
        // extra (e.g. a double-close where two close events fire for the
        // same OS window, or a save-then-rebuild race where the entry was
        // already pruned) is a silent no-op. Mirrors the silent-no-op shape
        // of every other window-scoped operation in this module — callers
        // do not need to pre-check existence.
        let mut data = make_workspace();
        let id_extra = data.spawn_extra_window(None);

        data.close_extra_window(WindowId::Extra(uuid::Uuid::new_v4()));

        assert_eq!(data.extra_windows.len(), 1);
        assert!(data.window(id_extra).is_some());
    }

    #[test]
    fn spawn_extra_window_two_calls_with_same_spawning_bounds_cascade_independently() {
        // Each spawn computes the cascade offset from its own caller-
        // supplied bounds; the data layer does not track "previous spawn"
        // state to chain cascades automatically. Cri 5 ("no cap") + cri
        // 6 ("from extra cascades from that extra") rely on the caller
        // (action handler) reading its own window's live bounds at each
        // press -- so two presses from the same window produce two extras
        // both at the same +30,+30 from that window. Pins the data
        // layer's stateless contract.
        let mut data = make_workspace();
        let spawning = WindowBounds {
            origin_x: 100.0,
            origin_y: 100.0,
            width: 800.0,
            height: 600.0,
        };
        let _ = data.spawn_extra_window(Some(spawning));
        let _ = data.spawn_extra_window(Some(spawning));

        let a = data.extra_windows[0].os_bounds.expect("first cascade");
        let b = data.extra_windows[1].os_bounds.expect("second cascade");
        assert_eq!(a.origin_x, 130.0);
        assert_eq!(a.origin_y, 130.0);
        assert_eq!(b.origin_x, 130.0);
        assert_eq!(b.origin_y, 130.0);
    }
}
