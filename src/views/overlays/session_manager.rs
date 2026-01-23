use crate::settings::settings_entity;
use crate::theme::{theme, with_alpha};
use crate::workspace::persistence::{
    config_dir, delete_session, export_workspace, import_workspace, list_sessions,
    load_session, rename_session, save_session, session_exists, SessionInfo,
};
use crate::workspace::state::{Workspace, WorkspaceData};
use gpui::*;
use gpui::prelude::*;

/// Session Manager overlay for managing multiple workspaces
pub struct SessionManager {
    workspace: Entity<Workspace>,
    focus_handle: FocusHandle,
    sessions: Vec<SessionInfo>,
    new_session_name: String,
    rename_session_name: String,
    renaming_session: Option<String>,
    error_message: Option<String>,
    show_delete_confirmation: Option<String>,
    export_path: String,
    import_path: String,
    active_tab: SessionManagerTab,
}

#[derive(Clone, Copy, PartialEq)]
enum SessionManagerTab {
    Sessions,
    ExportImport,
}

impl SessionManager {
    pub fn new(workspace: Entity<Workspace>, cx: &mut Context<Self>) -> Self {
        let sessions = list_sessions().unwrap_or_default();
        let focus_handle = cx.focus_handle();

        // Default export path
        let export_path = dirs::home_dir()
            .map(|p| p.join("workspace-export.json").to_string_lossy().to_string())
            .unwrap_or_else(|| "workspace-export.json".to_string());

        Self {
            workspace,
            focus_handle,
            sessions,
            new_session_name: String::new(),
            rename_session_name: String::new(),
            renaming_session: None,
            error_message: None,
            show_delete_confirmation: None,
            export_path,
            import_path: String::new(),
            active_tab: SessionManagerTab::Sessions,
        }
    }

    fn close(&self, cx: &mut Context<Self>) {
        cx.emit(SessionManagerEvent::Close);
    }

    fn refresh_sessions(&mut self) {
        self.sessions = list_sessions().unwrap_or_default();
        self.error_message = None;
    }

    fn save_new_session(&mut self, cx: &mut Context<Self>) {
        let name = self.new_session_name.trim().to_string();
        if name.is_empty() {
            self.error_message = Some("Session name cannot be empty".to_string());
            cx.notify();
            return;
        }

        if session_exists(&name) {
            self.error_message = Some(format!("Session '{}' already exists", name));
            cx.notify();
            return;
        }

        let data = self.workspace.read(cx).data.clone();
        match save_session(&name, &data) {
            Ok(()) => {
                self.new_session_name.clear();
                self.refresh_sessions();
                self.error_message = None;
            }
            Err(e) => {
                self.error_message = Some(format!("Failed to save session: {}", e));
            }
        }
        cx.notify();
    }

    fn load_session(&mut self, name: &str, cx: &mut Context<Self>) {
        let backend = settings_entity(cx).read(cx).settings.session_backend;
        match load_session(name, backend) {
            Ok(data) => {
                // Emit event to notify parent to switch workspace
                cx.emit(SessionManagerEvent::SwitchWorkspace(data));
                self.error_message = None;
            }
            Err(e) => {
                self.error_message = Some(format!("Failed to load session: {}", e));
                cx.notify();
            }
        }
    }

    fn start_rename(&mut self, name: &str, cx: &mut Context<Self>) {
        self.renaming_session = Some(name.to_string());
        self.rename_session_name = name.to_string();
        cx.notify();
    }

    fn cancel_rename(&mut self, cx: &mut Context<Self>) {
        self.renaming_session = None;
        self.rename_session_name.clear();
        cx.notify();
    }

    fn confirm_rename(&mut self, cx: &mut Context<Self>) {
        if let Some(old_name) = self.renaming_session.take() {
            let new_name = self.rename_session_name.trim().to_string();
            if new_name.is_empty() {
                self.error_message = Some("Session name cannot be empty".to_string());
                cx.notify();
                return;
            }

            if new_name != old_name && session_exists(&new_name) {
                self.error_message = Some(format!("Session '{}' already exists", new_name));
                cx.notify();
                return;
            }

            if new_name != old_name {
                match rename_session(&old_name, &new_name) {
                    Ok(()) => {
                        self.refresh_sessions();
                        self.error_message = None;
                    }
                    Err(e) => {
                        self.error_message = Some(format!("Failed to rename session: {}", e));
                    }
                }
            }
        }
        self.rename_session_name.clear();
        cx.notify();
    }

