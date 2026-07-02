use parking_lot::Mutex;
use std::sync::OnceLock;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResizeAuthoritySnapshot {
    pub local: bool,
    pub remote_owner_id: Option<String>,
    pub claimed: bool,
}

#[derive(Default)]
struct ResizeAuthorityState {
    seq: u64,
    last_local_seq: u64,
    last_remote_seq: u64,
    remote_owner_id: Option<String>,
}

/// Process-global resize authority: one side owns every PTY size at a time.
static RESIZE_AUTHORITY: OnceLock<Mutex<ResizeAuthorityState>> = OnceLock::new();

fn resize_authority() -> &'static Mutex<ResizeAuthorityState> {
    RESIZE_AUTHORITY.get_or_init(|| Mutex::new(ResizeAuthorityState::default()))
}

pub fn claim_resize_authority_local(_terminal_id: &str) {
    let mut authority = resize_authority().lock();
    authority.seq += 1;
    authority.last_local_seq = authority.seq;
    authority.remote_owner_id = None;
}

pub fn claim_resize_authority_remote(_terminal_id: &str) {
    let mut authority = resize_authority().lock();
    authority.seq += 1;
    authority.last_remote_seq = authority.seq;
    authority.remote_owner_id = None;
}

pub fn claim_resize_authority_remote_owner(_terminal_id: &str, owner_id: &str) {
    let mut authority = resize_authority().lock();
    authority.seq += 1;
    authority.last_remote_seq = authority.seq;
    authority.remote_owner_id = Some(owner_id.to_string());
}

pub fn claim_remote_resize_if_allowed(_terminal_id: &str, owner_id: &str) -> bool {
    let mut authority = resize_authority().lock();

    if authority.last_local_seq == 0 && authority.last_remote_seq == 0 {
        authority.seq += 1;
        authority.last_remote_seq = authority.seq;
        authority.remote_owner_id = Some(owner_id.to_string());
        return true;
    }

    if authority.last_local_seq > authority.last_remote_seq {
        return false;
    }

    match authority.remote_owner_id.as_deref() {
        Some(existing) => existing == owner_id,
        None => {
            authority.remote_owner_id = Some(owner_id.to_string());
            true
        }
    }
}

/// Release resize ownership held by a disconnecting connection. Keeps the
/// remote side as authority but ownerless, so the next client's resize can
/// adopt it instead of being denied by a dead owner.
pub fn release_remote_resize_owner(owner_id: &str) {
    let mut authority = resize_authority().lock();
    if authority.remote_owner_id.as_deref() == Some(owner_id) {
        authority.remote_owner_id = None;
    }
}

pub fn is_resize_authority_local(_terminal_id: &str) -> bool {
    resize_authority_snapshot("").local
}

pub fn resize_authority_snapshot(_terminal_id: &str) -> ResizeAuthoritySnapshot {
    let authority = resize_authority().lock();
    if authority.last_local_seq >= authority.last_remote_seq {
        ResizeAuthoritySnapshot {
            local: true,
            remote_owner_id: None,
            claimed: authority.last_local_seq != 0,
        }
    } else {
        ResizeAuthoritySnapshot {
            local: false,
            remote_owner_id: authority.remote_owner_id.clone(),
            claimed: true,
        }
    }
}

#[cfg(test)]
pub(super) fn reset_resize_authority() {
    *resize_authority().lock() = ResizeAuthorityState::default();
}
