//! Per-window viewport state.
//!
//! A `WindowState` is the filter/UI state for one window onto the shared
//! workspace: which projects are hidden in this window, the active folder
//! filter, per-project column widths, sidebar folder-collapse map, and OS
//! window bounds. Pure data — see PRD `plans/multi-window.md`.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// Restore bounds for an OS window: origin + size in screen pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WindowBounds {
    pub origin_x: f32,
    pub origin_y: f32,
    pub width: f32,
    pub height: f32,
}

/// Per-window viewport state. One instance per open window (main + extras).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct WindowState {
    /// Project IDs hidden in this window's grid.
    #[serde(default)]
    pub hidden_project_ids: HashSet<String>,
    /// Folder filter (folder ID) limiting visible projects in this window.
    #[serde(default)]
    pub folder_filter: Option<String>,
    /// Project column widths (percentages) scoped to this window.
    #[serde(default)]
    pub project_widths: HashMap<String, f32>,
    /// Per-folder collapsed state in this window's sidebar.
    #[serde(default)]
    pub folder_collapsed: HashMap<String, bool>,
    /// Last-known OS window bounds (used to restore position on next launch).
    #[serde(default)]
    pub os_bounds: Option<WindowBounds>,
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
            hidden_project_ids: hidden,
            folder_filter: Some("folder-7".to_string()),
            project_widths: widths,
            folder_collapsed: collapsed,
            os_bounds: Some(WindowBounds {
                origin_x: 100.0,
                origin_y: 50.0,
                width: 1280.0,
                height: 800.0,
            }),
        };

        let json = serde_json::to_string(&original).unwrap();
        let reloaded: WindowState = serde_json::from_str(&json).unwrap();

        assert_eq!(reloaded.hidden_project_ids, original.hidden_project_ids);
        assert_eq!(reloaded.folder_filter, original.folder_filter);
        assert_eq!(reloaded.project_widths, original.project_widths);
        assert_eq!(reloaded.folder_collapsed, original.folder_collapsed);
        assert_eq!(reloaded.os_bounds, original.os_bounds);
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
    }
}
