use crate::settings::settings_entity;
use crate::workspace::persistence::{
    delete_session, export_workspace, import_workspace, load_session, rename_session, save_session,
    session_exists,
};
use gpui::*;

use super::{SessionManager, SessionManagerEvent};

impl SessionManager {
    pub(super) fn close(&self, cx: &mut Context<Self>) {
        cx.emit(SessionManagerEvent::Close);
    }

    pub(super) fn refresh_sessions(&mut self) {
        self.sessions = crate::workspace::persistence::list_sessions().unwrap_or_default();
        self.error_message = None;
    }

    pub(super) fn save_new_session(&mut self, cx: &mut Context<Self>) {
        let name = self.new_session_input.read(cx).value().trim().to_string();
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
                self.new_session_input.update(cx, |input, cx| {
                    input.set_value("", cx);
                });
                self.refresh_sessions();
                self.error_message = None;
            }
            Err(e) => {
                self.error_message = Some(format!("Failed to save session: {}", e));
            }
        }
        cx.notify();
    }

    pub(super) fn load_session(&mut self, name: &str, cx: &mut Context<Self>) {
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

    pub(super) fn start_rename(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let rename_input = cx.new(|cx| {
            crate::views::components::SimpleInputState::new(cx)
                .placeholder("Session name...")
                .default_value(name)
        });
        rename_input.update(cx, |input, cx| {
            input.focus(window, cx);
        });
        self.rename_input = Some(rename_input);
        self.renaming_session = Some(name.to_string());
        cx.notify();
    }

    pub(super) fn cancel_rename(&mut self, cx: &mut Context<Self>) {
        self.renaming_session = None;
        self.rename_input = None;
        cx.notify();
    }

    pub(super) fn confirm_rename(&mut self, cx: &mut Context<Self>) {
        let new_name = self.rename_input
            .as_ref()
            .map(|input| input.read(cx).value().trim().to_string())
            .unwrap_or_default();

        if let Some(old_name) = self.renaming_session.take() {
            if new_name.is_empty() {
                self.error_message = Some("Session name cannot be empty".to_string());
                self.rename_input = None;
                cx.notify();
                return;
            }

            if new_name != old_name && session_exists(&new_name) {
                self.error_message = Some(format!("Session '{}' already exists", new_name));
                self.rename_input = None;
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
        self.rename_input = None;
        cx.notify();
    }

    pub(super) fn confirm_delete(&mut self, name: &str, cx: &mut Context<Self>) {
        self.show_delete_confirmation = Some(name.to_string());
        cx.notify();
    }

    pub(super) fn cancel_delete(&mut self, cx: &mut Context<Self>) {
        self.show_delete_confirmation = None;
        cx.notify();
    }

    pub(super) fn delete_session(&mut self, name: &str, cx: &mut Context<Self>) {
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

    pub(super) fn export_current(&mut self, cx: &mut Context<Self>) {
        let path = self.export_path_input.read(cx).value().trim().to_string();
        if path.is_empty() {
            self.error_message = Some("Export path cannot be empty".to_string());
            cx.notify();
            return;
        }

        let data = self.workspace.read(cx).data.clone();
        match export_workspace(&data, std::path::Path::new(&path)) {
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

    pub(super) fn import_from_file(&mut self, cx: &mut Context<Self>) {
        let path = self.import_path_input.read(cx).value().trim().to_string();
        if path.is_empty() {
            self.error_message = Some("Import path cannot be empty".to_string());
            cx.notify();
            return;
        }

        match import_workspace(std::path::Path::new(&path)) {
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
}
