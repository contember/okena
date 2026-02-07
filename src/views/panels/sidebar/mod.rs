//! Sidebar view with project and terminal list
//!
//! The sidebar provides navigation for projects and terminals, with features for:
//! - Adding/managing projects
//! - Renaming terminals and projects
//! - Drag-and-drop project reordering
//! - Folder color customization
//! - Organizing projects into collapsible folders

mod color_picker;
pub(super) mod drag;
mod folder_list;
mod item_widgets;
mod project_list;

use crate::keybindings::{
    format_keystroke, get_config, ShowKeybindings, SidebarConfirm, SidebarDown,
    SidebarEscape, SidebarToggleExpand, SidebarUp,
};
use crate::theme::{theme, FolderColor};
use crate::ui::ClickDetector;
use crate::views::components::{
    cancel_rename, finish_rename, start_rename_with_blur,
    RenameState,
};
use crate::views::root::TerminalsRegistry;
use crate::workspace::requests::SidebarRequest;
use crate::workspace::state::{FolderData, ProjectData, Workspace};
use gpui::*;
use gpui_component::h_flex;
use gpui::prelude::*;
use std::collections::{HashMap, HashSet};

use drag::{ProjectDrag, ProjectDragView, FolderDrag, FolderDragView};

/// Identifies each visible row in the sidebar for keyboard cursor navigation.
#[derive(Clone, Debug)]
pub(super) enum SidebarCursorItem {
    Folder { folder_id: String },
    Project { project_id: String },
    WorktreeProject { project_id: String },
    Terminal { project_id: String, terminal_id: String },
}

/// Sidebar view with project and terminal list
pub struct Sidebar {
    workspace: Entity<Workspace>,
    expanded_projects: HashSet<String>,
    pub(super) terminals: TerminalsRegistry,
    /// Terminal rename state: (project_id, terminal_id)
    pub(super) terminal_rename: Option<RenameState<(String, String)>>,
    /// Double-click detector for terminals
    terminal_click_detector: ClickDetector<String>,
    /// Project rename state
    pub(super) project_rename: Option<RenameState<String>>,
    /// Double-click detector for projects
    project_click_detector: ClickDetector<String>,
    /// Project ID for which color picker is shown
    color_picker_project_id: Option<String>,
    /// Folder rename state
    pub(super) folder_rename: Option<RenameState<String>>,
    /// Double-click detector for folders
    folder_click_detector: ClickDetector<String>,
    /// Folder ID for which color picker is shown
    color_picker_folder_id: Option<String>,
    /// Sidebar requests drained from Workspace by observer, applied in render() (needs Window)
    pending_sidebar_requests: Vec<SidebarRequest>,
    /// Focus handle for keyboard event capture
    focus_handle: FocusHandle,
    /// Scroll handle for programmatic scrolling
    scroll_handle: ScrollHandle,
    /// Current keyboard cursor position (index into flat item list)
    cursor_index: Option<usize>,
    /// Saved focus handle to restore when leaving sidebar
    pub saved_focus: Option<FocusHandle>,
}

impl Sidebar {
    pub fn new(workspace: Entity<Workspace>, terminals: TerminalsRegistry, cx: &mut Context<Self>) -> Self {
        // Observe Workspace to drain sidebar requests outside of render().
        // Requests are stored in pending_sidebar_requests and applied in render()
        // where Window access is available (needed for focus/rename).
        cx.observe(&workspace, |this, _workspace, cx| {
            if this.workspace.read(cx).sidebar_requests.is_empty() {
                return;
            }
            let requests: Vec<_> = this.workspace.update(cx, |ws, _cx| {
                ws.sidebar_requests.drain(..).collect()
            });
            this.pending_sidebar_requests.extend(requests);
            cx.notify();
        }).detach();

        Self {
            workspace,
            expanded_projects: HashSet::new(),
            terminals,
            terminal_rename: None,
            terminal_click_detector: ClickDetector::new(),
            project_rename: None,
            project_click_detector: ClickDetector::new(),
            color_picker_project_id: None,
            folder_rename: None,
            folder_click_detector: ClickDetector::new(),
            color_picker_folder_id: None,
            pending_sidebar_requests: Vec::new(),
            focus_handle: cx.focus_handle(),
            scroll_handle: ScrollHandle::new(),
            cursor_index: None,
            saved_focus: None,
        }
    }

