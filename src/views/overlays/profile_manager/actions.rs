use gpui::*;

use super::{ProfileManager, ProfileManagerEvent};

impl ProfileManager {
    pub(super) fn close(&self, cx: &mut Context<Self>) {
        cx.emit(ProfileManagerEvent::Close);
    }

    pub(super) fn refresh_profiles(&mut self) {
        self.profiles = okena_core::profiles::all_profiles().unwrap_or_default();
        self.error_message = None;
    }

    pub(super) fn create_profile(&mut self, cx: &mut Context<Self>) {
        let name = self.new_profile_input.read(cx).value().trim().to_string();
        if name.is_empty() {
            self.error_message = Some("Profile name cannot be empty".to_string());
            cx.notify();
            return;
        }

        match okena_core::profiles::create_profile(&name) {
            Ok(_id) => {
                self.new_profile_input.update(cx, |input, cx| {
                    input.set_value("", cx);
                });
                self.refresh_profiles();
            }
            Err(e) => {
                self.error_message = Some(format!("Failed to create profile: {e}"));
            }
        }
        cx.notify();
    }

    pub(super) fn confirm_delete(&mut self, id: &str, cx: &mut Context<Self>) {
        self.show_delete_confirmation = Some(id.to_string());
        cx.notify();
    }

    pub(super) fn cancel_delete(&mut self, cx: &mut Context<Self>) {
        self.show_delete_confirmation = None;
        cx.notify();
    }

    pub(super) fn delete_profile(&mut self, id: &str, cx: &mut Context<Self>) {
        match okena_core::profiles::delete_profile(id) {
            Ok(()) => {
                self.show_delete_confirmation = None;
                self.refresh_profiles();
            }
            Err(e) => {
                self.show_delete_confirmation = None;
                self.error_message = Some(format!("{e}"));
            }
        }
        cx.notify();
    }

    pub(super) fn switch_to(&mut self, id: String, cx: &mut Context<Self>) {
        cx.emit(ProfileManagerEvent::SwitchProfile(id));
    }
}
