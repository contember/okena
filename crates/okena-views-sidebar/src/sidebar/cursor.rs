//! Keyboard cursor navigation: flat item list, cursor movement, action
//! handlers for `SidebarUp`/`Down`/`Confirm`/`ToggleExpand`/`Escape`.

use super::{GroupKind, Sidebar, SidebarCursorItem};
use crate::{SidebarConfirm, SidebarDown, SidebarEscape, SidebarToggleExpand, SidebarUp};
use gpui::*;
use okena_workspace::state::ProjectData;
use std::collections::{HashMap, HashSet};

impl Sidebar {
    /// Initialize cursor to the focused project or first item
    pub fn activate_cursor(&mut self, cx: &mut Context<Self>) {
        let items = self.build_cursor_items(cx);
        if items.is_empty() {
            self.cursor_index = None;
            return;
        }
        // Try to place cursor on the focused project
        let focused_id = self.workspace.read(cx).focused_project_id().cloned();
        if let Some(ref focused_id) = focused_id {
            if let Some(pos) = items.iter().position(|item| match item {
                SidebarCursorItem::Project { project_id } |
                SidebarCursorItem::WorktreeProject { project_id } => project_id == focused_id,
                _ => false,
            }) {
                self.cursor_index = Some(pos);
                cx.notify();
                return;
            }
        }
        self.cursor_index = Some(0);
        cx.notify();
    }

    /// Build a flat list of cursor items matching the visual render order
    pub(super) fn build_cursor_items(&self, cx: &mut Context<Self>) -> Vec<SidebarCursorItem> {
        let workspace = self.workspace.read(cx);
        let all_projects: HashMap<&str, &ProjectData> = workspace.data().projects.iter()
            .map(|p| (p.id.as_str(), p))
            .collect();
        let all_project_ids: HashSet<&str> = workspace.data().projects.iter()
            .map(|p| p.id.as_str()).collect();

        // Pre-collect service names per project (avoids borrow issues with cx)
        let service_names: HashMap<String, Vec<String>> = if let Some(ref sm) = self.service_manager {
            let sm = sm.read(cx);
            workspace.data().projects.iter()
                .filter(|p| sm.has_services(&p.id))
                .map(|p| {
                    let names = sm.services_for_project(&p.id)
                        .into_iter()
                        .map(|inst| inst.definition.name.clone())
                        .collect();
                    (p.id.clone(), names)
                })
                .collect()
        } else {
            HashMap::new()
        };

        // Pre-collect hook terminal IDs per project
        let hook_terminal_ids: HashMap<String, Vec<String>> = workspace.data().projects.iter()
            .filter(|p| !p.hook_terminals.is_empty())
            .map(|p| {
                let ids = p.hook_terminals.keys().cloned().collect();
                (p.id.clone(), ids)
            })
            .collect();

        // Build worktree children map
        let mut worktree_children_map: HashMap<String, Vec<&ProjectData>> = HashMap::new();
        for parent in &workspace.data().projects {
            if !parent.worktree_ids.is_empty() {
                let mut children = Vec::new();
                for wt_id in &parent.worktree_ids {
                    if let Some(&child) = all_projects.get(wt_id.as_str()) {
                        children.push(child);
                    }
                }
                if !children.is_empty() {
                    worktree_children_map.insert(parent.id.clone(), children);
                }
            }
        }

        let mut cursor_items = Vec::new();

        for id in &workspace.data().project_order {
            // Check if this is a folder
            if let Some(folder) = workspace.data().folders.iter().find(|f| &f.id == id) {
                cursor_items.push(SidebarCursorItem::Folder { folder_id: folder.id.clone() });

                if !workspace.is_folder_collapsed(self.window_id, &folder.id) {
                    for pid in &folder.project_ids {
                        if let Some(&project) = all_projects.get(pid.as_str()) {
                            // Skip worktree children that have a parent in the project list
                            if project.worktree_info.as_ref().map_or(false, |w| {
                                all_project_ids.contains(w.parent_project_id.as_str())
                            }) {
                                continue;
                            }
                            self.push_project_cursor_items(project, &worktree_children_map, &service_names, &hook_terminal_ids, &mut cursor_items);
                        }
                    }
                }
                continue;
            }

            // Top-level project (not a worktree child of another)
            if let Some(&project) = all_projects.get(id.as_str()) {
                if project.worktree_info.as_ref().map_or(false, |w| {
                    all_project_ids.contains(w.parent_project_id.as_str())
                }) {
                    continue;
                }
                self.push_project_cursor_items(project, &worktree_children_map, &service_names, &hook_terminal_ids, &mut cursor_items);
            }
        }

        cursor_items
    }

