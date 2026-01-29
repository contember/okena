//! Add project dialog and path suggestions for the sidebar

use crate::theme::theme;
use crate::views::components::{button, button_primary, input_container, labeled_input, SimpleInput};
use gpui::prelude::*;
use gpui::*;

use super::Sidebar;

impl Sidebar {
    pub(super) fn render_add_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
                labeled_input("Name:", &t)
                    .child(
                        input_container(&t, None)
                            .child(SimpleInput::new(&name_input).text_size(px(12.0))),
                    ),
            )
            .child(
                // Path input with auto-complete
                labeled_input("Path (Tab to complete):", &t)
                    .child(path_input),
            )
            .child(
                // Browse button
                button("browse-folder-btn", "Browse...", &t)
                    .px(px(8.0))
                    .py(px(4.0))
                    .text_size(px(11.0))
                    .text_color(rgb(t.text_primary))
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
                        button("quick-add-home", "Home (~)", &t)
                            .px(px(8.0))
                            .py(px(4.0))
                            .text_size(px(11.0))
                            .text_color(rgb(t.text_primary))
                            .on_click(cx.listener(|this, _, window, cx| {
                                let path = dirs::home_dir()
                                    .map(|p| p.to_string_lossy().to_string())
                                    .unwrap_or_else(|| "/home".to_string());
                                this.set_quick_path("Home", &path, window, cx);
                            })),
                    )
                    .child(
                        button("quick-add-tmp", "Tmp (/tmp)", &t)
                            .px(px(8.0))
                            .py(px(4.0))
                            .text_size(px(11.0))
                            .text_color(rgb(t.text_primary))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.set_quick_path("Tmp", "/tmp", window, cx);
                            })),
                    )
                    .child(
                        button("quick-add-projects", "Projects", &t)
                            .px(px(8.0))
                            .py(px(4.0))
                            .text_size(px(11.0))
                            .text_color(rgb(t.text_primary))
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
                        button("cancel-add-btn", "Cancel", &t)
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
                        button_primary("confirm-add-btn", "Add", &t)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.add_project(window, cx);
                            })),
                    ),
            )
    }

    /// Render path auto-complete suggestions dropdown
    pub(super) fn render_path_suggestions(&self, cx: &mut Context<Self>) -> impl IntoElement {
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
            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                cx.stop_propagation();
            })
            .on_scroll_wheel(|_, _, cx| {
                cx.stop_propagation();
            })
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
}
