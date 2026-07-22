use crate::workspace::persistence::SessionInfo;
use gpui::*;
use okena_core::api::ActionRequest;

use super::{SessionManager, SessionManagerEvent};

impl SessionManager {
    pub(super) fn close(&self, cx: &mut Context<Self>) {
        cx.emit(SessionManagerEvent::Close);
    }

    pub(super) fn refresh_sessions(&mut self, cx: &mut Context<Self>) {
        self.loading_sessions = true;
        self.error_message = None;
        cx.notify();

        let client = self.client.clone();
        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || {
                client
                    .post_action(ActionRequest::ListSessions)
                    .and_then(|value| value.ok_or_else(|| "Missing session list".to_string()))
                    .and_then(|value| {
                        serde_json::from_value::<Vec<SessionInfo>>(value)
                            .map_err(|error| format!("Invalid session list: {error}"))
                    })
            })
            .await;

            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    match result {
                        Ok(sessions) => this.sessions = sessions,
                        Err(error) => this.error_message = Some(error),
                    }
                    this.loading_sessions = false;
                    cx.notify();
                });
            });
        })
        .detach();
    }

    pub(super) fn save_new_session(&mut self, cx: &mut Context<Self>) {
        let name = self.new_session_input.read(cx).value().trim().to_string();
        if name.is_empty() {
            self.error_message = Some("Session name cannot be empty".to_string());
            cx.notify();
            return;
        }

        if self.sessions.iter().any(|session| session.name == name) {
            self.error_message = Some(format!("Session '{}' already exists", name));
            cx.notify();
            return;
        }

        // The daemon owns the authoritative workspace (local ids) + session
        // files; saving from the client mirror would persist prefixed-id garbage.
        // Dispatch SaveSession and let the daemon write its own data.
        cx.emit(SessionManagerEvent::Action(ActionRequest::SaveSession {
            name,
        }));
        self.new_session_input.update(cx, |input, cx| {
            input.set_value("", cx);
        });
        self.error_message = None;
        cx.notify();
    }

    pub(super) fn load_session(&mut self, name: &str, cx: &mut Context<Self>) {
        // The daemon loads its own session file + swaps state; the new workspace
        // mirrors back via snapshot.
        cx.emit(SessionManagerEvent::Action(ActionRequest::LoadSession {
            name: name.to_string(),
        }));
        self.error_message = None;
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
        let new_name = self
            .rename_input
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

            if new_name != old_name && self.sessions.iter().any(|session| session.name == new_name)
            {
                self.error_message = Some(format!("Session '{}' already exists", new_name));
                self.rename_input = None;
                cx.notify();
                return;
            }

            if new_name != old_name {
                self.loading_sessions = true;
                self.error_message = None;
                let client = self.client.clone();
                cx.spawn(async move |this, cx| {
                    let result = smol::unblock(move || {
                        client.post_action(ActionRequest::RenameSession { old_name, new_name })
                    })
                    .await;

                    cx.update(|cx| {
                        let _ = this.update(cx, |this, cx| match result {
                            Ok(_) => this.refresh_sessions(cx),
                            Err(error) => {
                                this.loading_sessions = false;
                                this.error_message = Some(error);
                                cx.notify();
                            }
                        });
                    });
                })
                .detach();
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
        self.show_delete_confirmation = None;
        self.loading_sessions = true;
        self.error_message = None;
        cx.notify();

        let client = self.client.clone();
        let name = name.to_string();
        cx.spawn(async move |this, cx| {
            let result =
                smol::unblock(move || client.post_action(ActionRequest::DeleteSession { name }))
                    .await;

            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| match result {
                    Ok(_) => this.refresh_sessions(cx),
                    Err(error) => {
                        this.loading_sessions = false;
                        this.error_message = Some(error);
                        cx.notify();
                    }
                });
            });
        })
        .detach();
    }

    pub(super) fn export_current(&mut self, cx: &mut Context<Self>) {
        let path = self.export_path_input.read(cx).value().trim().to_string();
        if path.is_empty() {
            self.error_message = Some("Export path cannot be empty".to_string());
            cx.notify();
            return;
        }

        // Export the DAEMON's authoritative workspace (not the client mirror).
        cx.emit(SessionManagerEvent::Action(
            ActionRequest::ExportWorkspace { path },
        ));
        self.error_message = None;
        cx.notify();
    }

    pub(super) fn import_from_file(&mut self, cx: &mut Context<Self>) {
        let path = self.import_path_input.read(cx).value().trim().to_string();
        if path.is_empty() {
            self.error_message = Some("Import path cannot be empty".to_string());
            cx.notify();
            return;
        }

        // The daemon imports the file + swaps state; the result mirrors back.
        cx.emit(SessionManagerEvent::Action(
            ActionRequest::ImportWorkspace { path },
        ));
        self.error_message = None;
        cx.notify();
    }
}