    /// Helper: push a project row + its expanded terminals/services + worktree children into cursor items
    fn push_project_cursor_items(
        &self,
        project: &ProjectData,
        worktree_children_map: &HashMap<String, Vec<&ProjectData>>,
        service_names: &HashMap<String, Vec<String>>,
        hook_terminal_ids: &HashMap<String, Vec<String>>,
        cursor_items: &mut Vec<SidebarCursorItem>,
    ) {
        let has_worktrees = worktree_children_map.get(&project.id).map_or(false, |c| !c.is_empty());
        let is_orphan = project.worktree_info.is_some();

        if has_worktrees && !is_orphan {
            // Group header mode: Project = group header, WorktreeProject = main project child
            cursor_items.push(SidebarCursorItem::Project { project_id: project.id.clone() });

            let is_expanded = self.is_project_expanded(&project.id, true);
            if is_expanded {
                // Main project as first child
                cursor_items.push(SidebarCursorItem::WorktreeProject { project_id: project.id.clone() });

                if self.expanded_projects.contains(&project.id) {
                    self.push_group_cursor_items(&project.id, &project.layout, service_names, hook_terminal_ids, cursor_items);
                }

                // Worktree children as siblings
                if let Some(children) = worktree_children_map.get(&project.id) {
                    for child in children {
                        cursor_items.push(SidebarCursorItem::WorktreeProject { project_id: child.id.clone() });
                        if self.expanded_projects.contains(&child.id) {
                            self.push_group_cursor_items(&child.id, &child.layout, service_names, hook_terminal_ids, cursor_items);
                        }
                    }
                }
            }
        } else {
            // Standard mode: no worktrees
            cursor_items.push(SidebarCursorItem::Project { project_id: project.id.clone() });

            if self.expanded_projects.contains(&project.id) {
                self.push_group_cursor_items(&project.id, &project.layout, service_names, hook_terminal_ids, cursor_items);
            }
        }
    }

    /// Push group headers and their child cursor items for an expanded project.
    fn push_group_cursor_items(
        &self,
        project_id: &str,
        layout: &Option<okena_workspace::state::LayoutNode>,
        service_names: &HashMap<String, Vec<String>>,
        hook_terminal_ids: &HashMap<String, Vec<String>>,
        cursor_items: &mut Vec<SidebarCursorItem>,
    ) {
        // Terminals group
        if let Some(layout) = layout {
            let terminal_ids = layout.collect_terminal_ids();
            if !terminal_ids.is_empty() {
                cursor_items.push(SidebarCursorItem::GroupHeader {
                    project_id: project_id.to_string(),
                    group: GroupKind::Terminals,
                });

                if !self.is_group_collapsed(project_id, &GroupKind::Terminals) {
                    for tid in terminal_ids {
                        cursor_items.push(SidebarCursorItem::Terminal {
                            project_id: project_id.to_string(),
                            terminal_id: tid,
                        });
                    }
                }
            }
        }

        // Services group
        if let Some(names) = service_names.get(project_id) {
            if !names.is_empty() {
                cursor_items.push(SidebarCursorItem::GroupHeader {
                    project_id: project_id.to_string(),
                    group: GroupKind::Services,
                });

                if !self.is_group_collapsed(project_id, &GroupKind::Services) {
                    for name in names {
                        cursor_items.push(SidebarCursorItem::Service {
                            project_id: project_id.to_string(),
                            service_name: name.clone(),
                        });
                    }
                }
            }
        }

        // Hooks group
        if let Some(tids) = hook_terminal_ids.get(project_id) {
            if !tids.is_empty() {
                cursor_items.push(SidebarCursorItem::GroupHeader {
                    project_id: project_id.to_string(),
                    group: GroupKind::Hooks,
                });

                if !self.is_group_collapsed(project_id, &GroupKind::Hooks) {
                    for tid in tids {
                        cursor_items.push(SidebarCursorItem::Hook {
                            project_id: project_id.to_string(),
                            terminal_id: tid.clone(),
                        });
                    }
                }
            }
        }
    }