    /// Check for double-click on terminal and return true if detected
    pub(super) fn check_double_click(&mut self, terminal_id: &str) -> bool {
        self.terminal_click_detector.check(terminal_id.to_string())
    }

    fn toggle_expanded(&mut self, project_id: &str) {
        if self.expanded_projects.contains(project_id) {
            self.expanded_projects.remove(project_id);
        } else {
            self.expanded_projects.insert(project_id.to_string());
        }
    }

    pub(super) fn start_rename(&mut self, project_id: String, terminal_id: String, current_name: String, window: &mut Window, cx: &mut Context<Self>) {
        self.terminal_rename = Some(start_rename_with_blur(
            (project_id, terminal_id),
            &current_name,
            "Terminal name...",
            |this, _window, cx| this.finish_rename(cx),
            window,
            cx,
        ));
        self.workspace.update(cx, |ws, cx| ws.clear_focused_terminal(cx));
        cx.notify();
    }

    pub(super) fn finish_rename(&mut self, cx: &mut Context<Self>) {
        if let Some(((project_id, terminal_id), new_name)) = finish_rename(&mut self.terminal_rename, cx) {
            self.workspace.update(cx, |ws, cx| {
                ws.rename_terminal(&project_id, &terminal_id, new_name, cx);
            });
        }
        self.workspace.update(cx, |ws, cx| ws.restore_focused_terminal(cx));
        cx.notify();
    }

    pub(super) fn cancel_rename(&mut self, cx: &mut Context<Self>) {
        cancel_rename(&mut self.terminal_rename);
        self.workspace.update(cx, |ws, cx| ws.restore_focused_terminal(cx));
        cx.notify();
    }

    /// Check for double-click on project and return true if detected
    pub(super) fn check_project_double_click(&mut self, project_id: &str) -> bool {
        self.project_click_detector.check(project_id.to_string())
    }

    pub(super) fn start_project_rename(&mut self, project_id: String, current_name: String, window: &mut Window, cx: &mut Context<Self>) {
        self.project_rename = Some(start_rename_with_blur(
            project_id,
            &current_name,
            "Project name...",
            |this, _window, cx| this.finish_project_rename(cx),
            window,
            cx,
        ));
        self.workspace.update(cx, |ws, cx| ws.clear_focused_terminal(cx));
        cx.notify();
    }

    pub(super) fn finish_project_rename(&mut self, cx: &mut Context<Self>) {
        if let Some((project_id, new_name)) = finish_rename(&mut self.project_rename, cx) {
            self.workspace.update(cx, |ws, cx| {
                ws.rename_project(&project_id, new_name, cx);
            });
        }
        self.workspace.update(cx, |ws, cx| ws.restore_focused_terminal(cx));
        cx.notify();
    }

    pub(super) fn cancel_project_rename(&mut self, cx: &mut Context<Self>) {
        cancel_rename(&mut self.project_rename);
        self.workspace.update(cx, |ws, cx| ws.restore_focused_terminal(cx));
        cx.notify();
    }

    pub(super) fn show_color_picker(&mut self, project_id: String, cx: &mut Context<Self>) {
        self.color_picker_project_id = Some(project_id);
        self.color_picker_folder_id = None;
        cx.notify();
    }

    pub(super) fn show_folder_color_picker(&mut self, folder_id: String, cx: &mut Context<Self>) {
        self.color_picker_folder_id = Some(folder_id);
        self.color_picker_project_id = None;
        cx.notify();
    }

    fn hide_color_picker(&mut self, cx: &mut Context<Self>) {
        self.color_picker_project_id = None;
        self.color_picker_folder_id = None;
        cx.notify();
    }

