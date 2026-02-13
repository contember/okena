//! Project switcher overlay for quick project navigation.
//!
//! Provides keyboard-driven project switching with:
//! - Enter: Focus the selected project
//! - Space: Toggle project visibility
//! - Type to filter projects

use crate::keybindings::Cancel;
use crate::theme::{theme, with_alpha};
use crate::views::components::{
    badge, handle_list_overlay_key, keyboard_hints_footer, modal_backdrop, modal_content,
    modal_header, search_input_area, substring_filter, ListOverlayAction, ListOverlayConfig,
    ListOverlayState,
};
use crate::workspace::state::{ProjectData, Workspace};
use gpui::*;
use gpui_component::h_flex;
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
    state: ListOverlayState<ProjectData>,
}

impl ProjectSwitcher {
    pub fn new(workspace: Entity<Workspace>, cx: &mut Context<Self>) -> Self {
        // Get all projects from workspace, sorted by recency
        let projects: Vec<ProjectData> = workspace.read(cx).projects_by_recency().into_iter().cloned().collect();

        let config = ListOverlayConfig::new("Switch Project")
            .subtitle("Type to search, Enter to focus, Space to toggle visibility")
            .searchable("Type to filter projects...")
            .size(500.0, 500.0)
            .empty_message("No projects found")
            .keyboard_hints(vec![("Enter", "focus"), ("Space", "toggle visibility"), ("Esc", "close")])
            .key_context("ProjectSwitcher");

        let state = ListOverlayState::new(projects, config, cx);
        let focus_handle = state.focus_handle.clone();

        Self { focus_handle, state }
    }

    fn close(&self, cx: &mut Context<Self>) {
        cx.emit(ProjectSwitcherEvent::Close);
    }

    fn focus_selected(&self, cx: &mut Context<Self>) {
        if let Some(project) = self.state.selected_item() {
            cx.emit(ProjectSwitcherEvent::FocusProject(project.id.clone()));
        }
    }

    fn toggle_visibility_selected(&self, cx: &mut Context<Self>) {
        if let Some(project) = self.state.selected_item() {
            cx.emit(ProjectSwitcherEvent::ToggleVisibility(project.id.clone()));
        }
    }

    fn filter_projects(&mut self) {
        let filtered = substring_filter(&self.state.items, &self.state.search_query, |p| {
            vec![p.name.clone(), p.path.clone()]
        });
        self.state.set_filtered(filtered);
    }

    fn render_project_row(
        &self,
        display_index: usize,
        project_index: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let t = theme(cx);
        let project = &self.state.items[project_index];
        let is_selected = display_index == self.state.selected_index;
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
                    this.state.selected_index = display_index;
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
                        h_flex()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .text_size(px(13.0))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(rgb(t.text_primary))
                                    .child(name),
                            )
                            .when(is_worktree, |d| {
                                d.child(badge("worktree", &t))
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
        let search_query = self.state.search_query.clone();
        let config_width = self.state.config.width;
        let config_max_height = self.state.config.max_height;
        let config_title = self.state.config.title.clone();
        let config_subtitle = self.state.config.subtitle.clone();
        let search_placeholder = self.state.config.search_placeholder.clone().unwrap_or_default();
        let empty_message = self.state.config.empty_message.clone();

        if !focus_handle.is_focused(window) {
            window.focus(&focus_handle, cx);
        }

        modal_backdrop("project-switcher-backdrop", &t)
            .track_focus(&focus_handle)
            .key_context("ProjectSwitcher")
            .items_start()
            .pt(px(80.0))
            .on_action(cx.listener(|this, _: &Cancel, _window, cx| {
                this.close(cx);
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                match handle_list_overlay_key(&mut this.state, event, &[("space", "toggle")]) {
                    ListOverlayAction::Close => this.close(cx),
                    ListOverlayAction::SelectPrev | ListOverlayAction::SelectNext => {
                        this.state.scroll_to_selected();
                        cx.notify();
                    }
                    ListOverlayAction::Confirm => this.focus_selected(cx),
                    ListOverlayAction::QueryChanged => {
                        this.filter_projects();
                        cx.notify();
                    }
                    ListOverlayAction::Custom(action) if action == "toggle" => {
                        this.toggle_visibility_selected(cx);
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
                    .w(px(config_width))
                    .max_h(px(config_max_height))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(modal_header(
                        config_title,
                        config_subtitle,
                        &t,
                        cx.listener(|this, _, _window, cx| this.close(cx)),
                    ))
                    .child(search_input_area(&search_query, &search_placeholder, &t))
                    .child(
                        // Project list
                        div()
                            .id("project-list")
                            .flex_1()
                            .overflow_y_scroll()
                            .track_scroll(&self.state.scroll_handle)
                            .children(self.state.filtered.iter().enumerate().map(
                                |(display_idx, filter_result)| {
                                    self.render_project_row(display_idx, filter_result.index, cx)
                                },
                            ))
                            .when(self.state.is_empty(), |d| {
                                d.child(
                                    div()
                                        .px(px(12.0))
                                        .py(px(20.0))
                                        .text_size(px(13.0))
                                        .text_color(rgb(t.text_muted))
                                        .child(empty_message.clone()),
                                )
                            }),
                    )
                    .child(keyboard_hints_footer(&[("Enter", "focus"), ("Space", "toggle visibility"), ("Esc", "close")], &t)),
            )
    }
}

impl_focusable!(ProjectSwitcher);
