use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::OnceLock;

#[derive(Clone, Debug, PartialEq, Eq)]
enum ResizeAuthority {
    Local,
    Remote { owner_id: Option<String> },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResizeAuthoritySnapshot {
    pub local: bool,
    pub remote_owner_id: Option<String>,
}

static RESIZE_AUTHORITIES: OnceLock<Mutex<HashMap<String, ResizeAuthority>>> = OnceLock::new();

fn resize_authorities() -> &'static Mutex<HashMap<String, ResizeAuthority>> {
    RESIZE_AUTHORITIES.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn claim_resize_authority_local(terminal_id: &str) {
    resize_authorities()
        .lock()
        .insert(terminal_id.to_string(), ResizeAuthority::Local);
}

pub fn claim_resize_authority_remote(terminal_id: &str) {
    resize_authorities()
        .lock()
        .insert(terminal_id.to_string(), ResizeAuthority::Remote { owner_id: None });
}

pub fn claim_resize_authority_remote_owner(terminal_id: &str, owner_id: &str) {
    resize_authorities().lock().insert(
        terminal_id.to_string(),
        ResizeAuthority::Remote {
            owner_id: Some(owner_id.to_string()),
        },
    );
}

pub fn claim_remote_resize_if_allowed(terminal_id: &str, owner_id: &str) -> bool {
    let mut authorities = resize_authorities().lock();
    match authorities.get(terminal_id) {
        None => {
            authorities.insert(
                terminal_id.to_string(),
                ResizeAuthority::Remote {
                    owner_id: Some(owner_id.to_string()),
                },
            );
            true
        }
        Some(ResizeAuthority::Local) => false,
        Some(ResizeAuthority::Remote { owner_id: None }) => {
            authorities.insert(
                terminal_id.to_string(),
                ResizeAuthority::Remote {
                    owner_id: Some(owner_id.to_string()),
                },
            );
            true
        }
        Some(ResizeAuthority::Remote { owner_id: Some(existing) }) => existing == owner_id,
    }
}

pub fn is_resize_authority_local(terminal_id: &str) -> bool {
    resize_authority_snapshot(terminal_id).local
}

pub fn resize_authority_snapshot(terminal_id: &str) -> ResizeAuthoritySnapshot {
    match resize_authorities().lock().get(terminal_id).cloned() {
        Some(ResizeAuthority::Remote { owner_id }) => ResizeAuthoritySnapshot {
            local: false,
            remote_owner_id: owner_id,
        },
        Some(ResizeAuthority::Local) | None => ResizeAuthoritySnapshot {
            local: true,
            remote_owner_id: None,
        },
    }
}

#[cfg(test)]
pub(super) fn reset_resize_authority() {
    resize_authorities().lock().clear();
}
