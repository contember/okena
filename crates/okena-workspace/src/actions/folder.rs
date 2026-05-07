//! Folder management workspace actions
//!
//! Actions for creating, modifying, and deleting sidebar folders.

use okena_core::theme::FolderColor;
use crate::state::{FolderData, WindowId, Workspace};
use gpui::*;

impl Workspace {
    /// Create a new folder, appending it to project_order
    pub fn create_folder(&mut self, name: String, cx: &mut Context<Self>) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        self.data.folders.push(FolderData {
            id: id.clone(),
            name,
            project_ids: Vec::new(),
            folder_color: FolderColor::default(),
        });
        self.data.project_order.push(id.clone());
        self.notify_data(cx);
        id
    }

    /// Delete a folder, splicing its contained projects back into project_order at the folder's position
    pub fn delete_folder(&mut self, folder_id: &str, cx: &mut Context<Self>) {
        // Clear folder filter on main if the deleted folder was its active
        // filter. Folder delete is a workspace-level event (no window_id
        // parameter on this action), so the scrub stays main-only here. A
        // future helper analogous to `delete_project_scrub_all_windows`
        // would fan the clear out to every window's folder_filter; until
        // then, an extra holding the deleted folder's id as its filter
        // surfaces an empty viewport instead of an unfiltered grid -- the
        // visibility computation already treats a stale folder id as "no
        // matches", so the UI still degrades gracefully.
        if self.active_folder_filter(WindowId::Main).map(|s| s.as_str()) == Some(folder_id) {
            self.data.main_window.folder_filter = None;
        }

        let project_ids = self.data.folders.iter()
            .find(|f| f.id == folder_id)
            .map(|f| f.project_ids.clone())
            .unwrap_or_default();

        // Find folder position in project_order
        if let Some(pos) = self.data.project_order.iter().position(|id| id == folder_id) {
            self.data.project_order.remove(pos);
            // Insert contained projects at the folder's old position
            for (i, pid) in project_ids.into_iter().enumerate() {
                self.data.project_order.insert(pos + i, pid);
            }
        }

        self.data.folders.retain(|f| f.id != folder_id);
        // Scrub the folder's entry from the per-window collapsed map so a
        // re-added folder with the same id does not inherit stale state.
        self.data.main_window.folder_collapsed.remove(folder_id);
        self.notify_data(cx);
    }

    /// Rename a folder
    pub fn rename_folder(&mut self, folder_id: &str, new_name: String, cx: &mut Context<Self>) {
        if let Some(folder) = self.folder_mut(folder_id) {
            folder.name = new_name;
            self.notify_data(cx);
        }
    }

    /// Set the color for a folder
    pub fn set_folder_item_color(&mut self, folder_id: &str, color: FolderColor, cx: &mut Context<Self>) {
        if let Some(folder) = self.folder_mut(folder_id) {
            folder.folder_color = color;
            self.notify_data(cx);
        }
    }

    /// Returns whether a folder is collapsed in the targeted window.
    ///
    /// Routes through `data.window(window_id)` (the lookup pair on
    /// `WorkspaceData`): `WindowId::Main` always returns the main slot,
    /// `WindowId::Extra(uuid)` walks `extra_windows`. An unknown extra
    /// (e.g. a paint racing a close) yields `None` and falls back to the
    /// "absence == expanded" default of `false` -- the same default used
    /// when the targeted window has no entry for the folder. Mirrors the
    /// silent-no-op shape of the window-scoped setters.
    pub fn is_folder_collapsed(&self, window_id: WindowId, folder_id: &str) -> bool {
        self.data
            .window(window_id)
            .and_then(|w| w.folder_collapsed.get(folder_id).copied())
            .unwrap_or(false)
    }

    /// Toggle folder collapsed state in the targeted window.
    ///
    /// Reads the current state via `is_folder_collapsed(window_id, ...)` and
    /// writes the inverted bool via `set_folder_collapsed(window_id, ...)`.
    /// The "absence == expanded" runtime convention is inherited from
    /// `set_folder_collapsed` (collapsing inserts `(folder_id, true)`,
    /// expanding removes the entry rather than storing `false`).
    ///
    /// Project-existence guard at the top: if `folder_id` is not in the
    /// shared `data.folders` list, the toggle is a silent no-op. The guard
    /// prevents stale ids from a sidebar context-menu race from inserting
    /// tombstone entries into the per-window map. The guard reads the
    /// shared list (not a per-window thing), so it stays in this wrapper
    /// rather than moving to the data layer.
    ///
    /// Unknown extra ids inherit the silent no-op contract from
    /// `set_folder_collapsed`.
    pub fn toggle_folder_collapsed(&mut self, window_id: WindowId, folder_id: &str, cx: &mut Context<Self>) {
        if !self.data.folders.iter().any(|f| f.id == folder_id) {
            return;
        }
        let now_collapsed = !self.is_folder_collapsed(window_id, folder_id);
        self.set_folder_collapsed(window_id, folder_id, now_collapsed, cx);
    }

    /// Move a project into a folder at a given position
    pub fn move_project_to_folder(&mut self, project_id: &str, folder_id: &str, position: Option<usize>, cx: &mut Context<Self>) {
        // Remove from any current folder
        for folder in &mut self.data.folders {
            folder.project_ids.retain(|id| id != project_id);
        }
        // Remove from top-level project_order
        self.data.project_order.retain(|id| id != project_id);

        // Add to target folder
        if let Some(folder) = self.folder_mut(folder_id) {
            let pos = position.unwrap_or(folder.project_ids.len());
            let pos = pos.min(folder.project_ids.len());
            folder.project_ids.insert(pos, project_id.to_string());
            self.notify_data(cx);
        }
    }

    /// Move a project out of its folder into the top-level project_order
    #[allow(dead_code)]
    pub fn move_project_out_of_folder(&mut self, project_id: &str, top_level_index: usize, cx: &mut Context<Self>) {
        // Remove from any folder
        for folder in &mut self.data.folders {
            folder.project_ids.retain(|id| id != project_id);
        }
        // Remove from project_order if already there (shouldn't be, but be safe)
        self.data.project_order.retain(|id| id != project_id);

        let target = top_level_index.min(self.data.project_order.len());
        self.data.project_order.insert(target, project_id.to_string());
        self.notify_data(cx);
    }

    /// Reorder a project within a folder
    #[allow(dead_code)]
    pub fn reorder_project_in_folder(&mut self, folder_id: &str, project_id: &str, new_index: usize, cx: &mut Context<Self>) {
        if let Some(folder) = self.folder_mut(folder_id) {
            if let Some(current) = folder.project_ids.iter().position(|id| id == project_id) {
                let id = folder.project_ids.remove(current);
                let target = if new_index > current {
                    new_index.saturating_sub(1)
                } else {
                    new_index
                };
                let target = target.min(folder.project_ids.len());
                folder.project_ids.insert(target, id);
                self.notify_data(cx);
            }
        }
    }

    /// Reorder any top-level item (project or folder) in project_order
    pub fn move_item_in_order(&mut self, item_id: &str, new_index: usize, cx: &mut Context<Self>) {
        if let Some(current) = self.data.project_order.iter().position(|id| id == item_id) {
            let id = self.data.project_order.remove(current);
            let target = if new_index > current {
                new_index.saturating_sub(1)
            } else {
                new_index
            };
            let target = target.min(self.data.project_order.len());
            self.data.project_order.insert(target, id);
            self.notify_data(cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::state::*;
    use crate::settings::HooksConfig;
    use okena_core::theme::FolderColor;
    use std::collections::HashMap;

    fn make_project(id: &str) -> ProjectData {
        ProjectData {
            id: id.to_string(),
            name: format!("Project {}", id),
            path: "/tmp/test".to_string(),
            layout: Some(LayoutNode::new_terminal()),
            terminal_names: HashMap::new(),
            hidden_terminals: HashMap::new(),
            worktree_info: None,
            worktree_ids: Vec::new(),
            folder_color: FolderColor::default(),
            hooks: HooksConfig::default(),
            is_remote: false,
            connection_id: None,
            service_terminals: HashMap::new(),
            default_shell: None,
            hook_terminals: HashMap::new(),
        }
    }

    fn make_workspace_data(projects: Vec<ProjectData>, order: Vec<&str>) -> WorkspaceData {
        WorkspaceData {
            version: 1,
            projects,
            project_order: order.into_iter().map(String::from).collect(),
            service_panel_heights: HashMap::new(),
            hook_panel_heights: HashMap::new(),
            folders: vec![],
            main_window: crate::state::WindowState::default(),
            extra_windows: Vec::new(),
        }
    }

    /// Simulate delete_folder: splice projects back into project_order
    fn simulate_delete_folder(data: &mut WorkspaceData, folder_id: &str) {
        let project_ids = data.folders.iter()
            .find(|f| f.id == folder_id)
            .map(|f| f.project_ids.clone())
            .unwrap_or_default();

        if let Some(pos) = data.project_order.iter().position(|id| id == folder_id) {
            data.project_order.remove(pos);
            for (i, pid) in project_ids.into_iter().enumerate() {
                data.project_order.insert(pos + i, pid);
            }
        }
        data.folders.retain(|f| f.id != folder_id);
    }

    /// Simulate move_project_to_folder
    fn simulate_move_to_folder(data: &mut WorkspaceData, project_id: &str, folder_id: &str, position: Option<usize>) {
        for folder in &mut data.folders {
            folder.project_ids.retain(|id| id != project_id);
        }
        data.project_order.retain(|id| id != project_id);

        if let Some(folder) = data.folders.iter_mut().find(|f| f.id == folder_id) {
            let pos = position.unwrap_or(folder.project_ids.len());
            let pos = pos.min(folder.project_ids.len());
            folder.project_ids.insert(pos, project_id.to_string());
        }
    }

    #[test]
    fn test_delete_folder_preserves_project_order_around_folder() {
        let mut data = make_workspace_data(
            vec![make_project("p1"), make_project("p2"), make_project("p3")],
            vec!["p1", "f1", "p3"],
        );
        data.folders = vec![FolderData {
            id: "f1".to_string(),
            name: "Folder".to_string(),
            project_ids: vec!["p2".to_string()],
            folder_color: FolderColor::default(),
        }];

        simulate_delete_folder(&mut data, "f1");
        // p2 should be inserted where f1 was (between p1 and p3)
        assert_eq!(data.project_order, vec!["p1", "p2", "p3"]);
    }

    #[test]
    fn test_move_project_to_folder_at_position() {
        let mut data = make_workspace_data(
            vec![make_project("p1"), make_project("p2"), make_project("p3")],
            vec!["f1", "p2", "p3"],
        );
        data.folders = vec![FolderData {
            id: "f1".to_string(),
            name: "Folder".to_string(),
            project_ids: vec!["p1".to_string()],
            folder_color: FolderColor::default(),
        }];

        // Move p2 to folder at position 0 (before p1)
        simulate_move_to_folder(&mut data, "p2", "f1", Some(0));

        assert_eq!(data.folders[0].project_ids, vec!["p2", "p1"]);
        assert!(!data.project_order.contains(&"p2".to_string()));
    }
}

#[cfg(test)]
mod gpui_tests {
    use gpui::AppContext as _;
    use crate::state::{FolderData, LayoutNode, ProjectData, WindowId, WindowState, Workspace, WorkspaceData};
    use crate::settings::HooksConfig;
    use okena_core::theme::FolderColor;
    use std::collections::HashMap;

    fn make_project(id: &str) -> ProjectData {
        ProjectData {
            id: id.to_string(),
            name: format!("Project {}", id),
            path: "/tmp/test".to_string(),
            layout: Some(LayoutNode::new_terminal()),
            terminal_names: HashMap::new(),
            hidden_terminals: HashMap::new(),
            worktree_info: None,
            worktree_ids: Vec::new(),
            folder_color: FolderColor::default(),
            hooks: HooksConfig::default(),
            is_remote: false,
            connection_id: None,
            service_terminals: HashMap::new(),
            default_shell: None,
            hook_terminals: HashMap::new(),
        }
    }

    fn make_workspace_data(projects: Vec<ProjectData>, order: Vec<&str>) -> WorkspaceData {
        WorkspaceData {
            version: 1,
            projects,
            project_order: order.into_iter().map(String::from).collect(),
            service_panel_heights: HashMap::new(),
            hook_panel_heights: HashMap::new(),
            folders: vec![],
            main_window: crate::state::WindowState::default(),
            extra_windows: Vec::new(),
        }
    }

    #[gpui::test]
    fn test_create_folder_gpui(cx: &mut gpui::TestAppContext) {
        let data = make_workspace_data(vec![], vec![]);
        let workspace = cx.new(|_cx| Workspace::new(data));

        let folder_id = workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.create_folder("My Folder".to_string(), cx)
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert_eq!(ws.data().folders.len(), 1);
            assert_eq!(ws.data().folders[0].name, "My Folder");
            assert_eq!(ws.data().folders[0].id, folder_id);
            assert!(ws.data().project_order.contains(&folder_id));
        });
    }

    #[gpui::test]
    fn test_delete_folder_gpui(cx: &mut gpui::TestAppContext) {
        let mut data = make_workspace_data(
            vec![make_project("p1"), make_project("p2")],
            vec!["f1"],
        );
        data.folders = vec![FolderData {
            id: "f1".to_string(),
            name: "Folder".to_string(),
            project_ids: vec!["p1".to_string(), "p2".to_string()],
            folder_color: FolderColor::default(),
        }];
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.delete_folder("f1", cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert!(ws.data().folders.is_empty());
            assert_eq!(ws.data().project_order, vec!["p1", "p2"]);
        });
    }

    #[gpui::test]
    fn test_move_project_to_folder_gpui(cx: &mut gpui::TestAppContext) {
        let mut data = make_workspace_data(
            vec![make_project("p1"), make_project("p2")],
            vec!["f1", "p1", "p2"],
        );
        data.folders = vec![FolderData {
            id: "f1".to_string(),
            name: "Folder".to_string(),
            project_ids: vec![],
            folder_color: FolderColor::default(),
        }];
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.move_project_to_folder("p1", "f1", None, cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert!(!ws.data().project_order.contains(&"p1".to_string()));
            assert_eq!(ws.data().folders[0].project_ids, vec!["p1".to_string()]);
        });
    }

    #[gpui::test]
    fn test_folder_filter_cleared_on_delete(cx: &mut gpui::TestAppContext) {
        let mut data = make_workspace_data(
            vec![make_project("p1"), make_project("p2")],
            vec!["f1", "f2"],
        );
        data.folders = vec![
            FolderData {
                id: "f1".to_string(),
                name: "Folder 1".to_string(),
                project_ids: vec!["p1".to_string()],
                    folder_color: FolderColor::default(),
            },
            FolderData {
                id: "f2".to_string(),
                name: "Folder 2".to_string(),
                project_ids: vec!["p2".to_string()],
                    folder_color: FolderColor::default(),
            },
        ];
        let workspace = cx.new(|_cx| Workspace::new(data));

        // Set folder filter to f1
        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.set_folder_filter(WindowId::Main, Some("f1".to_string()), cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert_eq!(ws.active_folder_filter(WindowId::Main), Some(&"f1".to_string()));
        });

        // Delete f1 — filter should auto-clear
        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.delete_folder("f1", cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert!(ws.active_folder_filter(WindowId::Main).is_none());
        });
    }

    #[gpui::test]
    fn is_folder_collapsed_reads_from_main_window_folder_collapsed(cx: &mut gpui::TestAppContext) {
        // Per-window viewport model: WindowId::Main routes through
        // data.window(...) and reads main_window.folder_collapsed (the new
        // source of truth). A future regression that re-routes the read back
        // to FolderData.collapsed should fail loudly: this fixture populates
        // ONLY main_window and leaves FolderData.collapsed=false.
        let mut data = make_workspace_data(vec![], vec!["f1"]);
        data.folders = vec![FolderData {
            id: "f1".to_string(),
            name: "Folder".to_string(),
            project_ids: vec![],
            folder_color: FolderColor::default(),
        }];
        data.main_window.folder_collapsed.insert("f1".to_string(), true);
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert!(ws.is_folder_collapsed(WindowId::Main, "f1"));
            // Missing entry defaults to expanded.
            assert!(!ws.is_folder_collapsed(WindowId::Main, "missing"));
        });
    }

    #[gpui::test]
    fn is_folder_collapsed_extra_reads_from_targeted_window(cx: &mut gpui::TestAppContext) {
        // Per-window viewport model: WindowId::Extra(uuid) routes through
        // data.window(...) and reads the matching extra's folder_collapsed --
        // not main's. Fixture writes (f1, true) only on the extra; main's map
        // is empty. Reading with the extra id returns true; reading with Main
        // returns false (absence == expanded). Defends against a regression
        // that ignores window_id and unconditionally reads main, which would
        // silently break the per-window sidebar contract once extras land in
        // slice 05.
        let mut data = make_workspace_data(vec![], vec!["f1"]);
        data.folders = vec![FolderData {
            id: "f1".to_string(),
            name: "Folder".to_string(),
            project_ids: vec![],
            folder_color: FolderColor::default(),
        }];
        let mut extra = WindowState::default();
        extra.folder_collapsed.insert("f1".to_string(), true);
        let extra_id = extra.id;
        data.extra_windows.push(extra);
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert!(ws.is_folder_collapsed(WindowId::Extra(extra_id), "f1"));
            // Main has no entry for f1 -> absence == expanded.
            assert!(!ws.is_folder_collapsed(WindowId::Main, "f1"));
        });
    }

    #[gpui::test]
    fn is_folder_collapsed_unknown_extra_returns_default(cx: &mut gpui::TestAppContext) {
        // Close-race contract: a fresh uuid that does not match any extra is
        // a `data.window(...) == None`, which falls back to the "absence ==
        // expanded" default rather than panicking. Pre-populate main with
        // (f1, true) to ensure the unknown-extra path does NOT silently fall
        // back to main as a default -- a window-cross-contamination bug
        // would surface here.
        let mut data = make_workspace_data(vec![], vec!["f1"]);
        data.folders = vec![FolderData {
            id: "f1".to_string(),
            name: "Folder".to_string(),
            project_ids: vec![],
            folder_color: FolderColor::default(),
        }];
        data.main_window.folder_collapsed.insert("f1".to_string(), true);
        let workspace = cx.new(|_cx| Workspace::new(data));

        let unknown = uuid::Uuid::new_v4();
        workspace.read_with(cx, |ws: &Workspace, _cx| {
            // Unknown extra -> false, NOT main's true.
            assert!(!ws.is_folder_collapsed(WindowId::Extra(unknown), "f1"));
        });
    }

    #[gpui::test]
    fn toggle_folder_collapsed_writes_to_main_window(cx: &mut gpui::TestAppContext) {
        // Toggling on WindowId::Main flips main_window.folder_collapsed via the
        // window-scoped delegate. The "absence == expanded" runtime convention
        // is inherited from set_folder_collapsed: collapsing inserts true,
        // expanding removes the entry rather than storing false.
        let mut data = make_workspace_data(vec![], vec!["f1"]);
        data.folders = vec![FolderData {
            id: "f1".to_string(),
            name: "Folder".to_string(),
            project_ids: vec![],
            folder_color: FolderColor::default(),
        }];
        let workspace = cx.new(|_cx| Workspace::new(data));

        // First toggle: false -> true. main_window inserts true.
        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.toggle_folder_collapsed(WindowId::Main, "f1", cx);
        });
        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert_eq!(ws.data().main_window.folder_collapsed.get("f1"), Some(&true));
        });

        // Second toggle: true -> false. main_window removes the entry
        // (absence == expanded).
        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.toggle_folder_collapsed(WindowId::Main, "f1", cx);
        });
        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert!(!ws.data().main_window.folder_collapsed.contains_key("f1"));
        });
    }

    #[gpui::test]
    fn toggle_folder_collapsed_extra_writes_only_to_targeted_window(cx: &mut gpui::TestAppContext) {
        // Per-window viewport model: WindowId::Extra(uuid) routes through
        // set_folder_collapsed targeting the matching extra -- not main and
        // not the sibling extra. Defends against a regression that ignores
        // window_id and unconditionally writes to main, scatters the write
        // across all extras, or routes the read through main while the write
        // hits the extra (a subtle bug that would surface as a folder
        // collapsing on every other click instead of every click, since the
        // read leg of the toggle would always see main's state).
        let mut data = make_workspace_data(vec![], vec!["f1"]);
        data.folders = vec![FolderData {
            id: "f1".to_string(),
            name: "Folder".to_string(),
            project_ids: vec![],
            folder_color: FolderColor::default(),
        }];
        let extra_a = WindowState::default();
        let extra_a_id = extra_a.id;
        let extra_b = WindowState::default();
        let extra_b_id = extra_b.id;
        data.extra_windows.push(extra_a);
        data.extra_windows.push(extra_b);
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.toggle_folder_collapsed(WindowId::Extra(extra_a_id), "f1", cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            let extras = &ws.data().extra_windows;
            let a = extras.iter().find(|w| w.id == extra_a_id).unwrap();
            let b = extras.iter().find(|w| w.id == extra_b_id).unwrap();
            assert_eq!(a.folder_collapsed.get("f1"), Some(&true));
            assert!(!b.folder_collapsed.contains_key("f1"));
            assert!(!ws.data().main_window.folder_collapsed.contains_key("f1"));
        });
    }

    #[gpui::test]
    fn toggle_folder_collapsed_unknown_id_is_noop(cx: &mut gpui::TestAppContext) {
        // Project-existence guard: a folder id not in data.folders is a
        // silent no-op -- no entry inserted, no data_version bump. Pins the
        // guard against a future refactor that drops it "for symmetry with
        // the data-layer setter" (the data-layer setter has no such guard
        // because it is the general-purpose window-scoped writer; the guard
        // belongs at this wrapper because it reads from the shared folder
        // list).
        let data = make_workspace_data(vec![], vec![]);
        // No folders -- "stale" is unknown.
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.toggle_folder_collapsed(WindowId::Main, "stale", cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert!(ws.data().main_window.folder_collapsed.is_empty());
        });
    }

    #[gpui::test]
    fn delete_folder_clears_main_window_folder_collapsed(cx: &mut gpui::TestAppContext) {
        // Deleting a folder must scrub its entry from main_window.folder_collapsed
        // (the new source of truth). Without the scrub, a re-added folder with
        // the same id would inherit the deleted folder's collapsed state on
        // the next render.
        let mut data = make_workspace_data(
            vec![make_project("p1")],
            vec!["f1"],
        );
        data.folders = vec![FolderData {
            id: "f1".to_string(),
            name: "Folder".to_string(),
            project_ids: vec!["p1".to_string()],
            folder_color: FolderColor::default(),
        }];
        data.main_window.folder_collapsed.insert("f1".to_string(), true);
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.delete_folder("f1", cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert!(!ws.data().main_window.folder_collapsed.contains_key("f1"));
        });
    }

    #[gpui::test]
    fn test_folder_filter_not_cleared_on_other_folder_delete(cx: &mut gpui::TestAppContext) {
        let mut data = make_workspace_data(
            vec![make_project("p1"), make_project("p2")],
            vec!["f1", "f2"],
        );
        data.folders = vec![
            FolderData {
                id: "f1".to_string(),
                name: "Folder 1".to_string(),
                project_ids: vec!["p1".to_string()],
                    folder_color: FolderColor::default(),
            },
            FolderData {
                id: "f2".to_string(),
                name: "Folder 2".to_string(),
                project_ids: vec!["p2".to_string()],
                    folder_color: FolderColor::default(),
            },
        ];
        let workspace = cx.new(|_cx| Workspace::new(data));

        // Set folder filter to f1
        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.set_folder_filter(WindowId::Main, Some("f1".to_string()), cx);
        });

        // Delete f2 — filter should remain on f1
        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.delete_folder("f2", cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert_eq!(ws.active_folder_filter(WindowId::Main), Some(&"f1".to_string()));
        });
    }
}