    pub(super) fn set_folder_color(&mut self, project_id: &str, color: FolderColor, cx: &mut Context<Self>) {
        self.workspace.update(cx, |ws, cx| {
            ws.set_folder_color(project_id, color, cx);
        });
        self.hide_color_picker(cx);
    }

    pub(super) fn set_folder_item_color(&mut self, folder_id: &str, color: FolderColor, cx: &mut Context<Self>) {
        self.workspace.update(cx, |ws, cx| {
            ws.set_folder_item_color(folder_id, color, cx);
        });
        self.hide_color_picker(cx);
    }

    fn request_context_menu(&mut self, project_id: String, position: Point<Pixels>, cx: &mut Context<Self>) {
        self.workspace.update(cx, |ws, cx| {
            ws.push_overlay_request(crate::workspace::requests::OverlayRequest::ContextMenu {
                project_id,
                position,
            }, cx);
        });
    }

    /// Check for double-click on folder and return true if detected
    pub(super) fn check_folder_double_click(&mut self, folder_id: &str) -> bool {
        self.folder_click_detector.check(folder_id.to_string())
    }

    pub(super) fn start_folder_rename(&mut self, folder_id: String, current_name: String, window: &mut Window, cx: &mut Context<Self>) {
        self.folder_rename = Some(start_rename_with_blur(
            folder_id,
            &current_name,
            "Folder name...",
            |this, _window, cx| this.finish_folder_rename(cx),
            window,
            cx,
        ));
        self.workspace.update(cx, |ws, cx| ws.clear_focused_terminal(cx));
        cx.notify();
    }

    pub(super) fn finish_folder_rename(&mut self, cx: &mut Context<Self>) {
        if let Some((folder_id, new_name)) = finish_rename(&mut self.folder_rename, cx) {
            self.workspace.update(cx, |ws, cx| {
                ws.rename_folder(&folder_id, new_name, cx);
            });
        }
        self.workspace.update(cx, |ws, cx| ws.restore_focused_terminal(cx));
        cx.notify();
    }

    pub(super) fn cancel_folder_rename(&mut self, cx: &mut Context<Self>) {
        cancel_rename(&mut self.folder_rename);
        self.workspace.update(cx, |ws, cx| ws.restore_focused_terminal(cx));
        cx.notify();
    }

