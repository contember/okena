//! Project switcher overlay for quick project navigation.
//!
//! Provides keyboard-driven project switching with:
//! - Enter: Focus the selected project
//! - Space: Toggle project visibility
//! - Type to filter projects

use crate::theme::{theme, with_alpha};
use crate::views::components::{modal_backdrop, modal_content, modal_header};
use crate::workspace::state::{ProjectData, Workspace};
use gpui::*;
use gpui::prelude::*;

/// Events emitted by the ProjectSwitcher overlay.
#[derive(Clone)]
pub enum ProjectSwitcherEvent {
    /// Close the overlay
    Close,
    /// Focus a specific project (makes it the only visible one)
    FocusProject(String),
    /// Toggle visibility of a project
    ToggleVisibility(String),
}

impl EventEmitter<ProjectSwitcherEvent> for ProjectSwitcher {}

/// Project switcher overlay for quick project navigation.
pub struct ProjectSwitcher {
    focus_handle: FocusHandle,
    scroll_handle: ScrollHandle,
    /// All projects (snapshot at creation time)
    projects: Vec<ProjectData>,
    /// Indices into `projects` that match the current filter
    filtered_indices: Vec<usize>,
    /// Currently selected index (into filtered_indices)
    selected_index: usize,
    /// Search query for filtering
    search_query: String,
}

impl ProjectSwitcher {
    pub fn new(workspace: Entity<Workspace>, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        let scroll_handle = ScrollHandle::new();

        // Get all projects from workspace, sorted by recency
        let projects: Vec<ProjectData> = workspace.read(cx).projects_by_recency().into_iter().cloned().collect();
        let filtered_indices: Vec<usize> = (0..projects.len()).collect();

        Self {
            focus_handle,
            scroll_handle,
            projects,
            filtered_indices,
            selected_index: 0,
            search_query: String::new(),
        }
    }

    fn close(&self, cx: &mut Context<Self>) {
        cx.emit(ProjectSwitcherEvent::Close);
    }

    fn focus_selected(&self, cx: &mut Context<Self>) {
        if let Some(&project_index) = self.filtered_indices.get(self.selected_index) {
            if let Some(project) = self.projects.get(project_index) {
                cx.emit(ProjectSwitcherEvent::FocusProject(project.id.clone()));
            }
        }
    }

    fn toggle_visibility_selected(&self, cx: &mut Context<Self>) {
        if let Some(&project_index) = self.filtered_indices.get(self.selected_index) {
            if let Some(project) = self.projects.get(project_index) {
                cx.emit(ProjectSwitcherEvent::ToggleVisibility(project.id.clone()));
            }
        }
    }

    fn filter_projects(&mut self) {
        let query = self.search_query.to_lowercase();

        if query.is_empty() {
            self.filtered_indices = (0..self.projects.len()).collect();
        } else {
            self.filtered_indices = self
                .projects
                .iter()
                .enumerate()
                .filter(|(_, p)| {
                    p.name.to_lowercase().contains(&query)
                        || p.path.to_lowercase().contains(&query)
                })
                .map(|(i, _)| i)
                .collect();
        }

        // Reset selection to first item
        self.selected_index = 0;
    }

    fn scroll_to_selected(&self) {
        if !self.filtered_indices.is_empty() {
            self.scroll_handle.scroll_to_item(self.selected_index);
        }
    }

    fn render_project_row(
        &self,
        display_index: usize,
        project_index: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = theme(cx);
        let project = &self.projects[project_index];
        let is_selected = display_index == self.selected_index;
        let name = project.name.clone();
        let path = project.path.clone();
        let is_visible = project.is_visible;
        let is_worktree = project.worktree_info.is_some();
        let folder_color = t.get_folder_color(project.folder_color);

        div()
            .id(ElementId::Name(format!("project-{}", display_index).into()))
            .cursor_pointer()
            .flex()
            .items_center()
            .gap(px(12.0))
            .px(px(12.0))
            .py(px(10.0))
            .border_b_1()
            .border_color(rgb(t.border))
            .when(is_selected, |d| d.bg(with_alpha(t.border_active, 0.15)))
            .hover(|s| s.bg(rgb(t.bg_hover)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _window, cx| {
                    this.selected_index = display_index;
                    this.focus_selected(cx);
                }),
            )
            .child(
                // Folder icon with project color
                div()
                    .w(px(20.0))
                    .h(px(20.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        svg()
                            .path("icons/folder.svg")
                            .size(px(16.0))
                            .text_color(rgb(folder_color)),
                    ),
            )
            .child(
                // Project info
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .overflow_hidden()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .text_size(px(13.0))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(rgb(t.text_primary))
                                    .child(name),
                            )
                            .when(is_worktree, |d| {
                                d.child(
                                    div()
                                        .px(px(5.0))
                                        .py(px(1.0))
                                        .rounded(px(3.0))
                                        .bg(rgb(t.bg_secondary))
                                        .text_size(px(9.0))
                                        .text_color(rgb(t.text_muted))
                                        .child("worktree"),
                                )
                            }),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(rgb(t.text_muted))
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(path),
                    ),
            )
            .child(
                // Visibility indicator
                div()
                    .w(px(20.0))
                    .h(px(20.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        svg()
                            .path(if is_visible {
                                "icons/eye.svg"
                            } else {
                                "icons/eye-off.svg"
                            })
                            .size(px(14.0))
                            .text_color(if is_visible {
                                rgb(t.text_secondary)
                            } else {
                                rgb(t.text_muted)
                            }),
                    ),
            )
    }
}