    /// Clamp cursor to valid range
    pub(super) fn validate_cursor(&mut self, item_count: usize) {
        if item_count == 0 {
            self.cursor_index = None;
        } else if let Some(ref mut idx) = self.cursor_index {
            if *idx >= item_count {
                *idx = item_count - 1;
            }
        }
    }

    /// Check if any rename is active (blocks keyboard nav)
    fn is_interactive_mode_active(&self) -> bool {
        self.terminal_rename.is_some()
            || self.project_rename.is_some()
            || self.folder_rename.is_some()
    }

    pub(super) fn handle_sidebar_up(&mut self, _: &SidebarUp, _window: &mut Window, cx: &mut Context<Self>) {
        if self.is_interactive_mode_active() { return; }
        let items = self.build_cursor_items(cx);
        if items.is_empty() { return; }
        match self.cursor_index {
            Some(idx) if idx > 0 => self.cursor_index = Some(idx - 1),
            None => self.cursor_index = Some(items.len() - 1),
            _ => {}
        }
        self.scroll_to_cursor(items.len());
        cx.notify();
    }

    pub(super) fn handle_sidebar_down(&mut self, _: &SidebarDown, _window: &mut Window, cx: &mut Context<Self>) {
        if self.is_interactive_mode_active() { return; }
        let items = self.build_cursor_items(cx);
        if items.is_empty() { return; }
        match self.cursor_index {
            Some(idx) if idx < items.len() - 1 => self.cursor_index = Some(idx + 1),
            None => self.cursor_index = Some(0),
            _ => {}
        }
        self.scroll_to_cursor(items.len());
        cx.notify();
    }

    pub(super) fn handle_sidebar_confirm(&mut self, _: &SidebarConfirm, window: &mut Window, cx: &mut Context<Self>) {
        if self.project_rename.is_some() {
            self.finish_project_rename(cx);
            return;
        }
        if self.folder_rename.is_some() {
            self.finish_folder_rename(cx);
            return;
        }
        if self.terminal_rename.is_some() {
            self.finish_rename(cx);
            return;
        }
        if self.is_interactive_mode_active() { return; }
        let items = self.build_cursor_items(cx);
        let Some(idx) = self.cursor_index else { return };
        let Some(item) = items.get(idx) else { return };

        match item.clone() {
            SidebarCursorItem::Project { project_id } => {
                // Project may be a group header (has worktrees) → non-individual focus
                let has_worktrees = !self.workspace.read(cx).worktree_child_ids(&project_id).is_empty();
                if has_worktrees {
                    self.workspace.update(cx, |ws, cx| {
                        ws.set_focused_project(Some(project_id.clone()), cx);
                    });
                } else {
                    self.workspace.update(cx, |ws, cx| {
                        ws.set_focused_project_individual(Some(project_id.clone()), cx);
                    });
                }
                self.cursor_index = None;
                if let Some(ref saved) = self.saved_focus {
                    window.focus(saved, cx);
                }
                self.saved_focus = None;
            }
            SidebarCursorItem::WorktreeProject { project_id } => {
                self.workspace.update(cx, |ws, cx| {
                    ws.set_focused_project_individual(Some(project_id.clone()), cx);
                });
                self.cursor_index = None;
                if let Some(ref saved) = self.saved_focus {
                    window.focus(saved, cx);
                }
                self.saved_focus = None;
            }
            SidebarCursorItem::Terminal { project_id, terminal_id } => {
                self.workspace.update(cx, |ws, cx| {
                    ws.focus_terminal_by_id(&project_id, &terminal_id, cx);
                });
                self.cursor_index = None;
                if let Some(ref saved) = self.saved_focus {
                    window.focus(saved, cx);
                }
                self.saved_focus = None;
            }
            SidebarCursorItem::Folder { folder_id } => {
                let window_id = self.window_id;
                self.workspace.update(cx, |ws, cx| {
                    ws.toggle_folder_collapsed(window_id, &folder_id, cx);
                });
            }
            SidebarCursorItem::GroupHeader { project_id, group } => {
                self.toggle_group(&project_id, group);
            }
            SidebarCursorItem::Service { project_id, service_name } => {
                // Toggle start/stop for the service
                if let Some(ref sm) = self.service_manager {
                    sm.update(cx, |sm, cx| {
                        let key = (project_id.clone(), service_name.clone());
                        if let Some(inst) = sm.instances().get(&key) {
                            match inst.status {
                                okena_services::manager::ServiceStatus::Running |
                                okena_services::manager::ServiceStatus::Starting => {
                                    sm.stop_service(&project_id, &service_name, cx);
                                }
                                _ => {
                                    if let Some(path) = sm.project_path(&project_id) {
                                        let path = path.clone();
                                        sm.start_service(&project_id, &service_name, &path, cx);
                                    }
                                }
                            }
                        }
                    });
                }
            }
            SidebarCursorItem::RemoteConnection { connection_id } => {
                let collapsed = self.collapsed_connections.get(&connection_id).copied().unwrap_or(false);
                self.collapsed_connections.insert(connection_id, !collapsed);
            }
            SidebarCursorItem::RemoteProject { project_id, .. } => {
                // Remote projects are now materialized in workspace, use unified focus
                self.workspace.update(cx, |ws, cx| {
                    ws.set_focused_project_individual(Some(project_id.clone()), cx);
                });
                self.cursor_index = None;
                if let Some(ref saved) = self.saved_focus {
                    window.focus(saved, cx);
                }
                self.saved_focus = None;
            }
            SidebarCursorItem::Hook { project_id, terminal_id } => {
                self.workspace.update(cx, |ws, cx| {
                    ws.focus_terminal_by_id(&project_id, &terminal_id, cx);
                });
                self.cursor_index = None;
                if let Some(ref saved) = self.saved_focus {
                    window.focus(saved, cx);
                }
                self.saved_focus = None;
            }
        }
        cx.notify();
    }

