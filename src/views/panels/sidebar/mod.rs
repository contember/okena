//! Sidebar view with project and terminal list
//!
//! The sidebar provides navigation for projects and terminals, with features for:
//! - Adding/managing projects
//! - Renaming terminals and projects
//! - Drag-and-drop project reordering
//! - Folder color customization

mod color_picker;
mod project_list;

use crate::keybindings::{format_keystroke, get_config, NewProject, ShowKeybindings};
use crate::theme::{theme, FolderColor};
use crate::ui::ClickDetector;
use crate::views::components::{
    cancel_rename, finish_rename, start_rename_with_blur,
    RenameState,
};
use crate::views::root::TerminalsRegistry;
use crate::workspace::state::{ProjectData, Workspace};
use gpui::*;
use gpui::prelude::*;
use std::collections::{HashMap, HashSet};

/// Drag payload for project reordering
#[derive(Clone)]
pub(super) struct ProjectDrag {
    pub project_id: String,
    pub project_name: String,
}

/// Drag preview view
pub(super) struct ProjectDragView {
    pub name: String,
}

impl Render for ProjectDragView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(px(8.0))
            .py(px(4.0))
            .bg(rgb(0x2d2d2d))
            .border_1()
            .border_color(rgb(0x404040))
            .rounded(px(4.0))
            .shadow_lg()
            .text_size(px(12.0))
            .text_color(rgb(0xffffff))
            .child(self.name.clone())
    }
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
}

impl Sidebar {
    pub fn new(workspace: Entity<Workspace>, terminals: TerminalsRegistry) -> Self {
        Self {
            workspace,
            expanded_projects: HashSet::new(),
            terminals,
            terminal_rename: None,
            terminal_click_detector: ClickDetector::new(),
            project_rename: None,
            project_click_detector: ClickDetector::new(),
            color_picker_project_id: None,
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
        cx.notify();
    }

    fn hide_color_picker(&mut self, cx: &mut Context<Self>) {
        self.color_picker_project_id = None;
        cx.notify();
    }

    pub(super) fn set_folder_color(&mut self, project_id: &str, color: FolderColor, cx: &mut Context<Self>) {
        self.workspace.update(cx, |ws, cx| {
            ws.set_folder_color(project_id, color, cx);
        });
        self.hide_color_picker(cx);
    }

    fn request_context_menu(&mut self, project_id: String, position: Point<Pixels>, cx: &mut Context<Self>) {
        self.workspace.update(cx, |ws, cx| {
            ws.request_context_menu(&project_id, position, cx);
        });
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
                // Add project button
                div()
                    .id("add-project-btn")
                    .cursor_pointer()
                    .px(px(6.0))
                    .py(px(2.0))
                    .rounded(px(4.0))
                    .hover(|s| s.bg(rgb(t.bg_hover)))
                    .text_size(px(14.0))
                    .text_color(rgb(t.text_secondary))
                    .child("+")
                    .on_click(|_, _, cx| {
                        cx.dispatch_action(&NewProject);
                    }),
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

impl Render for Sidebar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        // Check for pending project rename request from context menu
        let pending_rename = self.workspace.read(cx).pending_project_rename.clone();
        if let Some(request) = pending_rename {
            self.workspace.update(cx, |ws, cx| {
                ws.clear_project_rename_request(cx);
            });
            self.start_project_rename(request.project_id, request.project_name, window, cx);
        }

        let workspace = self.workspace.read(cx);
        // Get projects in order from project_order
        let ordered_projects: Vec<ProjectData> = workspace.data.project_order
            .iter()
            .filter_map(|id| workspace.data.projects.iter().find(|p| &p.id == id).cloned())
            .collect();

        // Collect all project IDs for orphan detection
        let all_project_ids: HashSet<&str> = ordered_projects.iter().map(|p| p.id.as_str()).collect();

        // Split into main projects and worktree children grouped by parent
        let mut worktree_children: HashMap<String, Vec<ProjectData>> = HashMap::new();
        let mut main_projects: Vec<(ProjectData, usize)> = Vec::new();
        let mut main_index = 0;
        for project in &ordered_projects {
            if let Some(ref wt_info) = project.worktree_info {
                if all_project_ids.contains(wt_info.parent_project_id.as_str()) {
                    worktree_children
                        .entry(wt_info.parent_project_id.clone())
                        .or_default()
                        .push(project.clone());
                    continue;
                }
            }
            main_projects.push((project.clone(), main_index));
            main_index += 1;
        }

        let color_picker_project_id = self.color_picker_project_id.clone();

        div()
            .relative()
            .w_full()
            .h_full()
            .flex()
            .flex_col()
            .bg(rgb(t.bg_secondary))
            .child(self.render_header(cx))
            .child(self.render_projects_header(cx))
            .child(
                div()
                    .id("sidebar-scroll")
                    .flex_1()
                    .overflow_y_scroll()
                    .children(
                        main_projects
                            .iter()
                            .map(|(p, i)| {
                                let children = worktree_children.get(&p.id);
                                self.render_project_item_with_worktrees(p, *i, children, window, cx)
                            }),
                    ),
            )
            .child(self.render_keybindings_hint(cx))
            // Color picker overlay
            .when(color_picker_project_id.is_some(), |d| {
                let project_id = color_picker_project_id.unwrap();
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
                .child(self.render_color_picker(&project_id, cx))
            })
    }
}
