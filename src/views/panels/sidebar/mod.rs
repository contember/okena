//! Sidebar view with project and terminal list
//!
//! The sidebar provides navigation for projects and terminals, with features for:
//! - Adding/managing projects
//! - Renaming terminals and projects
//! - Drag-and-drop project reordering
//! - Folder color customization

mod add_dialog;
mod color_picker;
mod project_list;

use crate::keybindings::{format_keystroke, get_config, ShowKeybindings};
use crate::theme::{theme, FolderColor};
use crate::ui::ClickDetector;
use crate::views::components::{
    cancel_rename, finish_rename, start_rename_with_blur,
    RenameState, SimpleInputState, PathAutoCompleteState,
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
    pub(super) show_add_dialog: bool,
    pub(super) name_input: Option<Entity<SimpleInputState>>,
    pub(super) path_input: Option<Entity<PathAutoCompleteState>>,
    /// Pending values to set on inputs (for async updates)
    pub(super) pending_name_value: Option<String>,
    pub(super) pending_path_value: Option<String>,
    pub(super) terminals: TerminalsRegistry,
    /// Terminal rename state: (project_id, terminal_id)
    pub(super) terminal_rename: Option<RenameState<(String, String)>>,
    /// Double-click detector for terminals
    terminal_click_detector: ClickDetector<String>,
    /// Project rename state
    pub(super) project_rename: Option<RenameState<String>>,
    /// Double-click detector for projects
    project_click_detector: ClickDetector<String>,
    /// Whether to create project without terminal (bookmark mode)
    pub(super) create_without_terminal: bool,
    /// Project ID for which color picker is shown
    color_picker_project_id: Option<String>,
}

impl Sidebar {
    pub fn new(workspace: Entity<Workspace>, terminals: TerminalsRegistry) -> Self {
        Self {
            workspace,
            expanded_projects: HashSet::new(),
            show_add_dialog: false,
            name_input: None,
            path_input: None,
            pending_name_value: None,
            pending_path_value: None,
            terminals,
            terminal_rename: None,
            terminal_click_detector: ClickDetector::new(),
            project_rename: None,
            project_click_detector: ClickDetector::new(),
            create_without_terminal: false,
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

    pub(super) fn ensure_inputs(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.name_input.is_none() {
            self.name_input = Some(cx.new(|cx| {
                SimpleInputState::new(cx)
                    .placeholder("Enter project name...")
            }));
        }
        if self.path_input.is_none() {
            self.path_input = Some(cx.new(|cx| {
                PathAutoCompleteState::new(cx)
            }));
        }
    }

    pub(super) fn add_project(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let name = self.name_input.as_ref().map(|i| i.read(cx).value().to_string()).unwrap_or_default();
        let path = self.path_input.as_ref().map(|i| i.read(cx).value(cx)).unwrap_or_default();

        if !name.is_empty() && !path.is_empty() {
            let with_terminal = !self.create_without_terminal;
            self.workspace.update(cx, |ws, cx| {
                ws.add_project(name, path, with_terminal, cx);
            });
            // Clear inputs
            if let Some(ref input) = self.name_input {
                input.update(cx, |i, cx| i.set_value("", cx));
            }
            if let Some(ref input) = self.path_input {
                input.update(cx, |i, cx| i.set_value("", cx));
            }
            self.show_add_dialog = false;
            self.create_without_terminal = false;
            // Exit modal mode to restore terminal focus
            self.workspace.update(cx, |ws, cx| ws.restore_focused_terminal(cx));
            cx.notify();
        }
    }

    pub(super) fn set_quick_path(&mut self, name: &str, path: &str, _window: &mut Window, cx: &mut Context<Self>) {
        let name_str = name.to_string();
        let path_str = path.to_string();
        if let Some(ref input) = self.name_input {
            input.update(cx, |i, cx| i.set_value(&name_str, cx));
        }
        if let Some(ref input) = self.path_input {
            input.update(cx, |i, cx| i.set_value(&path_str, cx));
        }
        cx.notify();
    }

    pub(super) fn open_folder_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Select project folder".into()),
        });

        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(selected_paths))) = paths.await {
                if let Some(path) = selected_paths.first() {
                    let path_str = path.to_string_lossy().to_string();
                    let name_str = path.file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| "Project".to_string());

                    this.update(cx, |this, cx| {
                        // Store pending values to be applied in next render
                        this.pending_path_value = Some(path_str);
                        this.pending_name_value = Some(name_str);
                        cx.notify();
                    }).ok();
                }
            }
        }).detach();
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
                    .on_click(cx.listener(|this, _, _window, cx| {
                        this.show_add_dialog = !this.show_add_dialog;
                        // Enter/exit modal mode to prevent terminal from stealing focus
                        if this.show_add_dialog {
                            this.workspace.update(cx, |ws, cx| ws.clear_focused_terminal(cx));
                        } else {
                            this.workspace.update(cx, |ws, cx| ws.restore_focused_terminal(cx));
                        }
                        cx.notify();
                    })),
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

        let show_add_dialog = self.show_add_dialog;
        let color_picker_project_id = self.color_picker_project_id.clone();

        // Check if we have suggestions to show (must be checked before dialog renders)
        let has_suggestions = self.path_input.as_ref()
            .map(|input| input.read(cx).has_suggestions())
            .unwrap_or(false);

        div()
            .relative()
            .w_full()
            .h_full()
            .flex()
            .flex_col()
            .bg(rgb(t.bg_secondary))
            .child(self.render_header(cx))
            .when(show_add_dialog, |d| d.child(self.render_add_dialog(window, cx)))
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
            // Path suggestions overlay - rendered LAST to appear on top of everything
            .when(show_add_dialog && has_suggestions, |d| {
                d.child(self.render_path_suggestions(cx))
            })
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
