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