    fn create_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let folder_id = self.workspace.update(cx, |ws, cx| {
            ws.create_folder("New Folder".to_string(), cx)
        });
        // Immediately start renaming the new folder
        self.start_folder_rename(folder_id, "New Folder".to_string(), window, cx);
    }

    /// Public accessor for the focus handle (used by RootView for FocusSidebar)
    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus_handle
    }

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
    fn build_cursor_items(&self, cx: &mut Context<Self>) -> Vec<SidebarCursorItem> {
        let workspace = self.workspace.read(cx);
        let all_projects: HashMap<&str, &ProjectData> = workspace.data().projects.iter()
            .map(|p| (p.id.as_str(), p))
            .collect();
        let all_project_ids: HashSet<&str> = workspace.data().projects.iter()
            .map(|p| p.id.as_str()).collect();

        // Build worktree children map
        let mut worktree_children_map: HashMap<String, Vec<&ProjectData>> = HashMap::new();
        for project in &workspace.data().projects {
            if let Some(ref wt_info) = project.worktree_info {
                if all_project_ids.contains(wt_info.parent_project_id.as_str()) {
                    worktree_children_map
                        .entry(wt_info.parent_project_id.clone())
                        .or_default()
                        .push(project);
                }
            }
        }

        let mut cursor_items = Vec::new();

        for id in &workspace.data().project_order {
            // Check if this is a folder
            if let Some(folder) = workspace.data().folders.iter().find(|f| &f.id == id) {
                cursor_items.push(SidebarCursorItem::Folder { folder_id: folder.id.clone() });

                if !folder.collapsed {
                    for pid in &folder.project_ids {
                        if let Some(&project) = all_projects.get(pid.as_str()) {
                            // Skip worktree children that have a parent in the project list
                            if project.worktree_info.as_ref().map_or(false, |w| {
                                all_project_ids.contains(w.parent_project_id.as_str())
                            }) {
                                continue;
                            }
                            self.push_project_cursor_items(project, &worktree_children_map, &mut cursor_items);
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
                self.push_project_cursor_items(project, &worktree_children_map, &mut cursor_items);
            }
        }

        cursor_items
    }

    /// Helper: push a project row + its expanded terminals + worktree children into cursor items
    fn push_project_cursor_items(
        &self,
        project: &ProjectData,
        worktree_children_map: &HashMap<String, Vec<&ProjectData>>,
        cursor_items: &mut Vec<SidebarCursorItem>,
    ) {
        cursor_items.push(SidebarCursorItem::Project { project_id: project.id.clone() });

        // Expanded terminal items
        if self.expanded_projects.contains(&project.id) {
            if let Some(ref layout) = project.layout {
                for tid in layout.collect_terminal_ids() {
                    cursor_items.push(SidebarCursorItem::Terminal {
                        project_id: project.id.clone(),
                        terminal_id: tid,
                    });
                }
            }
        }

        // Worktree children (always visible below parent)
        if let Some(children) = worktree_children_map.get(&project.id) {
            for child in children {
                cursor_items.push(SidebarCursorItem::WorktreeProject { project_id: child.id.clone() });

                // Expanded terminal items for worktree child
                if self.expanded_projects.contains(&child.id) {
                    if let Some(ref layout) = child.layout {
                        for tid in layout.collect_terminal_ids() {
                            cursor_items.push(SidebarCursorItem::Terminal {
                                project_id: child.id.clone(),
                                terminal_id: tid,
                            });
                        }
                    }
                }
            }
        }
    }

    /// Clamp cursor to valid range
    fn validate_cursor(&mut self, item_count: usize) {
        if item_count == 0 {
            self.cursor_index = None;
        } else if let Some(ref mut idx) = self.cursor_index {
            if *idx >= item_count {
                *idx = item_count - 1;
            }
        }
    }

    /// Check if any rename or color picker is active (blocks keyboard nav)
    fn is_interactive_mode_active(&self) -> bool {
        self.terminal_rename.is_some()
            || self.project_rename.is_some()
            || self.folder_rename.is_some()
            || self.color_picker_project_id.is_some()
            || self.color_picker_folder_id.is_some()
    }

    fn handle_sidebar_up(&mut self, _: &SidebarUp, _window: &mut Window, cx: &mut Context<Self>) {
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

    fn handle_sidebar_down(&mut self, _: &SidebarDown, _window: &mut Window, cx: &mut Context<Self>) {
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

    fn handle_sidebar_confirm(&mut self, _: &SidebarConfirm, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_interactive_mode_active() { return; }
        let items = self.build_cursor_items(cx);
        let Some(idx) = self.cursor_index else { return };
        let Some(item) = items.get(idx) else { return };

        match item.clone() {
            SidebarCursorItem::Project { project_id } |
            SidebarCursorItem::WorktreeProject { project_id } => {
                self.workspace.update(cx, |ws, cx| {
                    ws.set_focused_project(Some(project_id), cx);
                });
                // Restore focus to terminal
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
                self.workspace.update(cx, |ws, cx| {
                    ws.toggle_folder_collapsed(&folder_id, cx);
                });
            }
        }
        cx.notify();
    }

    fn handle_sidebar_toggle_expand(&mut self, _: &SidebarToggleExpand, _window: &mut Window, cx: &mut Context<Self>) {
        if self.is_interactive_mode_active() { return; }
        let items = self.build_cursor_items(cx);
        let Some(idx) = self.cursor_index else { return };
        let Some(item) = items.get(idx) else { return };

        match item.clone() {
            SidebarCursorItem::Folder { folder_id } => {
                self.workspace.update(cx, |ws, cx| {
                    ws.toggle_folder_collapsed(&folder_id, cx);
                });
            }
            SidebarCursorItem::Project { project_id } |
            SidebarCursorItem::WorktreeProject { project_id } => {
                self.toggle_expanded(&project_id);
            }
            SidebarCursorItem::Terminal { .. } => {}
        }
        cx.notify();
    }

    fn handle_sidebar_escape(&mut self, _: &SidebarEscape, window: &mut Window, cx: &mut Context<Self>) {
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

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        div()
            .h(px(35.0))
            .px(px(12.0))
            .flex()
            .items_center()
            .justify_between()
            .bg(rgb(t.bg_header))
            .border_b_1()
            .border_color(rgb(t.border))
            .child(
                div()
                    .text_size(px(11.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(t.text_secondary))
                    .child("EXPLORER"),
            )
            .child(
                h_flex()
                    .gap(px(2.0))
                    .child(
                        // New folder button
                        div()
                            .id("new-folder-btn")
                            .cursor_pointer()
                            .px(px(4.0))
                            .py(px(2.0))
                            .rounded(px(4.0))
                            .hover(|s| s.bg(rgb(t.bg_hover)))
                            .child(
                                svg()
                                    .path("icons/folder.svg")
                                    .size(px(14.0))
                                    .text_color(rgb(t.text_secondary))
                            )
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.create_folder(window, cx);
                            })),
                    )
                    .child(
                        // Add project button
                        div()
                            .id("add-project-btn")
                            .cursor_pointer()
                            .px(px(4.0))
                            .py(px(2.0))
                            .rounded(px(4.0))
                            .hover(|s| s.bg(rgb(t.bg_hover)))
                            .flex()
                            .items_center()
                            .gap(px(4.0))
                            .child(
                                div()
                                    .text_size(px(14.0))
                                    .text_color(rgb(t.text_secondary))
                                    .child("+"),
                            )
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(rgb(t.text_secondary))
                                    .child("Add Project"),
                            )
                            .on_click(cx.listener(|this, _, _window, cx| {
                                this.workspace.update(cx, |ws, cx| {
                                    ws.push_overlay_request(
                                        crate::workspace::requests::OverlayRequest::AddProjectDialog,
                                        cx,
                                    );
                                });
                            })),
                    ),
            )
    }

    fn render_keybindings_hint(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        // Get the keybinding for ShowKeybindings action
        let shortcut = get_config()
            .bindings
            .get("ShowKeybindings")
            .and_then(|entries| entries.first())
            .map(|e| format_keystroke(&e.keystroke))
            .unwrap_or_else(|| "?".to_string());

        div()
            .id("keybindings-hint")
            .h(px(28.0))
            .px(px(12.0))
            .flex()
            .items_center()
            .gap(px(6.0))
            .cursor_pointer()
            .border_t_1()
            .border_color(rgb(t.border))
            .hover(|s| s.bg(rgb(t.bg_hover)))
            .on_action(cx.listener(|_, _: &ShowKeybindings, _, _| {
                // Action will be handled by parent
            }))
            .on_click(|_, _, cx| {
                cx.dispatch_action(&ShowKeybindings);
            })
            .child(
                svg()
                    .path("icons/keyboard.svg")
                    .size(px(14.0))
                    .text_color(rgb(t.text_muted))
            )
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(rgb(t.text_muted))
                    .child("Shortcuts"),
            )
            .child(
                div()
                    .px(px(4.0))
                    .py(px(2.0))
                    .rounded(px(3.0))
                    .bg(rgb(t.bg_primary))
                    .border_1()
                    .border_color(rgb(t.border))
                    .text_size(px(10.0))
                    .font_family("monospace")
                    .text_color(rgb(t.text_muted))
                    .child(shortcut),
            )
    }

    fn render_projects_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let workspace = self.workspace.clone();

        div()
            .h(px(28.0))
            .px(px(12.0))
            .flex()
            .items_center()
            .justify_between()
            .cursor_pointer()
            .hover(|s| s.bg(rgb(t.bg_hover)))
            .id("projects-header")
            .on_click(move |_, _window, cx| {
                workspace.update(cx, |ws, cx| {
                    ws.set_focused_project(None, cx);
                });
            })
            .child(
                div()
                    .text_size(px(11.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(t.text_secondary))
                    .child("PROJECTS"),
            )
    }
}

