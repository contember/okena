//! Add project modal dialog overlay.

use crate::keybindings::Cancel;
use crate::remote_client::manager::RemoteConnectionManager;
use crate::theme::theme;
use crate::views::components::{
    button, button_primary, input_container, labeled_input, modal_backdrop, modal_content,
    modal_header, PathAutoCompleteState, SimpleInput, SimpleInputState,
};
use crate::workspace::state::Workspace;
use gpui::prelude::*;
use gpui::*;
use gpui_component::v_flex;
use okena_core::api::ActionRequest;
use okena_core::client::ConnectionStatus;

enum AddProjectTarget {
    Local,
    Remote {
        connection_id: String,
        connection_name: String,
    },
}

pub struct AddProjectDialog {
    workspace: Entity<Workspace>,
    remote_manager: Option<Entity<RemoteConnectionManager>>,
    focus_handle: FocusHandle,
    name_input: Entity<SimpleInputState>,
    path_input: Entity<PathAutoCompleteState>,
    pending_name_value: Option<String>,
    pending_path_value: Option<String>,
    initial_focus_done: bool,
    targets: Vec<AddProjectTarget>,
    selected_target: usize,
}

pub enum AddProjectDialogEvent {
    Close,
}

impl EventEmitter<AddProjectDialogEvent> for AddProjectDialog {}

impl AddProjectDialog {
    pub fn new(
        workspace: Entity<Workspace>,
        remote_manager: Option<Entity<RemoteConnectionManager>>,
        cx: &mut Context<Self>,
    ) -> Self {
        let name_input = cx.new(|cx| SimpleInputState::new(cx).placeholder("Enter project name..."));
        let path_input = cx.new(|cx| PathAutoCompleteState::new(cx));

        // Build targets list: Local + connected remote connections
        let mut targets = vec![AddProjectTarget::Local];
        if let Some(ref rm) = remote_manager {
            let rm = rm.read(cx);
            for (config, status, _state) in rm.connections() {
                if matches!(status, ConnectionStatus::Connected) {
                    targets.push(AddProjectTarget::Remote {
                        connection_id: config.id.clone(),
                        connection_name: config.name.clone(),
                    });
                }
            }
        }

        Self {
            workspace,
            remote_manager,
            focus_handle: cx.focus_handle(),
            name_input,
            path_input,
            pending_name_value: None,
            pending_path_value: None,
            initial_focus_done: false,
            targets,
            selected_target: 0,
        }
    }

    fn close(&self, cx: &mut Context<Self>) {
        cx.emit(AddProjectDialogEvent::Close);
    }

    fn is_remote_target(&self) -> bool {
        matches!(
            self.targets.get(self.selected_target),
            Some(AddProjectTarget::Remote { .. })
        )
    }

    fn add_project(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let name = self.name_input.read(cx).value().to_string();
        let path = self.path_input.read(cx).value(cx);

        if name.is_empty() || path.is_empty() {
            return;
        }

        match self.targets.get(self.selected_target) {
            Some(AddProjectTarget::Local) | None => {
                self.workspace.update(cx, |ws, cx| {
                    ws.add_project(name, path, true, cx);
                });
            }
            Some(AddProjectTarget::Remote {
                connection_id, ..
            }) => {
                if let Some(ref rm) = self.remote_manager {
                    let cid = connection_id.clone();
                    rm.update(cx, |rm, cx| {
                        rm.send_action(
                            &cid,
                            ActionRequest::AddProject { name, path },
                            cx,
                        );
                    });
                }
            }
        }

        self.close(cx);
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
                    let name_str = path
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| "Project".to_string());

                    this.update(cx, |this, cx| {
                        this.pending_path_value = Some(path_str);
                        this.pending_name_value = Some(name_str);
                        cx.notify();
                    })
                    .ok();
                }
            }
        })
        .detach();
    }

    fn render_target_selector(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        div()
            .flex()
            .gap(px(6.0))
            .children(self.targets.iter().enumerate().map(|(i, target)| {
                let is_selected = i == self.selected_target;
                let label = match target {
                    AddProjectTarget::Local => "Local".to_string(),
                    AddProjectTarget::Remote {
                        connection_name, ..
                    } => connection_name.clone(),
                };

                div()
                    .id(ElementId::Name(format!("target-{}", i).into()))
                    .px(px(10.0))
                    .py(px(4.0))
                    .text_size(px(11.0))
                    .rounded(px(4.0))
                    .cursor_pointer()
                    .when(is_selected, |d| {
                        d.bg(rgb(t.border_active))
                            .text_color(rgb(t.bg_primary))
                    })
                    .when(!is_selected, |d| {
                        d.bg(rgb(t.bg_secondary))
                            .text_color(rgb(t.text_muted))
                            .hover(|s| s.bg(rgb(t.bg_hover)))
                    })
                    .child(label)
                    .on_click(cx.listener(move |this, _, _window, cx| {
                        this.selected_target = i;
                        cx.notify();
                    }))
            }))
    }

    fn render_path_suggestions(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let path_input = self.path_input.clone();

        let state = path_input.read(cx);
        let suggestions: Vec<_> = state.suggestions().to_vec();
        let selected_index = state.selected_index();
        let scroll_handle = state.suggestions_scroll().clone();

        if suggestions.is_empty() {
            return div().into_any_element();
        }

        // Adjust top offset when target selector is visible
        let top_offset = if self.targets.len() > 1 {
            210.0
        } else {
            180.0
        };

        div()
            .absolute()
            // Position below the path input inside the modal content
            .top(px(top_offset))
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
                v_flex().children(
                    suggestions
                        .iter()
                        .enumerate()
                        .map(|(i, suggestion)| {
                            let is_selected = i == selected_index;
                            let path_input = path_input.clone();

                            div()
                                .id(ElementId::Name(
                                    format!("path-suggestion-{}", i).into(),
                                ))
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
                                        .path(if suggestion.is_select_current {
                                            "icons/check.svg"
                                        } else if suggestion.is_directory {
                                            "icons/folder.svg"
                                        } else {
                                            "icons/file.svg"
                                        })
                                        .size(px(14.0))
                                        .text_color(if suggestion.is_select_current {
                                            rgb(t.border_active)
                                        } else if suggestion.is_directory {
                                            rgb(t.border_active)
                                        } else {
                                            rgb(t.text_muted)
                                        }),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.0))
                                        .text_color(if suggestion.is_select_current {
                                            rgb(t.border_active)
                                        } else {
                                            rgb(t.text_primary)
                                        })
                                        .child(suggestion.display_name.clone()),
                                )
                                .on_click(move |_, _window, cx| {
                                    path_input.update(cx, |state, cx| {
                                        state.select_and_complete(i, cx);
                                    });
                                })
                        }),
                ),
            )
            .into_any_element()
    }
}