    fn confirm_delete(&mut self, name: &str, cx: &mut Context<Self>) {
        self.show_delete_confirmation = Some(name.to_string());
        cx.notify();
    }

    fn cancel_delete(&mut self, cx: &mut Context<Self>) {
        self.show_delete_confirmation = None;
        cx.notify();
    }

    fn delete_session(&mut self, name: &str, cx: &mut Context<Self>) {
        match delete_session(name) {
            Ok(()) => {
                self.show_delete_confirmation = None;
                self.refresh_sessions();
                self.error_message = None;
            }
            Err(e) => {
                self.error_message = Some(format!("Failed to delete session: {}", e));
            }
        }
        cx.notify();
    }

    fn export_current(&mut self, cx: &mut Context<Self>) {
        let path = self.export_path.trim();
        if path.is_empty() {
            self.error_message = Some("Export path cannot be empty".to_string());
            cx.notify();
            return;
        }

        let data = self.workspace.read(cx).data.clone();
        match export_workspace(&data, std::path::Path::new(path)) {
            Ok(()) => {
                self.error_message = None;
                // Show success message briefly
                log::info!("Workspace exported to {}", path);
            }
            Err(e) => {
                self.error_message = Some(format!("Failed to export: {}", e));
            }
        }
        cx.notify();
    }

    fn import_from_file(&mut self, cx: &mut Context<Self>) {
        let path = self.import_path.trim();
        if path.is_empty() {
            self.error_message = Some("Import path cannot be empty".to_string());
            cx.notify();
            return;
        }

        match import_workspace(std::path::Path::new(path)) {
            Ok(data) => {
                cx.emit(SessionManagerEvent::SwitchWorkspace(data));
                self.error_message = None;
            }
            Err(e) => {
                self.error_message = Some(format!("Failed to import: {}", e));
                cx.notify();
            }
        }
    }