impl Render for ProjectSwitcher {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let focus_handle = self.focus_handle.clone();
        let filtered_indices = self.filtered_indices.clone();
        let search_query = self.search_query.clone();

        window.focus(&focus_handle, cx);

        modal_backdrop("project-switcher-backdrop", &t)
            .track_focus(&focus_handle)
            .key_context("ProjectSwitcher")
            .items_start()
            .pt(px(80.0))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                match event.keystroke.key.as_str() {
                    "escape" => this.close(cx),
                    "up" => {
                        if this.selected_index > 0 {
                            this.selected_index -= 1;
                            this.scroll_to_selected();
                            cx.notify();
                        }
                    }
                    "down" => {
                        if this.selected_index < this.filtered_indices.len().saturating_sub(1) {
                            this.selected_index += 1;
                            this.scroll_to_selected();
                            cx.notify();
                        }
                    }
                    "enter" => {
                        this.focus_selected(cx);
                    }
                    "space" => {
                        this.toggle_visibility_selected(cx);
                    }
                    "backspace" => {
                        if !this.search_query.is_empty() {
                            this.search_query.pop();
                            this.filter_projects();
                            cx.notify();
                        }
                    }
                    key if key.len() == 1 => {
                        let ch = key.chars().next().unwrap();
                        if ch.is_alphanumeric() || ch == '-' || ch == '_' || ch == '/' || ch == '.' {
                            this.search_query.push(ch);
                            this.filter_projects();
                            cx.notify();
                        }
                    }
                    _ => {}
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _window, cx| this.close(cx)),
            )
            .child(
                modal_content("project-switcher-modal", &t)
                    .w(px(500.0))
                    .max_h(px(500.0))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(modal_header(
                        "Switch Project",
                        Some("Type to search, Enter to focus, Space to toggle visibility"),
                        &t,
                        cx.listener(|this, _, _window, cx| this.close(cx)),
                    ))
                    .child(
                        // Search input display
                        div()
                            .px(px(12.0))
                            .py(px(10.0))
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .border_b_1()
                            .border_color(rgb(t.border))
                            .child(
                                div()
                                    .text_size(px(14.0))
                                    .text_color(rgb(t.text_muted))
                                    .child(">"),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .text_size(px(14.0))
                                    .text_color(if search_query.is_empty() {
                                        rgb(t.text_muted)
                                    } else {
                                        rgb(t.text_primary)
                                    })
                                    .child(if search_query.is_empty() {
                                        "Type to filter projects...".to_string()
                                    } else {
                                        search_query
                                    }),
                            ),
                    )
                    .child(
                        // Project list
                        div()
                            .id("project-list")
                            .flex_1()
                            .overflow_y_scroll()
                            .track_scroll(&self.scroll_handle)
                            .children(filtered_indices.iter().enumerate().map(
                                |(display_idx, &project_idx)| {
                                    self.render_project_row(display_idx, project_idx, cx)
                                },
                            ))
                            .when(filtered_indices.is_empty(), |d| {
                                d.child(
                                    div()
                                        .px(px(12.0))
                                        .py(px(20.0))
                                        .text_size(px(13.0))
                                        .text_color(rgb(t.text_muted))
                                        .child("No projects found"),
                                )
                            }),
                    )
                    .child(
                        // Footer with keyboard hints
                        div()
                            .px(px(12.0))
                            .py(px(8.0))
                            .border_t_1()
                            .border_color(rgb(t.border))
                            .flex()
                            .items_center()
                            .gap(px(16.0))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(4.0))
                                    .child(
                                        div()
                                            .px(px(4.0))
                                            .py(px(1.0))
                                            .rounded(px(3.0))
                                            .bg(rgb(t.bg_secondary))
                                            .text_size(px(10.0))
                                            .text_color(rgb(t.text_muted))
                                            .child("Enter"),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(10.0))
                                            .text_color(rgb(t.text_muted))
                                            .child("focus"),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(4.0))
                                    .child(
                                        div()
                                            .px(px(4.0))
                                            .py(px(1.0))
                                            .rounded(px(3.0))
                                            .bg(rgb(t.bg_secondary))
                                            .text_size(px(10.0))
                                            .text_color(rgb(t.text_muted))
                                            .child("Space"),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(10.0))
                                            .text_color(rgb(t.text_muted))
                                            .child("toggle visibility"),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(4.0))
                                    .child(
                                        div()
                                            .px(px(4.0))
                                            .py(px(1.0))
                                            .rounded(px(3.0))
                                            .bg(rgb(t.bg_secondary))
                                            .text_size(px(10.0))
                                            .text_color(rgb(t.text_muted))
                                            .child("Esc"),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(10.0))
                                            .text_color(rgb(t.text_muted))
                                            .child("close"),
                                    ),
                            ),
                    ),
            )
    }
}

impl_focusable!(ProjectSwitcher);
