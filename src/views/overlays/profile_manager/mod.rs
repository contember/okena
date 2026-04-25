mod actions;
mod render;

use crate::views::components::SimpleInputState;
use gpui::*;
use okena_core::profiles::ProfileEntry;

pub struct ProfileManager {
    pub(crate) focus_handle: FocusHandle,
    pub(crate) profiles: Vec<ProfileEntry>,
    pub(crate) active_id: String,
    pub(crate) new_profile_input: Entity<SimpleInputState>,
    pub(crate) error_message: Option<String>,
    pub(crate) show_delete_confirmation: Option<String>,
}

impl ProfileManager {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        let active_id = okena_core::profiles::try_current()
            .map(|p| p.id.clone())
            .unwrap_or_default();

        let profiles = okena_core::profiles::all_profiles().unwrap_or_default();

        let new_profile_input = cx.new(|cx| {
            SimpleInputState::new(cx).placeholder("New profile name...")
        });

        Self {
            focus_handle,
            profiles,
            active_id,
            new_profile_input,
            error_message: None,
            show_delete_confirmation: None,
        }
    }
}

pub enum ProfileManagerEvent {
    Close,
    SwitchProfile(String),
}

impl EventEmitter<ProfileManagerEvent> for ProfileManager {}

impl_focusable!(ProfileManager);