impl Render for AddProjectDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let focus_handle = self.focus_handle.clone();

        // Only focus the name input on first render, not on every re-render
        if !self.initial_focus_done {
            self.initial_focus_done = true;
            self.name_input.update(cx, |input, cx| {
                input.focus(window, cx);
            });
        }

        // Apply pending values from async operations
        if let Some(name_value) = self.pending_name_value.take() {
            self.name_input
                .update(cx, |i, cx| i.set_value(&name_value, cx));
        }
        if let Some(path_value) = self.pending_path_value.take() {
            self.path_input
                .update(cx, |i, cx| i.set_value_quiet(&path_value, cx));
        }

        let is_remote = self.is_remote_target();
        let has_suggestions = !is_remote && self.path_input.read(cx).has_suggestions();
        let has_multiple_targets = self.targets.len() > 1;

        let path_label = if is_remote {
            "Path:"
        } else {
            "Path (Tab to complete):"
        };

        modal_backdrop("add-project-backdrop", &t)
            .track_focus(&focus_handle)
            .key_context("AddProjectDialog")
            .items_center()
            .on_action(cx.listener(|this, _: &Cancel, _, cx| {
                this.close(cx);
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.close(cx);
                }),
            )
            .child(
                modal_content("add-project-modal", &t)
                    .relative()
                    .w(px(450.0))
                    .child(modal_header(
                        "Add Project",
                        None::<&str>,
                        &t,
                        cx.listener(|this, _, _, cx| this.close(cx)),
                    ))
                    .child(
                        div()
                            .p(px(16.0))
                            .flex()
                            .flex_col()
                            .gap(px(12.0))
                            // Target selector (only when multiple targets available)
                            .when(has_multiple_targets, |d| {
                                d.child(
                                    labeled_input("Target:", &t)
                                        .child(self.render_target_selector(cx)),
                                )
                            })
                            // Name input
                            .child(
                                labeled_input("Name:", &t).child(
                                    input_container(&t, None).child(
                                        SimpleInput::new(&self.name_input).text_size(px(12.0)),
                                    ),
                                ),
                            )
                            // Path input with auto-complete (or plain input for remote)
                            .child(
                                labeled_input(path_label, &t)
                                    .when(!is_remote, |d| {
                                        d.child(self.path_input.clone())
                                    })
                                    .when(is_remote, |d| {
                                        d.child(
                                            input_container(&t, None).child(
                                                SimpleInput::new(
                                                    self.path_input.read(cx).input(),
                                                )
                                                .text_size(px(12.0)),
                                            ),
                                        )
                                    }),
                            )
                            // Browse button (only for local target)
                            .when(!is_remote, |d| {
                                d.child(
                                    button("browse-folder-btn", "Browse...", &t)
                                        .px(px(8.0))
                                        .py(px(4.0))
                                        .text_size(px(11.0))
                                        .text_color(rgb(t.text_primary))
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.open_folder_picker(window, cx);
                                        })),
                                )
                            })
                            // Action buttons
                            .child(
                                div()
                                    .flex()
                                    .gap(px(8.0))
                                    .justify_end()
                                    .child(
                                        button("cancel-add-btn", "Cancel", &t).on_click(
                                            cx.listener(|this, _, _window, cx| {
                                                this.close(cx);
                                            }),
                                        ),
                                    )
                                    .child(
                                        button_primary("confirm-add-btn", "Add", &t).on_click(
                                            cx.listener(|this, _, window, cx| {
                                                this.add_project(window, cx);
                                            }),
                                        ),
                                    ),
                            ),
                    )
                    // Path suggestions overlay (only for local target)
                    .when(has_suggestions, |d| {
                        d.child(self.render_path_suggestions(cx))
                    }),
            )
    }
}

impl_focusable!(AddProjectDialog);