    pub(super) fn handle_sidebar_toggle_expand(&mut self, _: &SidebarToggleExpand, _window: &mut Window, cx: &mut Context<Self>) {
        if self.is_interactive_mode_active() { return; }
        let items = self.build_cursor_items(cx);
        let Some(idx) = self.cursor_index else { return };
        let Some(item) = items.get(idx) else { return };

        match item.clone() {
            SidebarCursorItem::Folder { folder_id } => {
                let window_id = self.window_id;
                self.workspace.update(cx, |ws, cx| {
                    ws.toggle_folder_collapsed(window_id, &folder_id, cx);
                });
            }
            SidebarCursorItem::Project { project_id } => {
                // Mirror mouse behavior: toggle worktree collapse for parent projects,
                // terminal details for projects without worktrees
                let has_worktrees = !self.workspace.read(cx)
                    .worktree_child_ids(&project_id).is_empty();
                if has_worktrees {
                    self.toggle_worktrees_collapsed(&project_id);
                } else {
                    self.toggle_expanded(&project_id);
                }
            }
            SidebarCursorItem::WorktreeProject { project_id } => {
                self.toggle_expanded(&project_id);
            }
            SidebarCursorItem::GroupHeader { project_id, group } => {
                self.toggle_group(&project_id, group);
            }
            SidebarCursorItem::Terminal { .. } | SidebarCursorItem::Service { .. } | SidebarCursorItem::Hook { .. } => {}
            SidebarCursorItem::RemoteConnection { connection_id } => {
                let collapsed = self.collapsed_connections.get(&connection_id).copied().unwrap_or(false);
                self.collapsed_connections.insert(connection_id, !collapsed);
            }
            SidebarCursorItem::RemoteProject { .. } => {}
        }
        cx.notify();
    }

    pub(super) fn handle_sidebar_escape(&mut self, _: &SidebarEscape, window: &mut Window, cx: &mut Context<Self>) {
        if self.project_rename.is_some() {
            self.cancel_project_rename(cx);
            return;
        }
        if self.folder_rename.is_some() {
            self.cancel_folder_rename(cx);
            return;
        }
        if self.terminal_rename.is_some() {
            self.cancel_rename(cx);
            return;
        }
        self.cursor_index = None;
        if let Some(ref saved) = self.saved_focus {
            window.focus(saved, cx);
        }
        self.saved_focus = None;
        cx.notify();
    }

    /// Scroll the sidebar to keep the cursor item visible
    fn scroll_to_cursor(&self, item_count: usize) {
        if let Some(idx) = self.cursor_index {
            if item_count > 0 {
                self.scroll_handle.scroll_to_item(idx);
            }
        }
    }
}