/// Lightweight projection of ProjectData for sidebar rendering.
/// Avoids cloning the full LayoutNode tree, path, hidden_terminals, and hooks
/// which are never used by the sidebar.
pub(super) struct SidebarProjectInfo {
    pub id: String,
    pub name: String,
    pub is_visible: bool,
    pub folder_color: FolderColor,
    pub has_layout: bool,
    pub terminal_ids: Vec<String>,
    pub terminal_names: HashMap<String, String>,
}

impl SidebarProjectInfo {
    fn from_project(project: &ProjectData) -> Self {
        Self {
            id: project.id.clone(),
            name: project.name.clone(),
            is_visible: project.is_visible,
            folder_color: project.folder_color,
            has_layout: project.layout.is_some(),
            terminal_ids: project.layout.as_ref()
                .map(|l| l.collect_terminal_ids())
                .unwrap_or_default(),
            terminal_names: project.terminal_names.clone(),
        }
    }
}

/// An item in the sidebar's top-level ordering: either a project or a folder
enum SidebarItem {
    Project {
        project: SidebarProjectInfo,
        index: usize,
        worktree_children: Vec<SidebarProjectInfo>,
    },
    Folder {
        folder: FolderData,
        index: usize,
        projects: Vec<SidebarProjectInfo>,
        worktree_children: HashMap<String, Vec<SidebarProjectInfo>>,
    },
}

