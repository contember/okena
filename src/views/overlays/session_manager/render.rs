use crate::keybindings::Cancel;
use crate::theme::{theme, with_alpha};
use crate::views::components::{modal_backdrop, modal_content, modal_header, SimpleInput};
use crate::workspace::persistence::{config_dir, SessionInfo};
use gpui::*;
use gpui_component::{h_flex, v_flex};
use gpui::prelude::*;

use super::{SessionManager, SessionManagerTab};

impl SessionManager {
    pub(super) fn render_session_row(
        &self,
        session: &SessionInfo,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let t = theme(cx);
        let name = session.name.clone();
        let is_renaming = self.renaming_session.as_ref() == Some(&name);
        let is_deleting = self.show_delete_confirmation.as_ref() == Some(&name);

        let name_for_load = name.clone();
        let name_for_rename = name.clone();
        let name_for_delete = name.clone();
        let name_for_delete_confirm = name.clone();

        h_flex()
            .justify_between()
            .px(px(12.0))
            .py(px(10.0))
            .border_b_1()
            .border_color(rgb(t.border))
            .when(is_deleting, |d| {
                // Delete confirmation row
                d.bg(with_alpha(t.error, 0.1)).child(
                    h_flex()
                        .justify_between()
                        .w_full()
                        .child(
                            div()
                                .text_size(px(13.0))
                                .text_color(rgb(t.error))
                                .child(format!("Delete '{}'?", name)),
                        )
                        .child(
                            h_flex()
                                .gap(px(8.0))
                                .child(
                                    div()
                                        .id(SharedString::from(format!("delete-confirm-{}", name)))
                                        .cursor_pointer()
                                        .px(px(10.0))
                                        .py(px(4.0))
                                        .rounded(px(4.0))
                                        .bg(rgb(t.error))
                                        .text_size(px(12.0))
                                        .text_color(rgb(0xFFFFFF))
                                        .child("Delete")
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _window, cx| {
                                                this.delete_session(&name_for_delete_confirm, cx);
                                            }),
                                        ),
                                )
                                .child(
                                    div()
                                        .id(SharedString::from(format!("delete-cancel-{}", name)))
                                        .cursor_pointer()
                                        .px(px(10.0))
                                        .py(px(4.0))
                                        .rounded(px(4.0))
                                        .bg(rgb(t.bg_secondary))
                                        .hover(|s| s.bg(rgb(t.bg_hover)))
                                        .text_size(px(12.0))
                                        .text_color(rgb(t.text_primary))
                                        .child("Cancel")
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|this, _, _window, cx| {
                                                this.cancel_delete(cx);
                                            }),
                                        ),
                                ),
                        ),
                )
            })
            .when(!is_deleting && !is_renaming, |d| {
                // Normal row
                d.child(
                    v_flex()
                        .gap(px(2.0))
                        .child(
                            div()
                                .text_size(px(14.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(rgb(t.text_primary))
                                .child(name.clone()),
                        )
                        .child(
                            h_flex()
                                .gap(px(12.0))
                                .child(
                                    div()
                                        .text_size(px(11.0))
                                        .text_color(rgb(t.text_muted))
                                        .child(format!(
                                            "{} project{}",
                                            session.project_count,
                                            if session.project_count == 1 { "" } else { "s" }
                                        )),
                                )
                                .child(
                                    div()
                                        .text_size(px(11.0))
                                        .text_color(rgb(t.text_muted))
                                        .child(format!("Modified: {}", &session.modified_at[..10])),
                                ),
                        ),
                )
                .child(
                    h_flex()
                        .gap(px(6.0))
                        .child(
                            div()
                                .id(SharedString::from(format!("load-{}", name)))
                                .cursor_pointer()
                                .px(px(10.0))
                                .py(px(4.0))
                                .rounded(px(4.0))
                                .bg(rgb(t.button_primary_bg))
                                .hover(|s| s.bg(rgb(t.button_primary_hover)))
                                .text_size(px(12.0))
                                .text_color(rgb(t.button_primary_fg))
                                .child("Load")
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _window, cx| {
                                        this.load_session(&name_for_load, cx);
                                    }),
                                ),
                        )
                        .child(
                            div()
                                .id(SharedString::from(format!("rename-{}", name)))
                                .cursor_pointer()
                                .px(px(8.0))
                                .py(px(4.0))
                                .rounded(px(4.0))
                                .bg(rgb(t.bg_secondary))
                                .hover(|s| s.bg(rgb(t.bg_hover)))
                                .text_size(px(12.0))
                                .text_color(rgb(t.text_secondary))
                                .child("Rename")
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, window, cx| {
                                        this.start_rename(&name_for_rename, window, cx);
                                    }),
                                ),
                        )
                        .child(
                            div()
                                .id(SharedString::from(format!("delete-{}", name)))
                                .cursor_pointer()
                                .px(px(8.0))
                                .py(px(4.0))
                                .rounded(px(4.0))
                                .bg(rgb(t.bg_secondary))
                                .hover(|s| s.bg(with_alpha(t.error, 0.2)))
                                .text_size(px(12.0))
                                .text_color(rgb(t.error))
                                .child("Delete")
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _window, cx| {
                                        this.confirm_delete(&name_for_delete, cx);
                                    }),
                                ),
                        ),
                )
            })
            .when(is_renaming, |d| {
                // Rename mode with SimpleInput
                if let Some(ref rename_input) = self.rename_input {
                    d.child(
                        h_flex()
                            .gap(px(8.0))
                            .w_full()
                            .child(
                                div()
                                    .id("rename-input-wrapper")
                                    .flex_1()
                                    .bg(rgb(t.bg_secondary))
                                    .rounded(px(4.0))
                                    .border_1()
                                    .border_color(rgb(t.border_active))
                                    .child(SimpleInput::new(rename_input).text_size(px(13.0)))
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                        cx.stop_propagation();
                                    })
                                    .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                                        cx.stop_propagation();
                                        match event.keystroke.key.as_str() {
                                            "enter" => this.confirm_rename(cx),
                                            "escape" => this.cancel_rename(cx),
                                            _ => {}
                                        }
                                    })),
                            )
                            .child(
                                h_flex()
                                    .gap(px(6.0))
                                    .child(
                                        div()
                                            .id("rename-confirm")
                                            .cursor_pointer()
                                            .px(px(10.0))
                                            .py(px(4.0))
                                            .rounded(px(4.0))
                                            .bg(rgb(t.button_primary_bg))
                                            .hover(|s| s.bg(rgb(t.button_primary_hover)))
                                            .text_size(px(12.0))
                                            .text_color(rgb(t.button_primary_fg))
                                            .child("Save")
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(|this, _, _window, cx| {
                                                    this.confirm_rename(cx);
                                                }),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .id("rename-cancel")
                                            .cursor_pointer()
                                            .px(px(10.0))
                                            .py(px(4.0))
                                            .rounded(px(4.0))
                                            .bg(rgb(t.bg_secondary))
                                            .hover(|s| s.bg(rgb(t.bg_hover)))
                                            .text_size(px(12.0))
                                            .text_color(rgb(t.text_primary))
                                            .child("Cancel")
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(|this, _, _window, cx| {
                                                    this.cancel_rename(cx);
                                                }),
                                            ),
                                    ),
                            ),
                    )
                } else {
                    d
                }
            })
    }

    pub(super) fn render_sessions_tab(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let sessions = self.sessions.clone();
        let new_session_input = self.new_session_input.clone();

        v_flex()
            .flex_1()
            .min_h_0()
            .child(
                // Save current as new session
                div()
                    .px(px(16.0))
                    .py(px(12.0))
                    .border_b_1()
                    .border_color(rgb(t.border))
                    .child(
                        h_flex()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .id("new-session-input-wrapper")
                                    .flex_1()
                                    .bg(rgb(t.bg_secondary))
                                    .rounded(px(4.0))
                                    .border_1()
                                    .border_color(rgb(t.border))
                                    .child(SimpleInput::new(&new_session_input).text_size(px(13.0)))
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                        cx.stop_propagation();
                                    })
                                    .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                                        cx.stop_propagation();
                                        if event.keystroke.key.as_str() == "enter" {
                                            this.save_new_session(cx);
                                        }
                                    })),
                            )
                            .child(
                                div()
                                    .id("save-session-btn")
                                    .cursor_pointer()
                                    .px(px(12.0))
                                    .py(px(8.0))
                                    .rounded(px(4.0))
                                    .bg(rgb(t.button_primary_bg))
                                    .hover(|s| s.bg(rgb(t.button_primary_hover)))
                                    .text_size(px(13.0))
                                    .text_color(rgb(t.button_primary_fg))
                                    .child("Save Current")
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _window, cx| {
                                            this.save_new_session(cx);
                                        }),
                                    ),
                            ),
                    ),
            )
            .child(
                // Sessions list
                div()
                    .id("sessions-list")
                    .flex_1()
                    .overflow_y_scroll()
                    .when(sessions.is_empty(), |d| {
                        d.flex()
                            .items_center()
                            .justify_center()
                            .child(
                                div()
                                    .text_size(px(14.0))
                                    .text_color(rgb(t.text_muted))
                                    .child("No saved sessions"),
                            )
                    })
                    .when(!sessions.is_empty(), |d| {
                        d.children(
                            sessions
                                .iter()
                                .map(|session| self.render_session_row(session, cx)),
                        )
                    }),
            )
    }

    pub(super) fn render_export_import_tab(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let export_path_input = self.export_path_input.clone();
        let import_path_input = self.import_path_input.clone();

        v_flex()
            .flex_1()
            .min_h_0()
            .px(px(16.0))
            .py(px(16.0))
            .gap(px(24.0))
            .child(
                // Export section
                v_flex()
                    .gap(px(8.0))
                    .child(
                        div()
                            .text_size(px(14.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(t.text_primary))
                            .child("Export Current Workspace"),
                    )
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(rgb(t.text_muted))
                            .child("Save your current workspace configuration to a file that can be shared or backed up."),
                    )
                    .child(
                        h_flex()
                            .gap(px(8.0))
                            .mt(px(4.0))
                            .child(
                                div()
                                    .id("export-path-input-wrapper")
                                    .flex_1()
                                    .bg(rgb(t.bg_secondary))
                                    .rounded(px(4.0))
                                    .border_1()
                                    .border_color(rgb(t.border))
                                    .font_family("monospace")
                                    .child(SimpleInput::new(&export_path_input).text_size(px(12.0)))
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                        cx.stop_propagation();
                                    })
                                    .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                                        cx.stop_propagation();
                                        if event.keystroke.key.as_str() == "enter" {
                                            this.export_current(cx);
                                        }
                                    })),
                            )
                            .child(
                                div()
                                    .id("export-btn")
                                    .cursor_pointer()
                                    .px(px(12.0))
                                    .py(px(8.0))
                                    .rounded(px(4.0))
                                    .bg(rgb(t.button_primary_bg))
                                    .hover(|s| s.bg(rgb(t.button_primary_hover)))
                                    .text_size(px(13.0))
                                    .text_color(rgb(t.button_primary_fg))
                                    .child("Export")
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _window, cx| {
                                            this.export_current(cx);
                                        }),
                                    ),
                            ),
                    ),
            )
            .child(
                // Import section
                v_flex()
                    .gap(px(8.0))
                    .child(
                        div()
                            .text_size(px(14.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(t.text_primary))
                            .child("Import Workspace"),
                    )
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(rgb(t.text_muted))
                            .child("Load a workspace configuration from an exported file. This will replace your current workspace."),
                    )
                    .child(
                        h_flex()
                            .gap(px(8.0))
                            .mt(px(4.0))
                            .child(
                                div()
                                    .id("import-path-input-wrapper")
                                    .flex_1()
                                    .bg(rgb(t.bg_secondary))
                                    .rounded(px(4.0))
                                    .border_1()
                                    .border_color(rgb(t.border))
                                    .font_family("monospace")
                                    .child(SimpleInput::new(&import_path_input).text_size(px(12.0)))
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                        cx.stop_propagation();
                                    })
                                    .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                                        cx.stop_propagation();
                                        if event.keystroke.key.as_str() == "enter" {
                                            this.import_from_file(cx);
                                        }
                                    })),
                            )
                            .child(
                                div()
                                    .id("import-btn")
                                    .cursor_pointer()
                                    .px(px(12.0))
                                    .py(px(8.0))
                                    .rounded(px(4.0))
                                    .bg(rgb(t.bg_secondary))
                                    .hover(|s| s.bg(rgb(t.bg_hover)))
                                    .text_size(px(13.0))
                                    .text_color(rgb(t.text_primary))
                                    .child("Import")
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _window, cx| {
                                            this.import_from_file(cx);
                                        }),
                                    ),
                            ),
                    ),
            )
            .child(
                // Config directory info
                v_flex()
                    .gap(px(4.0))
                    .pt(px(16.0))
                    .border_t_1()
                    .border_color(rgb(t.border))
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(rgb(t.text_muted))
                            .child("Sessions are stored in:"),
                    )
                    .child(
                        div()
                            .text_size(px(10.0))
                            .font_family("monospace")
                            .text_color(rgb(t.text_secondary))
                            .child(config_dir().join("sessions").display().to_string()),
                    ),
            )
    }
}

