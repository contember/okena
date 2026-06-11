//! Per-window viewport state.
//!
//! A `WindowState` is the filter/UI state for one window onto the shared
//! workspace: which projects are hidden in this window, the active folder
//! filter, per-project column widths, sidebar folder-collapse map, and OS
//! window bounds. Pure data — see PRD `plans/multi-window.md`.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

/// How project columns are arranged in a window's grid.
///
/// `Columns` (default) lays projects out side by side, each with a width;
/// `Rows` stacks them vertically, each with a height. Stored per-window so
/// each window can flip its own orientation independently. The persisted
/// `project_widths` map holds axis-agnostic percentages, so it carries over
/// unchanged when the orientation flips.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectLayoutMode {
    /// Projects laid out side by side (horizontal, width-resized). Default.
    #[default]
    Columns,
    /// Projects stacked vertically (height-resized).
    Rows,
}

impl ProjectLayoutMode {
    /// Return the opposite orientation.
    pub fn toggled(self) -> Self {
        match self {
            ProjectLayoutMode::Columns => ProjectLayoutMode::Rows,
            ProjectLayoutMode::Rows => ProjectLayoutMode::Columns,
        }
    }

    /// True when projects are stacked vertically.
    pub fn is_rows(self) -> bool {
        matches!(self, ProjectLayoutMode::Rows)
    }
}

/// How the sidebar orders projects in a window.
///
/// `Manual` (default) follows the persisted `project_order` and folder
/// grouping — the user's hand-arranged layout. `Activity` ignores
/// `project_order` and folders and instead groups projects into fixed tiers
/// (pinned, needs-attention, running, rest) sorted by recent activity, so the
/// projects that need attention float to the top. Stored per-window so each
/// window can flip its own view independently.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectSortMode {
    /// Hand-arranged order from `project_order` + folders. Default.
    #[default]
    Manual,
    /// Tiered, activity-sorted view that ignores `project_order` and folders.
    Activity,
}

impl ProjectSortMode {
    /// Return the other mode.
    pub fn toggled(self) -> Self {
        match self {
            ProjectSortMode::Manual => ProjectSortMode::Activity,
            ProjectSortMode::Activity => ProjectSortMode::Manual,
        }
    }

    /// True when the sidebar should use the activity-sorted view.
    pub fn is_activity(self) -> bool {
        matches!(self, ProjectSortMode::Activity)
    }
}

/// Restore bounds for an OS window: origin + size in screen pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WindowBounds {
    pub origin_x: f32,
    pub origin_y: f32,
    pub width: f32,
    pub height: f32,
}

/// Per-window viewport state. One instance per open window (main + extras).
///
/// `id` is the stable identity that pairs with `WindowId::Extra(Uuid)`. It is
/// load-bearing only for extras: the main slot is addressed by
/// `WindowId::Main` (not by id), so `main_window.id` is effectively ignored at
/// runtime. The field defaults to a fresh `Uuid::new_v4()` both for in-process
/// construction (`Default::default()`) and for deserialization of older
/// `workspace.json` files written before the field existed (via
/// `#[serde(default = "Uuid::new_v4")]`). Keeping the field present on every
/// `WindowState` -- main included -- avoids a per-variant struct fork and
/// keeps the on-disk shape uniform.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WindowState {
    /// Stable identity for this window. Matches `WindowId::Extra(_)` for
    /// extras; unused for the main slot (addressed by variant).
    ///
    /// **DO NOT compare `main_window.id` across `WorkspaceData` instances.**
    /// Main is addressed by `WindowId::Main`, never by id. Every default
    /// construction mints a fresh uuid, and the serde default behaves the
    /// same for legacy files written before this field existed, so two
    /// instances loaded from the same JSON can have different
    /// `main_window.id` values. Treat the field as identity-only for
    /// extras; for main it is opaque persistence padding.
    #[serde(default = "Uuid::new_v4")]
    pub id: Uuid,
    /// Project IDs hidden in this window's grid.
    #[serde(default)]
    pub hidden_project_ids: HashSet<String>,
    /// Folder filter (folder ID) limiting visible projects in this window.
    #[serde(default)]
    pub folder_filter: Option<String>,
    /// Project column widths (percentages) scoped to this window.
    ///
    /// Axis-agnostic: in `ProjectLayoutMode::Columns` these are widths, in
    /// `Rows` they are heights. The same percentage carries over when the
    /// orientation flips, so a window's relative sizing survives a toggle.
    #[serde(default)]
    pub project_widths: HashMap<String, f32>,
    /// Orientation of the project grid in this window (columns vs rows).
    #[serde(default)]
    pub project_layout: ProjectLayoutMode,
    /// How the sidebar orders projects in this window (manual vs activity).
    #[serde(default)]
    pub project_sort_mode: ProjectSortMode,
    /// Per-folder collapsed state in this window's sidebar.
    #[serde(default)]
    pub folder_collapsed: HashMap<String, bool>,
    /// Last-known OS window bounds (used to restore position on next launch).
    #[serde(default)]
    pub os_bounds: Option<WindowBounds>,
    /// Whether the sidebar is open in this window. `None` means no per-window
    /// value has been recorded yet, so callers should fall back to app settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sidebar_open: Option<bool>,
}

