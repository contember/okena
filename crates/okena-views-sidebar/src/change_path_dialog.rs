//! Dialog for repointing a project at a directory that already exists on disk.
//!
//! The sibling `rename_directory_dialog` renames a folder *in place* and so
//! takes a bare name. This one takes a whole path, because the case it serves
//! is the folder having moved somewhere else entirely — usually leaving the
//! project's recorded path dangling.

use crate::Cancel;
use gpui::prelude::*;
use gpui::*;
use gpui_component::h_flex;
use okena_ui::button::{button, button_primary};
use okena_ui::input::input_container;
use okena_ui::modal::{modal_backdrop, modal_content};
use okena_ui::simple_input::{SimpleInput, SimpleInputState};
use okena_ui::theme::theme;
use okena_ui::tokens::{ui_text, ui_text_md, ui_text_ms, ui_text_xl};
use std::path::{Path, PathBuf};

/// Events emitted by the change path dialog
#[derive(Clone)]
pub enum ChangePathDialogEvent {
    /// Dialog closed (cancelled or confirmed)
    Close,
    /// A new path was confirmed: the host dispatches
    /// `ActionRequest::ChangeProjectPath`; the daemon rewrites the record and
    /// mirrors the new path back.
    Confirmed {
        project_id: String,
        new_path: String,
    },
}

impl okena_ui::overlay::CloseEvent for ChangePathDialogEvent {
    fn is_close(&self) -> bool {
        matches!(self, Self::Close | Self::Confirmed { .. })
    }
}

impl EventEmitter<ChangePathDialogEvent> for ChangePathDialog {}

/// Dialog for pointing a project at a different existing directory.
pub struct ChangePathDialog {
    project_id: String,
    project_path: String,
    /// Whether the project's directory is on this machine. When it isn't, the
    /// owning daemon is the only thing that can judge the typed path, so the
    /// local pre-checks below stand down and it validates instead.
    shares_local_filesystem: bool,
    path_input: Entity<SimpleInputState>,
    error_message: Option<String>,
    focus_handle: FocusHandle,
    initialized: bool,
}

impl ChangePathDialog {
    pub fn new(
        project_id: String,
        project_path: String,
        shares_local_filesystem: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let path_input = cx.new(|cx| {
            let mut input = SimpleInputState::new(cx).placeholder("/path/to/project...");
            input.set_value(&project_path, cx);
            input
        });

        Self {
            project_id,
            project_path,
            shares_local_filesystem,
            path_input,
            error_message: None,
            focus_handle: cx.focus_handle(),
            initialized: false,
        }
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        cx.emit(ChangePathDialogEvent::Close);
    }

    /// Expand a leading `~` so typed and pasted paths both work.
    fn expand_home(raw: &str) -> String {
        let Some(rest) = raw.strip_prefix('~') else {
            return raw.to_string();
        };
        if !(rest.is_empty() || rest.starts_with('/') || rest.starts_with('\\')) {
            // `~other-user/...` is not ours to resolve.
            return raw.to_string();
        }
        let Some(home) = dirs::home_dir() else {
            return raw.to_string();
        };
        let trimmed = rest.trim_start_matches(['/', '\\']);
        if trimmed.is_empty() {
            home.to_string_lossy().into_owned()
        } else {
            home.join(trimmed).to_string_lossy().into_owned()
        }
    }

    fn confirm(&mut self, cx: &mut Context<Self>) {
        let raw = self.path_input.read(cx).value().trim().to_string();

        if raw.is_empty() {
            self.error_message = Some("Path cannot be empty".to_string());
            cx.notify();
            return;
        }

        // `~` and every filesystem check below describe *this* machine. For a
        // project served by a remote daemon none of them apply — its home
        // directory is not ours, and its paths need not even use this
        // platform's shape (a Windows `C:\...` reads as relative here). So the
        // owning daemon validates those, and its error arrives as a toast.
        let new_path = if self.shares_local_filesystem {
            Self::expand_home(&raw)
        } else {
            raw
        };

        if new_path == self.project_path {
            self.error_message = Some("Path is the same as the current one".to_string());
            cx.notify();
            return;
        }

        if self.shares_local_filesystem {
            let candidate = PathBuf::from(&new_path);
            if !candidate.is_absolute() {
                self.error_message = Some("Path must be absolute".to_string());
                cx.notify();
                return;
            }
            // The daemon checks these too and is the authority — repeating them
            // here only turns a toast fired after the dialog closes into an
            // inline message the user can act on without reopening anything.
            if !candidate.exists() {
                self.error_message = Some(format!("'{new_path}' does not exist"));
                cx.notify();
                return;
            }
            if !candidate.is_dir() {
                self.error_message = Some(format!("'{new_path}' is not a directory"));
                cx.notify();
                return;
            }
            if candidate.as_path() == Path::new(&self.project_path) {
                self.error_message = Some("Path is the same as the current one".to_string());
                cx.notify();
                return;
            }
        }

        cx.emit(ChangePathDialogEvent::Confirmed {
            project_id: self.project_id.clone(),
            new_path,
        });
    }
}

