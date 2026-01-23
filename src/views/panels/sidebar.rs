use crate::keybindings::{format_keystroke, get_config, ShowKeybindings};
use crate::theme::theme;
use crate::ui::ClickDetector;
use crate::views::components::{
    cancel_rename, finish_rename, is_renaming, rename_input, start_rename_with_blur,
    RenameState, SimpleInput, SimpleInputState, PathAutoCompleteState,
};
use crate::views::root::TerminalsRegistry;
use crate::workspace::state::{ProjectData, Workspace};
use gpui::*;
use gpui::prelude::*;
use gpui_component::tooltip::Tooltip;
use std::collections::HashSet;

/// Drag payload for project reordering
#[derive(Clone)]
struct ProjectDrag {
    project_id: String,
    project_name: String,
}

/// Drag preview view
struct ProjectDragView {
    name: String,
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
    show_add_dialog: bool,
    name_input: Option<Entity<SimpleInputState>>,
    path_input: Option<Entity<PathAutoCompleteState>>,
    /// Pending values to set on inputs (for async updates)
    pending_name_value: Option<String>,
    pending_path_value: Option<String>,
    terminals: TerminalsRegistry,
    /// Terminal rename state: (project_id, terminal_id)
    terminal_rename: Option<RenameState<(String, String)>>,
    /// Double-click detector for terminals
    terminal_click_detector: ClickDetector<String>,
    /// Project rename state
    project_rename: Option<RenameState<String>>,
    /// Double-click detector for projects
    project_click_detector: ClickDetector<String>,
    /// Whether to create project without terminal (bookmark mode)
    create_without_terminal: bool,
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
        }
    }

    /// Check for double-click on terminal and return true if detected
    fn check_double_click(&mut self, terminal_id: &str) -> bool {
        self.terminal_click_detector.check(terminal_id.to_string())
    }

    fn toggle_expanded(&mut self, project_id: &str) {
        if self.expanded_projects.contains(project_id) {
            self.expanded_projects.remove(project_id);
        } else {
            self.expanded_projects.insert(project_id.to_string());
        }
    }

    fn start_rename(&mut self, project_id: String, terminal_id: String, current_name: String, window: &mut Window, cx: &mut Context<Self>) {
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

    fn finish_rename(&mut self, cx: &mut Context<Self>) {
        if let Some(((project_id, terminal_id), new_name)) = finish_rename(&mut self.terminal_rename, cx) {
            self.workspace.update(cx, |ws, cx| {
                ws.rename_terminal(&project_id, &terminal_id, new_name, cx);
            });
        }
        self.workspace.update(cx, |ws, cx| ws.restore_focused_terminal(cx));
        cx.notify();
    }

    fn cancel_rename(&mut self, cx: &mut Context<Self>) {
        cancel_rename(&mut self.terminal_rename);
        self.workspace.update(cx, |ws, cx| ws.restore_focused_terminal(cx));
        cx.notify();
    }

    /// Check for double-click on project and return true if detected
    fn check_project_double_click(&mut self, project_id: &str) -> bool {
        self.project_click_detector.check(project_id.to_string())
    }

    fn start_project_rename(&mut self, project_id: String, current_name: String, window: &mut Window, cx: &mut Context<Self>) {
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

    fn finish_project_rename(&mut self, cx: &mut Context<Self>) {
        if let Some((project_id, new_name)) = finish_rename(&mut self.project_rename, cx) {
            self.workspace.update(cx, |ws, cx| {
                ws.rename_project(&project_id, new_name, cx);
            });
        }
        self.workspace.update(cx, |ws, cx| ws.restore_focused_terminal(cx));
        cx.notify();
    }

    fn cancel_project_rename(&mut self, cx: &mut Context<Self>) {
        cancel_rename(&mut self.project_rename);
        self.workspace.update(cx, |ws, cx| ws.restore_focused_terminal(cx));
        cx.notify();
    }

    fn request_context_menu(&mut self, project_id: String, position: Point<Pixels>, cx: &mut Context<Self>) {
        self.workspace.update(cx, |ws, cx| {
            ws.request_context_menu(&project_id, position, cx);
        });
    }

    fn ensure_inputs(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
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

    fn add_project(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
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

    fn set_quick_path(&mut self, name: &str, path: &str, _window: &mut Window, cx: &mut Context<Self>) {
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

    fn open_folder_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

    fn render_add_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        // Ensure inputs exist
        self.ensure_inputs(window, cx);

        // Apply pending values from async operations
        if let Some(name_value) = self.pending_name_value.take() {
            if let Some(ref input) = self.name_input {
                input.update(cx, |i, cx| i.set_value(&name_value, cx));
            }
        }
        if let Some(path_value) = self.pending_path_value.take() {
            if let Some(ref input) = self.path_input {
                input.update(cx, |i, cx| i.set_value(&path_value, cx));
            }
        }

        // Safe to unwrap since ensure_inputs was just called
        let name_input = self.name_input.clone().expect("name_input should exist after ensure_inputs");
        let path_input = self.path_input.clone().expect("path_input should exist after ensure_inputs");

        div()
            .relative()
            .p(px(12.0))
            .flex()
            .flex_col()
            .gap(px(8.0))
            .bg(rgb(t.bg_primary))
            .border_1()
            .border_color(rgb(t.border))
            .rounded(px(4.0))
            .m(px(8.0))
            .child(
                div()
                    .text_size(px(12.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(t.text_primary))
                    .child("Add Project"),
            )
            .child(
                // Name input
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(rgb(t.text_muted))
                            .child("Name:"),
                    )
                    .child(
                        div()
                            .bg(rgb(t.bg_secondary))
                            .border_1()
                            .border_color(rgb(t.border))
                            .rounded(px(4.0))
                            .child(
                                SimpleInput::new(&name_input)
                                    .text_size(px(12.0))
                            )
                    ),
            )
            .child(
                // Path input with auto-complete
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(rgb(t.text_muted))
                            .child("Path (Tab to complete):"),
                    )
                    .child(path_input),
            )
            .child(
                // Browse button
                div()
                    .id("browse-folder-btn")
                    .cursor_pointer()
                    .px(px(8.0))
                    .py(px(4.0))
                    .rounded(px(4.0))
                    .bg(rgb(t.bg_secondary))
                    .hover(|s| s.bg(rgb(t.bg_hover)))
                    .text_size(px(11.0))
                    .text_color(rgb(t.text_primary))
                    .child("Browse...")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_folder_picker(window, cx);
                    })),
            )
            .child(
                // Quick add buttons for common paths
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(4.0))
                    .child(
                        div()
                            .id("quick-add-home")
                            .cursor_pointer()
                            .px(px(8.0))
                            .py(px(4.0))
                            .rounded(px(4.0))
                            .bg(rgb(t.bg_secondary))
                            .hover(|s| s.bg(rgb(t.bg_hover)))
                            .text_size(px(11.0))
                            .text_color(rgb(t.text_primary))
                            .child("Home (~)")
                            .on_click(cx.listener(|this, _, window, cx| {
                                let path = dirs::home_dir()
                                    .map(|p| p.to_string_lossy().to_string())
                                    .unwrap_or_else(|| "/home".to_string());
                                this.set_quick_path("Home", &path, window, cx);
                            })),
                    )
                    .child(
                        div()
                            .id("quick-add-tmp")
                            .cursor_pointer()
                            .px(px(8.0))
                            .py(px(4.0))
                            .rounded(px(4.0))
                            .bg(rgb(t.bg_secondary))
                            .hover(|s| s.bg(rgb(t.bg_hover)))
                            .text_size(px(11.0))
                            .text_color(rgb(t.text_primary))
                            .child("Tmp (/tmp)")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.set_quick_path("Tmp", "/tmp", window, cx);
                            })),
                    )
                    .child(
                        div()
                            .id("quick-add-projects")
                            .cursor_pointer()
                            .px(px(8.0))
                            .py(px(4.0))
                            .rounded(px(4.0))
                            .bg(rgb(t.bg_secondary))
                            .hover(|s| s.bg(rgb(t.bg_hover)))
                            .text_size(px(11.0))
                            .text_color(rgb(t.text_primary))
                            .child("Projects")
                            .on_click(cx.listener(|this, _, window, cx| {
                                let path = dirs::home_dir()
                                    .map(|p| p.join("projects").to_string_lossy().to_string())
                                    .unwrap_or_else(|| "/home/projects".to_string());
                                this.set_quick_path("Projects", &path, window, cx);
                            })),
                    ),
            )
            .child(
                // Create without terminal checkbox
                {
                    let is_checked = self.create_without_terminal;
                    div()
                        .id("create-without-terminal")
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .cursor_pointer()
                        .py(px(4.0))
                        .on_click(cx.listener(|this, _, _window, cx| {
                            this.create_without_terminal = !this.create_without_terminal;
                            cx.notify();
                        }))
                        .child(
                            // Checkbox
                            div()
                                .size(px(14.0))
                                .rounded(px(2.0))
                                .border_1()
                                .border_color(rgb(t.border))
                                .bg(if is_checked { rgb(t.border_active) } else { rgb(t.bg_secondary) })
                                .flex()
                                .items_center()
                                .justify_center()
                                .when(is_checked, |d| {
                                    d.child(
                                        svg()
                                            .path("icons/check.svg")
                                            .size(px(10.0))
                                            .text_color(rgb(0xffffff))
                                    )
                                })
                        )
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(rgb(t.text_secondary))
                                .child("Create as bookmark (no terminal)")
                        )
                }
            )
            .child(
                // Action buttons
                div()
                    .flex()
                    .gap(px(8.0))
                    .justify_end()
                    .child(
                        div()
                            .id("cancel-add-btn")
                            .cursor_pointer()
                            .px(px(12.0))
                            .py(px(6.0))
                            .rounded(px(4.0))
                            .bg(rgb(t.bg_secondary))
                            .hover(|s| s.bg(rgb(t.bg_hover)))
                            .text_size(px(12.0))
                            .text_color(rgb(t.text_secondary))
                            .child("Cancel")
                            .on_click(cx.listener(|this, _, _window, cx| {
                                this.show_add_dialog = false;
                                this.create_without_terminal = false;
                                if let Some(ref input) = this.name_input {
                                    input.update(cx, |i, cx| i.set_value("", cx));
                                }
                                if let Some(ref input) = this.path_input {
                                    input.update(cx, |i, cx| i.set_value("", cx));
                                }
                                // Exit modal mode to restore terminal focus
                                this.workspace.update(cx, |ws, cx| ws.restore_focused_terminal(cx));
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .id("confirm-add-btn")
                            .cursor_pointer()
                            .px(px(12.0))
                            .py(px(6.0))
                            .rounded(px(4.0))
                            .bg(rgb(t.button_primary_bg))
                            .hover(|s| s.bg(rgb(t.button_primary_hover)))
                            .text_size(px(12.0))
                            .text_color(rgb(t.button_primary_fg))
                            .child("Add")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.add_project(window, cx);
                            })),
                    ),
            )
    }

    /// Render path auto-complete suggestions dropdown
    fn render_path_suggestions(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let path_input = match &self.path_input {
            Some(input) => input.clone(),
            None => return div().into_any_element(),
        };

        let state = path_input.read(cx);
        let suggestions: Vec<_> = state.suggestions().to_vec();
        let selected_index = state.selected_index();
        let scroll_handle = state.suggestions_scroll().clone();

        if suggestions.is_empty() {
            return div().into_any_element();
        }

        div()
            .absolute()
            // Position below the path input (approximately)
            // Header(35) + dialog margin(8) + padding(12) + title(20) + gap(8) + name section(48) + gap(8) + path section(48) + gap(4)
            .top(px(191.0))
            .left(px(20.0))
            .right(px(20.0))
            .id("path-suggestions-container")
            .bg(rgb(t.bg_primary))
            .border_1()
            .border_color(rgb(t.border))
            .rounded(px(4.0))
            .shadow_xl()
            .max_h(px(200.0))
            .overflow_y_scroll()
            .track_scroll(&scroll_handle)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .children(
                        suggestions.iter().enumerate().map(|(i, suggestion)| {
                            let is_selected = i == selected_index;
                            let path_input = path_input.clone();

                            div()
                                .id(ElementId::Name(format!("path-suggestion-{}", i).into()))
                                .px(px(8.0))
                                .py(px(6.0))
                                .cursor_pointer()
                                .when(is_selected, |d| d.bg(rgb(t.bg_selection)))
                                .hover(|s| s.bg(rgb(t.bg_hover)))
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .child(
                                    svg()
                                        .path(if suggestion.is_directory { "icons/folder.svg" } else { "icons/file.svg" })
                                        .size(px(14.0))
                                        .text_color(if suggestion.is_directory {
                                            rgb(t.border_active)
                                        } else {
                                            rgb(t.text_muted)
                                        })
                                )
                                .child(
                                    div()
                                        .text_size(px(12.0))
                                        .text_color(rgb(t.text_primary))
                                        .child(suggestion.display_name.clone())
                                )
                                .on_click(move |_, _window, cx| {
                                    path_input.update(cx, |state, cx| {
                                        state.select_and_complete(i, cx);
                                    });
                                })
                        })
                    )
            )
            .into_any_element()
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

    fn render_project_item(&self, project: &ProjectData, index: usize, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let is_expanded = self.expanded_projects.contains(&project.id);
        let workspace_for_focus = self.workspace.clone();
        let workspace_for_drop = self.workspace.clone();
        let project_id = project.id.clone();
        let project_id_for_focus = project.id.clone();
        let project_id_for_toggle = project.id.clone();
        let project_id_for_visibility = project.id.clone();
        let project_id_for_rename = project.id.clone();
        let project_id_for_context_menu = project.id.clone();
        let project_id_for_drag = project.id.clone();
        let project_name = project.name.clone();
        let project_name_for_rename = project.name.clone();
        let project_name_for_drag = project.name.clone();

        let is_focused = {
            let ws = self.workspace.read(cx);
            ws.focused_project_id.as_ref() == Some(&project.id)
        };

        let is_renaming = is_renaming(&self.project_rename, &project.id);

        let terminal_ids = project.layout.as_ref()
            .map(|l| l.collect_terminal_ids())
            .unwrap_or_default();
        let terminal_count = terminal_ids.len();
        let has_layout = project.layout.is_some();

        div()
            .flex()
            .flex_col()
            .child(
                // Project row
                div()
                    .id(ElementId::Name(format!("project-row-{}", project.id).into()))
                    .h(px(24.0))
                    .px(px(8.0))
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .cursor_pointer()
                    .when(is_focused, |d| {
                        d.border_l_2().border_color(rgb(t.border_active))
                    })
                    .hover(|s| s.bg(rgb(t.bg_hover)))
                    // Drag source
                    .on_drag(ProjectDrag { project_id: project_id_for_drag.clone(), project_name: project_name_for_drag.clone() }, move |drag, _position, _window, cx| {
                        cx.new(|_| ProjectDragView { name: drag.project_name.clone() })
                    })
                    // Drop target - show indicator line at top
                    .drag_over::<ProjectDrag>(move |style, _, _, _| {
                        style.border_t_2().border_color(rgb(t.border_active))
                    })
                    .on_drop(cx.listener(move |_this, drag: &ProjectDrag, _window, cx| {
                        if drag.project_id != project_id_for_drag {
                            workspace_for_drop.update(cx, |ws, cx| {
                                ws.move_project(&drag.project_id, index, cx);
                            });
                        }
                    }))
                    .on_mouse_down(MouseButton::Right, cx.listener({
                        let project_id = project_id_for_context_menu.clone();
                        move |this, event: &MouseDownEvent, _window, cx| {
                            this.request_context_menu(project_id.clone(), event.position, cx);
                            cx.stop_propagation();
                        }
                    }))
                    .child(
                        // Expand arrow
                        div()
                            .id(ElementId::Name(format!("expand-{}", project.id).into()))
                            .w(px(16.0))
                            .h(px(16.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                svg()
                                    .path(if is_expanded { "icons/chevron-down.svg" } else { "icons/chevron-right.svg" })
                                    .size(px(12.0))
                                    .text_color(rgb(t.text_secondary))
                            )
                            .on_click(cx.listener(move |this, _, _window, cx| {
                                this.toggle_expanded(&project_id_for_toggle);
                                cx.notify();
                            })),
                    )
                    .child(
                        // Project folder icon
                        div()
                            .flex_shrink_0()
                            .w(px(16.0))
                            .h(px(16.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                svg()
                                    .path("icons/folder.svg")
                                    .size(px(14.0))
                                    .text_color(rgb(t.border_active))
                            ),
                    )
                    .child(
                        // Project name (or input if renaming)
                        if is_renaming {
                            if let Some(input) = rename_input(&self.project_rename) {
                                div()
                                    .id("project-rename-input")
                                    .flex_1()
                                    .min_w_0()
                                    .bg(rgb(t.bg_hover))
                                    .rounded(px(2.0))
                                    .child(
                                        SimpleInput::new(input)
                                            .text_size(px(12.0))
                                    )
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                        cx.stop_propagation();
                                    })
                                    .on_click(|_, _window, cx| {
                                        cx.stop_propagation();
                                    })
                                    .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                                        match event.keystroke.key.as_str() {
                                            "enter" => this.finish_project_rename(cx),
                                            "escape" => this.cancel_project_rename(cx),
                                            _ => {}
                                        }
                                    }))
                                    .into_any_element()
                            } else {
                                div().flex_1().into_any_element()
                            }
                        } else {
                            div()
                                .id(ElementId::Name(format!("project-name-{}", project.id).into()))
                                .flex_1()
                                .text_size(px(12.0))
                                .text_color(rgb(t.text_primary))
                                .text_ellipsis()
                                .child(project_name)
                                .on_click(cx.listener({
                                    let project_id = project_id_for_rename;
                                    let project_id_for_focus = project_id_for_focus.clone();
                                    let name = project_name_for_rename;
                                    move |this, _event: &ClickEvent, window, cx| {
                                        if this.check_project_double_click(&project_id) {
                                            this.start_project_rename(project_id.clone(), name.clone(), window, cx);
                                        } else {
                                            // Single click - focus the project
                                            workspace_for_focus.update(cx, |ws, cx| {
                                                ws.set_focused_project(Some(project_id_for_focus.clone()), cx);
                                            });
                                        }
                                        cx.stop_propagation();
                                    }
                                }))
                                .into_any_element()
                        },
                    )
                    .child(
                        // Terminal count badge or bookmark indicator
                        if has_layout {
                            div()
                                .px(px(4.0))
                                .py(px(1.0))
                                .rounded(px(4.0))
                                .bg(rgb(t.bg_secondary))
                                .text_size(px(10.0))
                                .text_color(rgb(t.text_muted))
                                .child(format!("{}", terminal_count))
                                .into_any_element()
                        } else {
                            // Bookmark badge for terminal-less projects
                            div()
                                .px(px(4.0))
                                .py(px(1.0))
                                .rounded(px(4.0))
                                .bg(rgb(t.bg_secondary))
                                .flex()
                                .items_center()
                                .gap(px(2.0))
                                .child(
                                    svg()
                                        .path("icons/bookmark.svg")
                                        .size(px(10.0))
                                        .text_color(rgb(t.text_muted))
                                )
                                .into_any_element()
                        },
                    )
                    .child(
                        // Visibility toggle
                        {
                            let workspace = self.workspace.clone();
                            let is_visible = project.is_visible;
                            let visibility_tooltip = if is_visible { "Hide Project" } else { "Show Project" };
                            div()
                                .id(ElementId::Name(format!("visibility-{}", project.id).into()))
                                .cursor_pointer()
                                .w(px(18.0))
                                .h(px(18.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(3.0))
                                .hover(|s| s.bg(rgb(t.bg_hover)))
                                .on_click(move |_, _window, cx| {
                                    workspace.update(cx, |ws, cx| {
                                        ws.toggle_project_visibility(&project_id_for_visibility, cx);
                                    });
                                })
                                .child(
                                    svg()
                                        .path(if is_visible { "icons/eye.svg" } else { "icons/eye-off.svg" })
                                        .size(px(12.0))
                                        .text_color(if is_visible {
                                            rgb(t.text_secondary)
                                        } else {
                                            rgb(t.text_muted)
                                        })
                                )
                                .tooltip(move |_window, cx| Tooltip::new(visibility_tooltip).build(_window, cx))
                        },
                    ),
            )
            .when(is_expanded, |d| {
                // Collect minimized states first to avoid borrow checker issues
                let minimized_states: Vec<(String, bool)> = {
                    let ws = self.workspace.read(cx);
                    terminal_ids.iter().map(|id| {
                        let is_minimized = ws.is_terminal_minimized(&project_id, id);
                        (id.clone(), is_minimized)
                    }).collect()
                };

                // Show all terminals (minimized ones will be dimmed with different icon)
                let terminal_elements: Vec<_> = minimized_states.iter().map(|(id, is_minimized)| {
                    self.render_terminal_item(&project_id, id, project, *is_minimized, window, cx).into_any_element()
                }).collect();

                d.children(terminal_elements)
            })
    }

    fn render_terminal_item(
        &self,
        project_id: &str,
        terminal_id: &str,
        project: &ProjectData,
        is_minimized: bool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = theme(cx);
        let workspace = self.workspace.clone();
        let workspace_for_focus = self.workspace.clone();
        let workspace_for_minimize = self.workspace.clone();
        let project_id = project_id.to_string();
        let project_id_for_focus = project_id.clone();
        let project_id_for_minimize = project_id.clone();
        let project_id_for_rename = project_id.clone();
        let terminal_id_owned = terminal_id.to_string();
        let terminal_id_for_focus = terminal_id.to_string();
        let terminal_id_for_minimize = terminal_id.to_string();
        let terminal_id_for_rename = terminal_id.to_string();

        // Priority: custom name > OSC title > terminal ID prefix
        // Also check for bell notification
        let (terminal_name, has_bell) = {
            let terminals = self.terminals.lock();
            if let Some(terminal) = terminals.get(terminal_id) {
                let name = if let Some(custom_name) = project.terminal_names.get(terminal_id) {
                    custom_name.clone()
                } else {
                    terminal.title().unwrap_or_else(|| terminal_id.chars().take(8).collect())
                };
                (name, terminal.has_bell())
            } else {
                let name = project.terminal_names.get(terminal_id)
                    .cloned()
                    .unwrap_or_else(|| terminal_id.chars().take(8).collect());
                (name, false)
            }
        };

        // Check if this terminal is being renamed
        let is_renaming = is_renaming(&self.terminal_rename, &(project_id.clone(), terminal_id.to_string()));

        // Check if this terminal is currently focused
        let is_focused = {
            let ws = self.workspace.read(cx);
            ws.focus_manager.focused_terminal_state().map_or(false, |ft| {
                if let Some(proj) = ws.project(&project_id) {
                    proj.layout.as_ref()
                        .and_then(|l| l.find_terminal_path(&terminal_id_for_focus))
                        .map_or(false, |path| ft.project_id == project_id && ft.layout_path == path)
                } else {
                    false
                }
            })
        };

        let terminal_name_for_rename = terminal_name.clone();

        div()
            .id(ElementId::Name(format!("terminal-item-{}", terminal_id).into()))
            .group("terminal-item")
            .h(px(22.0))
            .pl(px(28.0))
            .pr(px(8.0))
            .flex()
            .items_center()
            .gap(px(4.0))
            .cursor_pointer()
            .hover(|s| s.bg(rgb(t.bg_hover)))
            .when(is_minimized, |d| d.opacity(0.5))
            .when(is_focused, |d| d.bg(rgb(t.bg_selection)))
            // Click to focus this terminal
            .on_click({
                let workspace = workspace_for_focus.clone();
                let project_id = project_id_for_focus.clone();
                let terminal_id = terminal_id_for_focus.clone();
                move |_, _window, cx| {
                    workspace.update(cx, |ws, cx| {
                        ws.focus_terminal_by_id(&project_id, &terminal_id, cx);
                    });
                }
            })
            .child(
                // Terminal icon - different for minimized and bell state
                div()
                    .flex_shrink_0()
                    .w(px(14.0))
                    .h(px(14.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        svg()
                            .path(if has_bell {
                                "icons/bell.svg"
                            } else if is_minimized {
                                "icons/terminal-minimized.svg"
                            } else {
                                "icons/terminal.svg"
                            })
                            .size(px(12.0))
                            .text_color(if has_bell {
                                rgb(t.border_bell)
                            } else if is_minimized {
                                rgb(t.text_muted)
                            } else {
                                rgb(t.success)
                            })
                    ),
            )
            .child(
                // Terminal name (or input if renaming)
                if is_renaming {
                    if let Some(input) = rename_input(&self.terminal_rename) {
                        div()
                            .id("terminal-rename-input")
                            .flex_1()
                            .min_w_0()
                            .bg(rgb(t.bg_hover))
                            .rounded(px(2.0))
                            .child(
                                SimpleInput::new(input)
                                    .text_size(px(11.0))
                            )
                            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                cx.stop_propagation();
                            })
                            .on_click(|_, _window, cx| {
                                cx.stop_propagation();
                            })
                            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                                match event.keystroke.key.as_str() {
                                    "enter" => this.finish_rename(cx),
                                    "escape" => this.cancel_rename(cx),
                                    _ => {}
                                }
                            }))
                            .into_any_element()
                    } else {
                        div().flex_1().min_w_0().into_any_element()
                    }
                } else {
                    div()
                        .id(ElementId::Name(format!("terminal-name-{}", terminal_id).into()))
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .text_size(px(11.0))
                        .text_color(rgb(t.text_primary))
                        .text_ellipsis()
                        .child(terminal_name)
                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                            cx.stop_propagation();
                        })
                        .on_click(cx.listener({
                            let workspace = workspace_for_focus.clone();
                            let project_id = project_id_for_rename;
                            let project_id_for_focus = project_id_for_focus.clone();
                            let terminal_id = terminal_id_for_rename;
                            let terminal_id_for_focus = terminal_id_for_focus.clone();
                            let name = terminal_name_for_rename;
                            move |this, _event: &ClickEvent, window, cx| {
                                if this.check_double_click(&terminal_id) {
                                    this.start_rename(project_id.clone(), terminal_id.clone(), name.clone(), window, cx);
                                } else {
                                    // Single click - focus the terminal
                                    workspace.update(cx, |ws, cx| {
                                        ws.focus_terminal_by_id(&project_id_for_focus, &terminal_id_for_focus, cx);
                                    });
                                }
                                cx.stop_propagation();
                            }
                        }))
                        .into_any_element()
                },
            )
            .child(
                // Action buttons - show on hover
                div()
                    .flex()
                    .flex_shrink_0()
                    .gap(px(2.0))
                    .opacity(0.0)
                    .group_hover("terminal-item", |s| s.opacity(1.0))
                    .child(
                        // Minimize/restore button
                        div()
                            .id(ElementId::Name(format!("minimize-{}", terminal_id).into()))
                            .cursor_pointer()
                            .w(px(18.0))
                            .h(px(18.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(3.0))
                            .hover(|s| s.bg(rgb(t.bg_hover)))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                cx.stop_propagation();
                            })
                            .on_click(move |_, _window, cx| {
                                cx.stop_propagation();
                                workspace_for_minimize.update(cx, |ws, cx| {
                                    ws.toggle_terminal_minimized_by_id(&project_id_for_minimize, &terminal_id_for_minimize, cx);
                                });
                            })
                            .child(
                                svg()
                                    .path("icons/minimize.svg")
                                    .size(px(12.0))
                                    .text_color(rgb(t.text_secondary))
                            )
                            .tooltip({
                                let tooltip_text = if is_minimized { "Restore" } else { "Minimize" };
                                move |_window, cx| Tooltip::new(tooltip_text).build(_window, cx)
                            }),
                    )
                    .child(
                        // Fullscreen button
                        div()
                            .id(ElementId::Name(format!("fullscreen-{}", terminal_id).into()))
                            .cursor_pointer()
                            .w(px(18.0))
                            .h(px(18.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(3.0))
                            .hover(|s| s.bg(rgb(t.bg_hover)))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                cx.stop_propagation();
                            })
                            .on_click(move |_, _window, cx| {
                                cx.stop_propagation();
                                workspace.update(cx, |ws, cx| {
                                    ws.set_fullscreen_terminal(
                                        project_id.clone(),
                                        terminal_id_owned.clone(),
                                        cx,
                                    );
                                });
                            })
                            .child(
                                svg()
                                    .path("icons/fullscreen.svg")
                                    .size(px(12.0))
                                    .text_color(rgb(t.text_secondary))
                            )
                            .tooltip(|_window, cx| Tooltip::new("Fullscreen").build(_window, cx)),
                    ),
            )
    }
}

impl Render for Sidebar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let workspace = self.workspace.read(cx);
        // Get projects in order from project_order
        let ordered_projects: Vec<ProjectData> = workspace.data.project_order
            .iter()
            .filter_map(|id| workspace.data.projects.iter().find(|p| &p.id == id).cloned())
            .collect();
        let show_add_dialog = self.show_add_dialog;

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
            .border_r_1()
            .border_color(rgb(t.border))
            .child(self.render_header(cx))
            .when(show_add_dialog, |d| d.child(self.render_add_dialog(window, cx)))
            .child(self.render_projects_header(cx))
            .child(
                div()
                    .id("sidebar-scroll")
                    .flex_1()
                    .overflow_y_scroll()
                    .children(
                        ordered_projects
                            .iter()
                            .enumerate()
                            .map(|(i, p)| self.render_project_item(p, i, window, cx)),
                    ),
            )
            .child(self.render_keybindings_hint(cx))
            // Path suggestions overlay - rendered LAST to appear on top of everything
            .when(show_add_dialog && has_suggestions, |d| {
                d.child(self.render_path_suggestions(cx))
            })
    }
}