impl Default for WindowState {
    fn default() -> Self {
        // Fresh Uuid per default-construction so two extras minted at runtime
        // never collide. Matches the serde default for missing-on-disk ids.
        Self {
            id: Uuid::new_v4(),
            hidden_project_ids: HashSet::new(),
            folder_filter: None,
            project_widths: HashMap::new(),
            project_layout: ProjectLayoutMode::default(),
            project_sort_mode: ProjectSortMode::default(),
            folder_collapsed: HashMap::new(),
            os_bounds: None,
            sidebar_open: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_state_default_is_empty() {
        let s = WindowState::default();
        assert!(s.hidden_project_ids.is_empty());
        assert!(s.folder_filter.is_none());
        assert!(s.project_widths.is_empty());
        assert!(s.folder_collapsed.is_empty());
        assert!(s.os_bounds.is_none());
    }

    #[test]
    fn window_state_serde_roundtrip_populated() {
        let mut hidden = HashSet::new();
        hidden.insert("p1".to_string());
        hidden.insert("p2".to_string());

        let mut widths = HashMap::new();
        widths.insert("p3".to_string(), 0.42);

        let mut collapsed = HashMap::new();
        collapsed.insert("f1".to_string(), true);

        let original = WindowState {
            id: Uuid::new_v4(),
            hidden_project_ids: hidden,
            folder_filter: Some("folder-7".to_string()),
            project_widths: widths,
            project_layout: ProjectLayoutMode::Rows,
            project_sort_mode: ProjectSortMode::Activity,
            folder_collapsed: collapsed,
            os_bounds: Some(WindowBounds {
                origin_x: 100.0,
                origin_y: 50.0,
                width: 1280.0,
                height: 800.0,
            }),
            sidebar_open: Some(false),
        };

        let json = serde_json::to_string(&original).unwrap();
        let reloaded: WindowState = serde_json::from_str(&json).unwrap();

        assert_eq!(reloaded.id, original.id);
        assert_eq!(reloaded.hidden_project_ids, original.hidden_project_ids);
        assert_eq!(reloaded.folder_filter, original.folder_filter);
        assert_eq!(reloaded.project_widths, original.project_widths);
        assert_eq!(reloaded.project_layout, original.project_layout);
        assert_eq!(reloaded.project_sort_mode, original.project_sort_mode);
        assert_eq!(reloaded.folder_collapsed, original.folder_collapsed);
        assert_eq!(reloaded.os_bounds, original.os_bounds);
        assert_eq!(reloaded.sidebar_open, original.sidebar_open);
    }

    #[test]
    fn missing_sidebar_open_deserializes_as_unset() {
        let s: WindowState = serde_json::from_str("{}").unwrap();
        assert_eq!(s.sidebar_open, None);
    }

    #[test]
    fn distinct_default_window_states_have_distinct_ids() {
        // Default minting uses Uuid::new_v4() so two extras created via
        // Default::default() never collide. Pins the runtime contract that
        // `WindowId::Extra(state.id)` is unique-by-construction.
        let a = WindowState::default();
        let b = WindowState::default();
        assert_ne!(a.id, b.id);
        // And neither is the nil uuid (which is what Uuid::default() returns).
        assert_ne!(a.id, Uuid::nil());
        assert_ne!(b.id, Uuid::nil());
    }

    #[test]
    fn deserialize_missing_id_gets_fresh_non_nil_uuid() {
        // Forward-compatibility: workspace.json files written before the id
        // field existed must still load. The serde default mints a fresh
        // Uuid::new_v4() per missing entry. Two such loads must produce
        // distinct ids (so an old file that contains two extras does not
        // collapse to a single id) and neither may be nil.
        let a: WindowState = serde_json::from_str("{}").unwrap();
        let b: WindowState = serde_json::from_str("{}").unwrap();
        assert_ne!(a.id, b.id);
        assert_ne!(a.id, Uuid::nil());
    }

    #[test]
    fn window_state_deserializes_from_empty_object() {
        // Any missing field must default — schema invariant: a window always
        // loads, even from minimal/corrupt input. Bootstrap path relies on
        // this when an old workspace.json has no per-window section.
        let s: WindowState = serde_json::from_str("{}").unwrap();
        assert!(s.hidden_project_ids.is_empty());
        assert!(s.folder_filter.is_none());
        assert!(s.project_widths.is_empty());
        assert!(s.folder_collapsed.is_empty());
        assert!(s.os_bounds.is_none());
        assert_eq!(s.sidebar_open, None);
        assert_eq!(s.project_layout, ProjectLayoutMode::Columns);
        assert_eq!(s.project_sort_mode, ProjectSortMode::Manual);
    }
}
