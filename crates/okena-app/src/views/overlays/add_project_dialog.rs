//! Add project modal dialog overlay.

use crate::keybindings::Cancel;
use crate::remote_client::manager::RemoteConnectionManager;
use crate::theme::theme;
use crate::ui::tokens::{ui_text_md, ui_text_ms};
use crate::views::components::{
    PathAutoCompleteState, SimpleInput, SimpleInputState, button, dropdown_anchored_below,
    input_container, labeled_input, modal_backdrop, modal_content, modal_header,
};
use crate::workspace::state::{WindowId, Workspace};
use gpui::prelude::*;
use gpui::*;
use gpui_component::v_flex;
use okena_core::api::ActionRequest;
use okena_transport::client::{ConnectionStatus, LOCAL_DAEMON_CONNECTION_ID};
use okena_ui::dialog_actions::dialog_actions;
use okena_ui::simple_input::InputChangedEvent;

enum AddProjectTarget {
    Local,
    Remote {
        connection_id: String,
        connection_name: String,
    },
}

/// Where the project's directory comes from.
#[derive(Clone, Copy, PartialEq, Eq)]
enum AddProjectSource {
    /// A directory that already exists on the target host.
    Folder,
    /// A repository to clone into a directory that does not exist yet.
    Git,
}

pub struct AddProjectDialog {
    workspace: Entity<Workspace>,
    /// Spawning window for the multi-window new-project visibility rule
    /// (PRD user story 14): the new project lands visible in this window
    /// only, hidden in every other window. Threaded from the originating
    /// `WindowView` through `OverlayManager::toggle_add_project_dialog`.
    window_id: WindowId,
    remote_manager: Option<Entity<RemoteConnectionManager>>,
    focus_handle: FocusHandle,
    source: AddProjectSource,
    name_input: Entity<SimpleInputState>,
    /// Folder mode: the project directory. Git mode: the parent to clone into.
    path_input: Entity<PathAutoCompleteState>,
    /// Window-absolute bounds of the path input, captured during paint so the
    /// completion list can be anchored to it instead of a guessed offset.
    path_input_bounds: Option<Bounds<Pixels>>,
    url_input: Entity<SimpleInputState>,
    /// Git mode: the directory name created inside the parent.
    directory_input: Entity<SimpleInputState>,
    /// The last values this dialog derived into the directory / name inputs. A
    /// field still holding its derived value counts as untouched and keeps
    /// following the URL; once the user types their own, the auto-fill stops
    /// overwriting it.
    derived_directory: String,
    derived_name: String,
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
        window_id: WindowId,
        cx: &mut Context<Self>,
    ) -> Self {
        let name_input =
            cx.new(|cx| SimpleInputState::new(cx).placeholder("Enter project name..."));
        let path_input = cx.new(PathAutoCompleteState::new);
        let url_input = cx.new(|cx| {
            SimpleInputState::new(cx).placeholder("https://github.com/user/repo.git")
        });
        let directory_input =
            cx.new(|cx| SimpleInputState::new(cx).placeholder("Folder name..."));

        // Typing a URL fills in the directory and the name; editing the
        // directory keeps the name in step. Subscribed to the change event, not
        // to notify — the cursor blink notifies twice a second, and re-running a
        // fill on those would keep resetting the caret to the end of the field.
        cx.subscribe(
            &url_input,
            |this: &mut Self, _, _: &InputChangedEvent, cx| this.derive_from_url(cx),
        )
        .detach();
        cx.subscribe(
            &directory_input,
            |this: &mut Self, _, _: &InputChangedEvent, cx| this.derive_name_from_directory(cx),
        )
        .detach();

        // Build targets list: Local (the implicit loopback local-daemon
        // connection) + connected remote connections. The local-daemon
        // connection itself is hidden from the remote list — "Local" already
        // represents it.
        let mut targets = vec![AddProjectTarget::Local];
        if let Some(ref rm) = remote_manager {
            let rm = rm.read(cx);
            for (config, status, _state) in rm.connections() {
                if config.id == LOCAL_DAEMON_CONNECTION_ID {
                    continue;
                }
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
            window_id,
            remote_manager,
            focus_handle: cx.focus_handle(),
            source: AddProjectSource::Folder,
            name_input,
            path_input,
            path_input_bounds: None,
            url_input,
            directory_input,
            derived_directory: String::new(),
            derived_name: String::new(),
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

    fn is_git_source(&self) -> bool {
        self.source == AddProjectSource::Git
    }

    /// Fill the directory (and, through it, the name) from the URL, for as long
    /// as the user has not overridden them.
    fn derive_from_url(&mut self, cx: &mut Context<Self>) {
        let url = self.url_input.read(cx).value().to_string();
        let derived = okena_git::clone_dir_name(&url).unwrap_or_default();
        let current = self.directory_input.read(cx).value();
        // A field holding anything other than what we put there is the user's.
        if current != self.derived_directory || current == derived {
            return;
        }
        self.derived_directory = derived.clone();
        self.directory_input
            .update(cx, |input, cx| input.set_value(derived, cx));
    }

    /// Keep the project name equal to the directory name until the user gives
    /// the project a name of its own.
    fn derive_name_from_directory(&mut self, cx: &mut Context<Self>) {
        let directory = self.directory_input.read(cx).value().to_string();
        let current = self.name_input.read(cx).value();
        if current != self.derived_name || current == directory {
            return;
        }
        self.derived_name = directory.clone();
        self.name_input
            .update(cx, |input, cx| input.set_value(directory, cx));
    }

    /// Resolve the connection this dialog dispatches to. "Local" is just the
    /// implicit loopback local-daemon connection; every project (local or
    /// remote) is created by dispatching to a daemon over the same mechanism —
    /// the GUI never mutates its read-only mirror directly.
    fn selected_connection_id(&self) -> String {
        match self.targets.get(self.selected_target) {
            Some(AddProjectTarget::Local) | None => LOCAL_DAEMON_CONNECTION_ID.to_string(),
            Some(AddProjectTarget::Remote { connection_id, .. }) => connection_id.clone(),
        }
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        let action = match self.source {
            AddProjectSource::Folder => self.folder_action(cx),
            AddProjectSource::Git => self.clone_action(cx),
        };
        let Some((action, name, path)) = action else {
            return;
        };

        let connection_id = self.selected_connection_id();
        if let Some(ref rm) = self.remote_manager {
            let connection_available = rm
                .read(cx)
                .connections()
                .iter()
                .any(|(config, _, _)| config.id == connection_id);
            if connection_available {
                let window_id = self.window_id;
                self.workspace.update(cx, |ws, _cx| {
                    ws.queue_pending_remote_project_visibility(
                        window_id,
                        &connection_id,
                        &name,
                        path.as_deref(),
                    );
                });
                rm.update(cx, |rm, cx| {
                    rm.send_action(&connection_id, action, cx);
                });
            }
        }

        self.close(cx);
    }

    /// The action plus the (name, path) the multi-window visibility queue
    /// matches the materialized project on.
    fn folder_action(&self, cx: &App) -> Option<(ActionRequest, String, Option<String>)> {
        let name = self.name_input.read(cx).value().trim().to_string();
        let path = self.path_input.read(cx).value(cx).trim().to_string();
        if name.is_empty() || path.is_empty() {
            return None;
        }
        Some((
            ActionRequest::AddProject {
                name: name.clone(),
                path: path.clone(),
            },
            name,
            Some(path),
        ))
    }

    fn clone_action(&self, cx: &App) -> Option<(ActionRequest, String, Option<String>)> {
        let url = self.url_input.read(cx).value().trim().to_string();
        let parent_dir = self.path_input.read(cx).value(cx).trim().to_string();
        let directory = self.directory_input.read(cx).value().trim().to_string();
        let name = self.name_input.read(cx).value().trim().to_string();
        if url.is_empty() || parent_dir.is_empty() || directory.is_empty() || name.is_empty() {
            return None;
        }
        Some((
            ActionRequest::CloneProject {
                url,
                parent_dir,
                directory,
                name: name.clone(),
            },
            name,
            // The target host joins the parent and the directory with ITS own
            // separator, so the client cannot predict the final path. The
            // visibility queue falls back to matching on the name alone.
            None,
        ))
    }

    fn open_folder_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let is_git = self.is_git_source();
        let paths = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(if is_git {
                "Select the folder to clone into".into()
            } else {
                "Select project folder".into()
            }),
        });

        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(selected_paths))) = paths.await
                && let Some(path) = selected_paths.first()
            {
                let path_str = path.to_string_lossy().to_string();
                // Git mode picks the PARENT directory — the project's own name
                // comes from the repository, not from the folder chosen here.
                let name_str = if is_git {
                    None
                } else {
                    Some(
                        path.file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_else(|| "Project".to_string()),
                    )
                };

                this.update(cx, |this, cx| {
                    this.pending_path_value = Some(path_str);
                    this.pending_name_value = name_str;
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    fn render_source_selector(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        div().flex().gap(px(6.0)).children(
            [
                (AddProjectSource::Folder, "Folder"),
                (AddProjectSource::Git, "Git"),
            ]
            .into_iter()
            .map(|(source, label)| {
                let is_selected = self.source == source;
                div()
                    .id(ElementId::Name(format!("source-{label}").into()))
                    .px(px(10.0))
                    .py(px(4.0))
                    .text_size(ui_text_ms(cx))
                    .rounded(px(4.0))
                    .cursor_pointer()
                    .when(is_selected, |d| {
                        d.bg(rgb(t.border_active)).text_color(rgb(t.bg_primary))
                    })
                    .when(!is_selected, |d| {
                        d.bg(rgb(t.bg_secondary))
                            .text_color(rgb(t.text_muted))
                            .hover(|s| s.bg(rgb(t.bg_hover)))
                    })
                    .child(label)
                    .on_click(cx.listener(move |this, _, _window, cx| {
                        this.source = source;
                        cx.notify();
                    }))
            }),
        )
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
                    .text_size(ui_text_ms(cx))
                    .rounded(px(4.0))
                    .cursor_pointer()
                    .when(is_selected, |d| {
                        d.bg(rgb(t.border_active)).text_color(rgb(t.bg_primary))
                    })
                    .when(!is_selected, |d| {
                        d.bg(rgb(t.bg_secondary))
                            .text_color(rgb(t.text_muted))
                            .hover(|s| s.bg(rgb(t.bg_hover)))
                    })
                    .child(label)
                    .on_click(cx.listener(move |this, _, _window, cx| {
                        this.selected_target = i;
                        let local_completion_enabled =
                            matches!(this.targets.get(i), Some(AddProjectTarget::Local));
                        this.path_input.update(cx, |input, cx| {
                            input.set_local_completion_enabled(local_completion_enabled, cx);
                        });
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

        let Some(bounds) = self.path_input_bounds.filter(|_| !suggestions.is_empty()) else {
            return div().into_any_element();
        };

        dropdown_anchored_below(
            bounds,
            div()
                .id("path-suggestions-container")
                .occlude()
                .w(bounds.size.width)
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
                    v_flex().children(suggestions.iter().enumerate().map(|(i, suggestion)| {
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
                                    .path(if suggestion.is_select_current {
                                        "icons/check.svg"
                                    } else if suggestion.is_directory {
                                        "icons/folder.svg"
                                    } else {
                                        "icons/file.svg"
                                    })
                                    .size(px(14.0))
                                    .text_color(
                                        if suggestion.is_select_current || suggestion.is_directory {
                                            rgb(t.border_active)
                                        } else {
                                            rgb(t.text_muted)
                                        },
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(ui_text_md(cx))
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
                    })),
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
            // The picker-derived name is an auto-fill like the URL-derived one,
            // so record it as such — switching to Git afterwards then still lets
            // the repository name take over.
            self.derived_name = name_value.clone();
            self.name_input
                .update(cx, |i, cx| i.set_value(&name_value, cx));
        }
        if let Some(path_value) = self.pending_path_value.take() {
            self.path_input
                .update(cx, |i, cx| i.set_value_quiet(&path_value, cx));
        }

        // Records the path input's bounds during paint. Deliberately does NOT
        // notify — a notify from inside paint would re-render every frame; the
        // stored bounds are read by the next render, which the keystroke that
        // produced the suggestions triggers anyway.
        let path_bounds_setter = {
            let entity = cx.entity().downgrade();
            move |bounds, _: &mut Window, cx: &mut App| {
                if let Some(entity) = entity.upgrade() {
                    entity.update(cx, |this, _| this.path_input_bounds = Some(bounds));
                }
            }
        };

        let is_remote = self.is_remote_target();
        let is_git = self.is_git_source();
        let has_suggestions = !is_remote && self.path_input.read(cx).has_suggestions();
        let has_multiple_targets = self.targets.len() > 1;

        let path_label = match (is_git, is_remote) {
            (true, _) => "Clone into:",
            (false, true) => "Path:",
            (false, false) => "Path (Tab to complete):",
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
                        cx,
                        cx.listener(|this, _, _, cx| this.close(cx)),
                    ))
                    .child(
                        div()
                            .p(px(16.0))
                            .flex()
                            .flex_col()
                            .gap(px(12.0))
                            .child(
                                labeled_input("Source:", &t).child(self.render_source_selector(cx)),
                            )
                            // Target selector (only when multiple targets available)
                            .when(has_multiple_targets, |d| {
                                d.child(
                                    labeled_input("Target:", &t)
                                        .child(self.render_target_selector(cx)),
                                )
                            })
                            // Repository URL (git source only)
                            .when(is_git, |d| {
                                d.child(labeled_input("Repository URL:", &t).child(
                                    input_container(&t, None).child(
                                        SimpleInput::new(&self.url_input)
                                            .text_size(ui_text_md(cx)),
                                    ),
                                ))
                            })
                            // Path input with auto-complete (or plain input for remote).
                            // Folder source: the project directory. Git source: the
                            // parent the clone lands in.
                            .child(
                                labeled_input(path_label, &t)
                                    .when(!is_remote, |d| d.child(self.path_input.clone()))
                                    .when(is_remote, |d| {
                                        d.child(
                                            input_container(&t, None).child(
                                                SimpleInput::new(self.path_input.read(cx).input())
                                                    .text_size(ui_text_md(cx)),
                                            ),
                                        )
                                    })
                                    // Track the input's painted bounds so the
                                    // completion list anchors to it in every layout.
                                    .child(
                                        canvas(path_bounds_setter, |_, _, _, _| {})
                                            .absolute()
                                            .size_full(),
                                    ),
                            )
                            // Target directory name (git source only)
                            .when(is_git, |d| {
                                d.child(labeled_input("Folder name:", &t).child(
                                    input_container(&t, None).child(
                                        SimpleInput::new(&self.directory_input)
                                            .text_size(ui_text_md(cx)),
                                    ),
                                ))
                            })
                            // Name input
                            .child(labeled_input("Name:", &t).child(
                                input_container(&t, None).child(
                                    SimpleInput::new(&self.name_input).text_size(ui_text_md(cx)),
                                ),
                            ))
                            // Browse button (only for local target)
                            .when(!is_remote, |d| {
                                d.child(
                                    button("browse-folder-btn", "Browse...", &t)
                                        .px(px(8.0))
                                        .py(px(4.0))
                                        .text_size(ui_text_ms(cx))
                                        .text_color(rgb(t.text_primary))
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.open_folder_picker(window, cx);
                                        })),
                                )
                            })
                            // Action buttons
                            .child(dialog_actions(
                                "Cancel",
                                cx.listener(|this, _, _window, cx| {
                                    this.close(cx);
                                }),
                                if is_git { "Clone" } else { "Add" },
                                cx.listener(|this, _, _window, cx| {
                                    this.submit(cx);
                                }),
                                &t,
                            )),
                    )
                    // Path suggestions overlay (only for local target)
                    .when(has_suggestions, |d| {
                        d.child(self.render_path_suggestions(cx))
                    }),
            )
    }
}

impl_focusable!(AddProjectDialog);