impl Render for Sidebar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        // Process pending sidebar requests (drained from Workspace by observer)
        let pending = std::mem::take(&mut self.pending_sidebar_requests);
        for request in pending {
            match request {
                SidebarRequest::RenameProject { project_id, project_name } => {
                    self.start_project_rename(project_id, project_name, window, cx);
                }
                SidebarRequest::RenameFolder { folder_id, folder_name } => {
                    self.start_folder_rename(folder_id, folder_name, window, cx);
                }
            }
        }

        // Clear cursor when sidebar loses focus
        if self.cursor_index.is_some() && !self.focus_handle.is_focused(window) {
            self.cursor_index = None;
        }

        let workspace = self.workspace.read(cx);

        // Collect all projects for lookup
        let all_projects: HashMap<&str, &ProjectData> = workspace.data().projects.iter()
            .map(|p| (p.id.as_str(), p))
            .collect();

        // Build worktree children map (child project -> parent project)
        let mut worktree_children_map: HashMap<String, Vec<SidebarProjectInfo>> = HashMap::new();
        let all_project_ids: HashSet<&str> = workspace.data().projects.iter().map(|p| p.id.as_str()).collect();
        for project in &workspace.data().projects {
            if let Some(ref wt_info) = project.worktree_info {
                if all_project_ids.contains(wt_info.parent_project_id.as_str()) {
                    worktree_children_map
                        .entry(wt_info.parent_project_id.clone())
                        .or_default()
                        .push(SidebarProjectInfo::from_project(project));
                }
            }
        }

        // Build sidebar items from project_order
        let mut items: Vec<SidebarItem> = Vec::new();
        let mut top_index = 0;
        for id in &workspace.data().project_order {
            // Check if this is a folder
            if let Some(folder) = workspace.data().folders.iter().find(|f| &f.id == id) {
                let folder_projects: Vec<SidebarProjectInfo> = folder.project_ids.iter()
                    .filter_map(|pid| all_projects.get(pid.as_str()))
                    .filter(|p| p.worktree_info.is_none() || !all_project_ids.contains(
                        p.worktree_info.as_ref().map(|w| w.parent_project_id.as_str()).unwrap_or("")
                    ))
                    .map(|p| SidebarProjectInfo::from_project(p))
                    .collect();
                let mut folder_wt_children: HashMap<String, Vec<SidebarProjectInfo>> = HashMap::new();
                for fp in &folder_projects {
                    if let Some(children) = worktree_children_map.remove(&fp.id) {
                        folder_wt_children.insert(fp.id.clone(), children);
                    }
                }
                items.push(SidebarItem::Folder {
                    folder: folder.clone(),
                    index: top_index,
                    projects: folder_projects,
                    worktree_children: folder_wt_children,
                });
                top_index += 1;
                continue;
            }

            // Check if this is a top-level project (not a worktree child)
            if let Some(&project) = all_projects.get(id.as_str()) {
                if let Some(ref wt_info) = project.worktree_info {
                    if all_project_ids.contains(wt_info.parent_project_id.as_str()) {
                        // This is a worktree child shown under its parent, skip
                        continue;
                    }
                }
                let wt_children = worktree_children_map.remove(&project.id).unwrap_or_default();
                items.push(SidebarItem::Project {
                    project: SidebarProjectInfo::from_project(project),
                    index: top_index,
                    worktree_children: wt_children,
                });
                top_index += 1;
            }
        }

        let color_picker_project_id = self.color_picker_project_id.clone();
        let color_picker_folder_id = self.color_picker_folder_id.clone();
        let has_color_picker = color_picker_project_id.is_some() || color_picker_folder_id.is_some();

        // Build cursor items and validate cursor position
        let cursor_items = self.build_cursor_items(cx);
        self.validate_cursor(cursor_items.len());
        let cursor_index = self.cursor_index;

        // Build flat elements with cursor tracking
        let mut flat_elements: Vec<AnyElement> = Vec::new();
        let mut flat_idx: usize = 0;

        for item in items {
            match item {
                SidebarItem::Project { project, index, worktree_children } => {
                    let is_cursor = cursor_index == Some(flat_idx);
                    flat_elements.push(
                        self.render_project_item(&project, index, is_cursor, window, cx).into_any_element()
                    );
                    flat_idx += 1;

                    // Expanded terminals
                    if self.expanded_projects.contains(&project.id) {
                        let minimized_states: Vec<(String, bool)> = {
                            let ws = self.workspace.read(cx);
                            project.terminal_ids.iter().map(|id| {
                                (id.clone(), ws.is_terminal_minimized(&project.id, id))
                            }).collect()
                        };
                        for (tid, is_minimized) in &minimized_states {
                            let is_cursor = cursor_index == Some(flat_idx);
                            flat_elements.push(
                                self.render_terminal_item(&project.id, tid, &project.terminal_names, *is_minimized, 28.0, "", is_cursor, cx).into_any_element()
                            );
                            flat_idx += 1;
                        }
                    }

                    // Worktree children
                    for child in &worktree_children {
                        let is_cursor = cursor_index == Some(flat_idx);
                        flat_elements.push(
                            self.render_worktree_item(child, is_cursor, window, cx).into_any_element()
                        );
                        flat_idx += 1;

                        if self.expanded_projects.contains(&child.id) {
                            let minimized_states: Vec<(String, bool)> = {
                                let ws = self.workspace.read(cx);
                                child.terminal_ids.iter().map(|id| {
                                    (id.clone(), ws.is_terminal_minimized(&child.id, id))
                                }).collect()
                            };
                            for (tid, is_minimized) in &minimized_states {
                                let is_cursor = cursor_index == Some(flat_idx);
                                flat_elements.push(
                                    self.render_terminal_item(&child.id, tid, &child.terminal_names, *is_minimized, 48.0, "wt-", is_cursor, cx).into_any_element()
                                );
                                flat_idx += 1;
                            }
                        }
                    }
                }
                SidebarItem::Folder { folder, index, projects, worktree_children } => {
                    let is_cursor = cursor_index == Some(flat_idx);
                    flat_elements.push(
                        self.render_folder_header(&folder, index, projects.len(), is_cursor, window, cx).into_any_element()
                    );
                    flat_idx += 1;

                    // Folder children when not collapsed
                    if !folder.collapsed {
                        for fp in &projects {
                            let is_cursor = cursor_index == Some(flat_idx);
                            flat_elements.push(
                                self.render_folder_project_item(fp, &folder.id, is_cursor, window, cx).into_any_element()
                            );
                            flat_idx += 1;

                            // Expanded terminals for folder project
                            if self.expanded_projects.contains(&fp.id) {
                                let minimized_states: Vec<(String, bool)> = {
                                    let ws = self.workspace.read(cx);
                                    fp.terminal_ids.iter().map(|id| {
                                        (id.clone(), ws.is_terminal_minimized(&fp.id, id))
                                    }).collect()
                                };
                                for (tid, is_minimized) in &minimized_states {
                                    let is_cursor = cursor_index == Some(flat_idx);
                                    flat_elements.push(
                                        self.render_terminal_item(&fp.id, tid, &fp.terminal_names, *is_minimized, 28.0, "", is_cursor, cx).into_any_element()
                                    );
                                    flat_idx += 1;
                                }
                            }

                            // Worktree children for folder project
                            if let Some(wt_children) = worktree_children.get(&fp.id) {
                                for child in wt_children {
                                    let is_cursor = cursor_index == Some(flat_idx);
                                    flat_elements.push(
                                        self.render_worktree_item(child, is_cursor, window, cx).into_any_element()
                                    );
                                    flat_idx += 1;

                                    if self.expanded_projects.contains(&child.id) {
                                        let minimized_states: Vec<(String, bool)> = {
                                            let ws = self.workspace.read(cx);
                                            child.terminal_ids.iter().map(|id| {
                                                (id.clone(), ws.is_terminal_minimized(&child.id, id))
                                            }).collect()
                                        };
                                        for (tid, is_minimized) in &minimized_states {
                                            let is_cursor = cursor_index == Some(flat_idx);
                                            flat_elements.push(
                                                self.render_terminal_item(&child.id, tid, &child.terminal_names, *is_minimized, 48.0, "wt-", is_cursor, cx).into_any_element()
                                            );
                                            flat_idx += 1;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        div()
            .relative()
            .w_full()
            .h_full()
            .flex()
            .flex_col()
            .bg(rgb(t.bg_secondary))
            .track_focus(&self.focus_handle)
            .key_context("Sidebar")
            .on_action(cx.listener(Self::handle_sidebar_up))
            .on_action(cx.listener(Self::handle_sidebar_down))
            .on_action(cx.listener(Self::handle_sidebar_confirm))
            .on_action(cx.listener(Self::handle_sidebar_toggle_expand))
            .on_action(cx.listener(Self::handle_sidebar_escape))
            .child(self.render_header(cx))
            .child(self.render_projects_header(cx))
            .child(
                div()
                    .id("sidebar-scroll")
                    .flex_1()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll_handle)
                    .children(flat_elements),
            )
            .child(self.render_keybindings_hint(cx))
            // Color picker overlay
            .when(has_color_picker, |d: Div| {
                d.child(
                    // Backdrop to close picker when clicking outside
                    div()
                        .id("color-picker-backdrop")
                        .absolute()
                        .inset_0()
                        .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                            this.hide_color_picker(cx);
                        }))
                        .on_scroll_wheel(|_, _, cx| {
                            cx.stop_propagation();
                        })
                )
                .when(color_picker_project_id.is_some(), |d: Div| {
                    let project_id = color_picker_project_id.unwrap();
                    d.child(self.render_color_picker(&project_id, cx))
                })
                .when(color_picker_folder_id.is_some(), |d: Div| {
                    let folder_id = color_picker_folder_id.unwrap();
                    d.child(self.render_folder_color_picker(&folder_id, cx))
                })
            })
    }
}
