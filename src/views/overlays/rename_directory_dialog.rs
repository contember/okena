use crate::keybindings::Cancel;
use crate::theme::theme;
use crate::views::components::{button, button_primary, input_container, modal_backdrop, modal_content};
use crate::views::components::simple_input::{SimpleInput, SimpleInputState};
use crate::workspace::state::Workspace;
use gpui::prelude::*;
use gpui::*;
use gpui_component::h_flex;
use std::path::Path;

/// Events emitted by the rename directory dialog
#[derive(Clone)]
pub enum RenameDirectoryDialogEvent {
    /// Dialog closed (cancelled or renamed)
    Close,
    /// Directory was successfully renamed
    Renamed,
}

impl EventEmitter<RenameDirectoryDialogEvent> for RenameDirectoryDialog {}

/// Dialog for renaming a project's directory on disk.
pub struct RenameDirectoryDialog {
    workspace: Entity<Workspace>,
    project_id: String,
    project_path: String,
    name_input: Entity<SimpleInputState>,
    error_message: Option<String>,
    focus_handle: FocusHandle,
    initialized: bool,
}

impl RenameDirectoryDialog {
    pub fn new(
        workspace: Entity<Workspace>,
        project_id: String,
        project_path: String,
        cx: &mut Context<Self>,
    ) -> Self {
        let current_name = Path::new(&project_path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();

        let name_input = cx.new(|cx| {
            let mut input = SimpleInputState::new(cx)
                .placeholder("Directory name...");
            input.set_value(&current_name, cx);
            input
        });

        Self {
            workspace,
            project_id,
            project_path,
            name_input,
            error_message: None,
            focus_handle: cx.focus_handle(),
            initialized: false,
        }
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        cx.emit(RenameDirectoryDialogEvent::Close);
    }

    fn confirm(&mut self, cx: &mut Context<Self>) {
        let new_name = self.name_input.read(cx).value().trim().to_string();

        // Validate: non-empty
        if new_name.is_empty() {
            self.error_message = Some("Directory name cannot be empty".to_string());
            cx.notify();
            return;
        }

        // Validate: no path separators
        if new_name.contains('/') || new_name.contains('\\') {
            self.error_message = Some("Directory name cannot contain path separators".to_string());
            cx.notify();
            return;
        }

        // Get current directory name
        let old_path = Path::new(&self.project_path);
        let current_name = old_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");

        // Validate: not same as current
        if new_name == current_name {
            self.error_message = Some("Name is the same as current directory".to_string());
            cx.notify();
            return;
        }

        // Build new path
        let new_path = match old_path.parent() {
            Some(parent) => parent.join(&new_name),
            None => {
                self.error_message = Some("Cannot determine parent directory".to_string());
                cx.notify();
                return;
            }
        };

        // Check new path doesn't already exist
        if new_path.exists() {
            self.error_message = Some(format!("'{}' already exists", new_name));
            cx.notify();
            return;
        }

        // Rename on disk
        if let Err(e) = std::fs::rename(&self.project_path, &new_path) {
            self.error_message = Some(format!("Failed to rename: {}", e));
            cx.notify();
            return;
        }

        // Update workspace
        let new_path_str = new_path.to_string_lossy().to_string();
        let project_id = self.project_id.clone();
        let new_name_clone = new_name.clone();
        self.workspace.update(cx, |ws, cx| {
            ws.rename_project_directory(&project_id, new_path_str, new_name_clone, cx);
        });

        cx.emit(RenameDirectoryDialogEvent::Renamed);
    }
}

impl_focusable!(RenameDirectoryDialog);

impl Render for RenameDirectoryDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let focus_handle = self.focus_handle.clone();

        // Focus input on first render
        if !self.initialized {
            self.initialized = true;
            self.name_input.update(cx, |input, cx| {
                input.focus(window, cx);
            });
        }

        let name_input = self.name_input.clone();
        let input_focused = self.name_input.read(cx).focus_handle(cx).is_focused(window);
        let error_msg = self.error_message.clone();

        // Show full path as context
        let path_display = self.project_path.clone();

        modal_backdrop("rename-dir-dialog-backdrop", &t)
            .track_focus(&focus_handle)
            .key_context("RenameDirectoryDialog")
            .items_center()
            .on_action(cx.listener(|this, _: &Cancel, _, cx| {
                this.close(cx);
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key.as_str() == "enter" {
                    this.confirm(cx);
                }
            }))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                this.close(cx);
            }))
            .child(
                modal_content("rename-dir-dialog", &t)
                    .w(px(420.0))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    // Header
                    .child(
                        div()
                            .px(px(16.0))
                            .py(px(12.0))
                            .flex()
                            .items_center()
                            .justify_between()
                            .border_b_1()
                            .border_color(rgb(t.border))
                            .child(
                                h_flex()
                                    .gap(px(8.0))
                                    .child(
                                        svg()
                                            .path("icons/folder.svg")
                                            .size(px(16.0))
                                            .text_color(rgb(t.border_active)),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(14.0))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(t.text_primary))
                                            .child("Rename Directory"),
                                    ),
                            )
                            .child(
                                div()
                                    .id("close-rename-dir-btn")
                                    .cursor_pointer()
                                    .w(px(24.0))
                                    .h(px(24.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(4.0))
                                    .hover(|s| s.bg(rgb(t.bg_hover)))
                                    .child(
                                        svg()
                                            .path("icons/close.svg")
                                            .size(px(14.0))
                                            .text_color(rgb(t.text_secondary)),
                                    )
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.close(cx);
                                    })),
                            ),
                    )
                    // Content
                    .child(
                        div()
                            .px(px(16.0))
                            .py(px(12.0))
                            .flex()
                            .flex_col()
                            .gap(px(8.0))
                            // Current path info
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(rgb(t.text_muted))
                                    .overflow_x_hidden()
                                    .whitespace_nowrap()
                                    .child(path_display),
                            )
                            // Name input
                            .child(
                                input_container(&t, Some(input_focused))
                                    .child(SimpleInput::new(&name_input).text_size(px(13.0))),
                            ),
                    )
                    // Error message
                    .when_some(error_msg, |d, msg| {
                        d.child(
                            div()
                                .px(px(16.0))
                                .py(px(8.0))
                                .bg(rgba(0xff00001a))
                                .text_size(px(12.0))
                                .text_color(rgb(t.error))
                                .child(msg),
                        )
                    })
                    // Footer
                    .child(
                        div()
                            .px(px(16.0))
                            .py(px(12.0))
                            .flex()
                            .justify_end()
                            .gap(px(8.0))
                            .border_t_1()
                            .border_color(rgb(t.border))
                            .child(
                                button("cancel-rename-dir-btn", "Cancel", &t)
                                    .px(px(16.0))
                                    .py(px(8.0))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.close(cx);
                                    })),
                            )
                            .child(
                                button_primary("confirm-rename-dir-btn", "Rename", &t)
                                    .px(px(16.0))
                                    .py(px(8.0))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.confirm(cx);
                                    })),
                            ),
                    ),
            )
    }
}