okena_ui::impl_focusable!(ChangePathDialog);

impl Render for ChangePathDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let focus_handle = self.focus_handle.clone();

        if !self.initialized {
            self.initialized = true;
            self.path_input.update(cx, |input, cx| {
                input.focus(window, cx);
            });
        }

        let path_input = self.path_input.clone();
        let input_focused = self.path_input.read(cx).focus_handle(cx).is_focused(window);
        let error_msg = self.error_message.clone();
        // Only meaningful for a path on this disk; a remote project's directory
        // is not ours to stat, and calling it missing would be a lie.
        let current_missing =
            self.shares_local_filesystem && !Path::new(&self.project_path).exists();
        let current_label = if current_missing {
            format!("{} (missing)", self.project_path)
        } else {
            self.project_path.clone()
        };
        let caption = if self.shares_local_filesystem {
            "Nothing is moved on disk — only the folder this project points at."
        } else {
            "Nothing is moved on disk. The path is on the remote host and is checked there."
        };

        modal_backdrop("change-path-dialog-backdrop", &t)
            .track_focus(&focus_handle)
            .key_context("ChangePathDialog")
            .items_center()
            .on_action(cx.listener(|this, _: &Cancel, _, cx| {
                this.close(cx);
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key.as_str() == "enter" {
                    this.confirm(cx);
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.close(cx);
                }),
            )
            .child(
                modal_content("change-path-dialog", &t)
                    .w(px(520.0))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
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
                                            .text_size(ui_text_xl(cx))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(t.text_primary))
                                            .child("Change Folder Path"),
                                    ),
                            )
                            .child(
                                div()
                                    .id("close-change-path-btn")
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
                    .child(
                        div()
                            .px(px(16.0))
                            .py(px(12.0))
                            .flex()
                            .flex_col()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .text_size(ui_text_ms(cx))
                                    .text_color(rgb(if current_missing {
                                        t.error
                                    } else {
                                        t.text_muted
                                    }))
                                    .overflow_x_hidden()
                                    .whitespace_nowrap()
                                    .child(current_label),
                            )
                            .child(
                                input_container(&t, Some(input_focused)).child(
                                    SimpleInput::new(&path_input).text_size(ui_text(13.0, cx)),
                                ),
                            )
                            .child(
                                div()
                                    .text_size(ui_text_ms(cx))
                                    .text_color(rgb(t.text_muted))
                                    .child(caption),
                            ),
                    )
                    .when_some(error_msg, |d, msg| {
                        d.child(
                            div()
                                .px(px(16.0))
                                .py(px(8.0))
                                .bg(rgba(0xff00001a))
                                .text_size(ui_text_md(cx))
                                .text_color(rgb(t.error))
                                .child(msg),
                        )
                    })
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
                                button("cancel-change-path-btn", "Cancel", &t)
                                    .px(px(16.0))
                                    .py(px(8.0))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.close(cx);
                                    })),
                            )
                            .child(
                                button_primary("confirm-change-path-btn", "Change Path", &t)
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

#[cfg(test)]
mod tests {
    use super::ChangePathDialog;

    #[test]
    fn expand_home_rewrites_only_our_own_tilde() {
        let home = dirs::home_dir().expect("home dir");
        assert_eq!(
            ChangePathDialog::expand_home("~/Developer/x"),
            home.join("Developer/x").to_string_lossy()
        );
        assert_eq!(
            ChangePathDialog::expand_home("~"),
            home.to_string_lossy().into_owned()
        );
        // Another user's home is not ours to resolve, and a bare path is
        // already what it claims to be.
        assert_eq!(ChangePathDialog::expand_home("~someone/x"), "~someone/x");
        assert_eq!(ChangePathDialog::expand_home("/abs/x"), "/abs/x");
    }
}
