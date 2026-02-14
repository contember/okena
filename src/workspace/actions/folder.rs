//! Folder management workspace actions
//!
//! Actions for creating, modifying, and deleting sidebar folders.

use crate::theme::FolderColor;
use crate::workspace::state::{FolderData, Workspace};
use gpui::*;

impl Workspace {
    /// Create a new folder, appending it to project_order
    pub fn create_folder(&mut self, name: String, cx: &mut Context<Self>) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        self.data.folders.push(FolderData {
            id: id.clone(),
            name,
            project_ids: Vec::new(),
            collapsed: false,
            folder_color: FolderColor::default(),
        });
        self.data.project_order.push(id.clone());
        self.notify_data(cx);
        id
    }

    /// Delete a folder, splicing its contained projects back into project_order at the folder's position
    pub fn delete_folder(&mut self, folder_id: &str, cx: &mut Context<Self>) {
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

    /// Toggle folder collapsed state
    pub fn toggle_folder_collapsed(&mut self, folder_id: &str, cx: &mut Context<Self>) {
        if let Some(folder) = self.folder_mut(folder_id) {
            folder.collapsed = !folder.collapsed;
            self.notify_data(cx);
        }
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
    use crate::workspace::state::*;
    use crate::workspace::settings::HooksConfig;
    use crate::theme::FolderColor;
    use std::collections::HashMap;

    fn make_project(id: &str) -> ProjectData {
        ProjectData {
            id: id.to_string(),
            name: format!("Project {}", id),
            path: "/tmp/test".to_string(),
            is_visible: true,
            layout: Some(LayoutNode::new_terminal()),
            terminal_names: HashMap::new(),
            hidden_terminals: HashMap::new(),
            worktree_info: None,
            folder_color: FolderColor::default(),
            hooks: HooksConfig::default(),
            is_remote: false,
            connection_id: None,
        }
    }

    fn make_workspace_data(projects: Vec<ProjectData>, order: Vec<&str>) -> WorkspaceData {
        WorkspaceData {
            version: 1,
            projects,
            project_order: order.into_iter().map(String::from).collect(),
            project_widths: HashMap::new(),
            folders: vec![],
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
            collapsed: false,
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
            collapsed: false,
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
    use crate::workspace::state::{FolderData, LayoutNode, ProjectData, Workspace, WorkspaceData};
    use crate::workspace::settings::HooksConfig;
    use crate::theme::FolderColor;
    use std::collections::HashMap;

    fn make_project(id: &str) -> ProjectData {
        ProjectData {
            id: id.to_string(),
            name: format!("Project {}", id),
            path: "/tmp/test".to_string(),
            is_visible: true,
            layout: Some(LayoutNode::new_terminal()),
            terminal_names: HashMap::new(),
            hidden_terminals: HashMap::new(),
            worktree_info: None,
            folder_color: FolderColor::default(),
            hooks: HooksConfig::default(),
            is_remote: false,
            connection_id: None,
        }
    }

    fn make_workspace_data(projects: Vec<ProjectData>, order: Vec<&str>) -> WorkspaceData {
        WorkspaceData {
            version: 1,
            projects,
            project_order: order.into_iter().map(String::from).collect(),
            project_widths: HashMap::new(),
            folders: vec![],
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
            collapsed: false,
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
            collapsed: false,
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

}