impl Render for SessionManager {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let focus_handle = self.focus_handle.clone();
        let error_message = self.error_message.clone();
        let active_tab = self.active_tab;

        // Focus on first render
        if !focus_handle.is_focused(window) {
            window.focus(&focus_handle, cx);
        }

        modal_backdrop("session-manager-backdrop", &t)
            .track_focus(&focus_handle)
            .key_context("SessionManager")
            .items_center()
            .on_action(cx.listener(|this, _: &Cancel, _window, cx| {
                if this.renaming_session.is_some() {
                    this.cancel_rename(cx);
                } else {
                    this.close(cx);
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _window, cx| {
                    this.close(cx);
                }),
            )
            .child(
                modal_content("session-manager-modal", &t)
                    .w(px(550.0))
                    .max_h(px(600.0))
                    .child(modal_header(
                        "Session Manager",
                        Some("Save and restore workspace sessions"),
                        &t,
                        cx.listener(|this, _, _window, cx| this.close(cx)),
                    ))
                    .child(
                        // Tabs
                        div()
                            .flex()
                            .border_b_1()
                            .border_color(rgb(t.border))
                            .child(
                                div()
                                    .id("tab-sessions")
                                    .cursor_pointer()
                                    .px(px(16.0))
                                    .py(px(10.0))
                                    .text_size(px(13.0))
                                    .text_color(if active_tab == SessionManagerTab::Sessions {
                                        rgb(t.text_primary)
                                    } else {
                                        rgb(t.text_muted)
                                    })
                                    .when(active_tab == SessionManagerTab::Sessions, |d| {
                                        d.border_b_2().border_color(rgb(t.border_active))
                                    })
                                    .hover(|s| s.bg(rgb(t.bg_hover)))
                                    .child("Sessions")
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _window, cx| {
                                            this.active_tab = SessionManagerTab::Sessions;
                                            cx.notify();
                                        }),
                                    ),
                            )
                            .child(
                                div()
                                    .id("tab-export-import")
                                    .cursor_pointer()
                                    .px(px(16.0))
                                    .py(px(10.0))
                                    .text_size(px(13.0))
                                    .text_color(if active_tab == SessionManagerTab::ExportImport {
                                        rgb(t.text_primary)
                                    } else {
                                        rgb(t.text_muted)
                                    })
                                    .when(active_tab == SessionManagerTab::ExportImport, |d| {
                                        d.border_b_2().border_color(rgb(t.border_active))
                                    })
                                    .hover(|s| s.bg(rgb(t.bg_hover)))
                                    .child("Export/Import")
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _window, cx| {
                                            this.active_tab = SessionManagerTab::ExportImport;
                                            cx.notify();
                                        }),
                                    ),
                            ),
                    )
                    // Error message
                    .when(error_message.is_some(), |d| {
                        if let Some(msg) = error_message {
                            d.child(
                                div()
                                    .px(px(16.0))
                                    .py(px(8.0))
                                    .bg(with_alpha(t.error, 0.1))
                                    .border_b_1()
                                    .border_color(rgb(t.border))
                                    .child(
                                        div()
                                            .text_size(px(12.0))
                                            .text_color(rgb(t.error))
                                            .child(msg),
                                    ),
                            )
                        } else {
                            d
                        }
                    })
                    // Tab content
                    .child(match active_tab {
                        SessionManagerTab::Sessions => self.render_sessions_tab(cx).into_any_element(),
                        SessionManagerTab::ExportImport => {
                            self.render_export_import_tab(cx).into_any_element()
                        }
                    }),
            )
    }
}