    fn render_session_row(
        &self,
        session: &SessionInfo,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = theme(cx);
        let name = session.name.clone();
        let is_renaming = self.renaming_session.as_ref() == Some(&name);
        let is_deleting = self.show_delete_confirmation.as_ref() == Some(&name);

        let name_for_load = name.clone();
        let name_for_rename = name.clone();
        let name_for_delete = name.clone();
        let name_for_delete_confirm = name.clone();

        div()
            .flex()
            .items_center()
            .justify_between()
            .px(px(12.0))
            .py(px(10.0))
            .border_b_1()
            .border_color(rgb(t.border))
            .when(is_deleting, |d| {
                // Delete confirmation row
                d.bg(with_alpha(t.error, 0.1)).child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .w_full()
                        .child(
                            div()
                                .text_size(px(13.0))
                                .text_color(rgb(t.error))
                                .child(format!("Delete '{}'?", name)),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
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
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .child(
                            div()
                                .text_size(px(14.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(rgb(t.text_primary))
                                .child(name.clone()),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
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
                    div()
                        .flex()
                        .items_center()
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
                                    cx.listener(move |this, _, _window, cx| {
                                        this.start_rename(&name_for_rename, cx);
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
                // Rename mode
                let rename_name = self.rename_session_name.clone();
                d.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .w_full()
                        .child(
                            div()
                                .flex_1()
                                .px(px(8.0))
                                .py(px(4.0))
                                .bg(rgb(t.bg_secondary))
                                .rounded(px(4.0))
                                .border_1()
                                .border_color(rgb(t.border_active))
                                .text_size(px(13.0))
                                .text_color(rgb(t.text_primary))
                                .child(rename_name),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
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
            })
    }

    fn render_sessions_tab(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let sessions = self.sessions.clone();
        let new_name = self.new_session_name.clone();

        div()
            .flex()
            .flex_col()
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
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .flex_1()
                                    .px(px(10.0))
                                    .py(px(8.0))
                                    .bg(rgb(t.bg_secondary))
                                    .rounded(px(4.0))
                                    .border_1()
                                    .border_color(rgb(t.border))
                                    .text_size(px(13.0))
                                    .text_color(if new_name.is_empty() {
                                        rgb(t.text_muted)
                                    } else {
                                        rgb(t.text_primary)
                                    })
                                    .child(if new_name.is_empty() {
                                        "Enter session name...".to_string()
                                    } else {
                                        new_name
                                    }),
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

    fn render_export_import_tab(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let export_path = self.export_path.clone();
        let import_path = self.import_path.clone();

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .px(px(16.0))
            .py(px(16.0))
            .gap(px(24.0))
            .child(
                // Export section
                div()
                    .flex()
                    .flex_col()
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
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .mt(px(4.0))
                            .child(
                                div()
                                    .flex_1()
                                    .px(px(10.0))
                                    .py(px(8.0))
                                    .bg(rgb(t.bg_secondary))
                                    .rounded(px(4.0))
                                    .border_1()
                                    .border_color(rgb(t.border))
                                    .text_size(px(12.0))
                                    .font_family("monospace")
                                    .text_color(rgb(t.text_primary))
                                    .child(export_path),
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
                div()
                    .flex()
                    .flex_col()
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
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .mt(px(4.0))
                            .child(
                                div()
                                    .flex_1()
                                    .px(px(10.0))
                                    .py(px(8.0))
                                    .bg(rgb(t.bg_secondary))
                                    .rounded(px(4.0))
                                    .border_1()
                                    .border_color(rgb(t.border))
                                    .text_size(px(12.0))
                                    .font_family("monospace")
                                    .text_color(if import_path.is_empty() {
                                        rgb(t.text_muted)
                                    } else {
                                        rgb(t.text_primary)
                                    })
                                    .child(if import_path.is_empty() {
                                        "Enter path to import...".to_string()
                                    } else {
                                        import_path
                                    }),
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
                div()
                    .flex()
                    .flex_col()
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

pub enum SessionManagerEvent {
    Close,
    SwitchWorkspace(WorkspaceData),
}

impl EventEmitter<SessionManagerEvent> for SessionManager {}

impl Render for SessionManager {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let focus_handle = self.focus_handle.clone();
        let error_message = self.error_message.clone();
        let active_tab = self.active_tab;

        // Focus on first render
        window.focus(&focus_handle, cx);

        div()
            .track_focus(&focus_handle)
            .key_context("SessionManager")
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                if event.keystroke.key.as_str() == "escape" {
                    this.close(cx);
                }
            }))
            .absolute()
            .inset_0()
            .bg(hsla(0.0, 0.0, 0.0, 0.5))
            .flex()
            .items_center()
            .justify_center()
            .id("session-manager-backdrop")
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _window, cx| {
                    this.close(cx);
                }),
            )
            .child(
                // Modal content
                div()
                    .id("session-manager-modal")
                    .w(px(550.0))
                    .max_h(px(600.0))
                    .bg(rgb(t.bg_primary))
                    .rounded(px(8.0))
                    .border_1()
                    .border_color(rgb(t.border))
                    .shadow_xl()
                    .flex()
                    .flex_col()
                    .on_mouse_down(MouseButton::Left, |_, _window, _cx| {})
                    .child(
                        // Header
                        div()
                            .px(px(16.0))
                            .py(px(12.0))
                            .flex()
                            .items_center()
                            .justify_between()
                            .border_b_1()
                            .border_color(rgb(t.border))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(2.0))
                                    .child(
                                        div()
                                            .text_size(px(16.0))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(t.text_primary))
                                            .child("Session Manager"),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.0))
                                            .text_color(rgb(t.text_muted))
                                            .child("Save and restore workspace sessions"),
                                    ),
                            )
                            .child(
                                div()
                                    .id("session-manager-close-btn")
                                    .cursor_pointer()
                                    .px(px(8.0))
                                    .py(px(4.0))
                                    .rounded(px(4.0))
                                    .hover(|s| s.bg(rgb(t.bg_hover)))
                                    .text_size(px(16.0))
                                    .text_color(rgb(t.text_muted))
                                    .child("✕")
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _window, cx| {
                                            this.close(cx);
                                        }),
                                    ),
                            ),
                    )
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

impl_focusable!(SessionManager);
