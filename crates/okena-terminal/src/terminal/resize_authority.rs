use parking_lot::Mutex;
use std::collections::HashMap;
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

/// Per-scope resize authority: within one scope, one side owns every PTY size
/// at a time ("last to interact wins", see PR history for the flicker that
/// per-terminal ownership caused).
///
/// A scope is one server's terminals. On the daemon/server side terminal ids
/// are plain, so everything shares the "" scope (process-global, as before).
/// On a client, mirror terminals are keyed `remote:<connection>:<id>`, so each
/// connection gets its own scope — another server's owner reclaiming must not
/// stop this client from resizing an unrelated server's terminals.
static RESIZE_AUTHORITY: OnceLock<Mutex<HashMap<String, ResizeAuthorityState>>> = OnceLock::new();

fn resize_authority() -> &'static Mutex<HashMap<String, ResizeAuthorityState>> {
    RESIZE_AUTHORITY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Authority scope of a terminal id: the connection prefix for client mirror
/// ids (`remote:<connection>:<id>` → `remote:<connection>`), "" for plain
/// server-side ids.
fn scope_of(terminal_id: &str) -> &str {
    match terminal_id.rfind(':') {
        Some(idx) => &terminal_id[..idx],
        None => "",
    }
}

pub fn claim_resize_authority_local(terminal_id: &str) {
    let mut map = resize_authority().lock();
    let authority = map.entry(scope_of(terminal_id).to_string()).or_default();
    authority.seq += 1;
    authority.last_local_seq = authority.seq;
    authority.remote_owner_id = None;
    log::debug!("resize_authority: claim LOCAL (terminal={terminal_id})");
}

pub fn claim_resize_authority_remote(terminal_id: &str) {
    let mut map = resize_authority().lock();
    let authority = map.entry(scope_of(terminal_id).to_string()).or_default();
    authority.seq += 1;
    authority.last_remote_seq = authority.seq;
    authority.remote_owner_id = None;
    log::debug!("resize_authority: claim REMOTE ownerless (terminal={terminal_id})");
}

pub fn claim_resize_authority_remote_owner(terminal_id: &str, owner_id: &str) {
    let mut map = resize_authority().lock();
    let authority = map.entry(scope_of(terminal_id).to_string()).or_default();
    authority.seq += 1;
    authority.last_remote_seq = authority.seq;
    authority.remote_owner_id = Some(owner_id.to_string());
    log::debug!("resize_authority: claim REMOTE owner={owner_id} (terminal={terminal_id})");
}

pub fn claim_remote_resize_if_allowed(terminal_id: &str, owner_id: &str) -> bool {
    let mut map = resize_authority().lock();
    let authority = map.entry(scope_of(terminal_id).to_string()).or_default();

    if authority.last_local_seq == 0 && authority.last_remote_seq == 0 {
        authority.seq += 1;
        authority.last_remote_seq = authority.seq;
        authority.remote_owner_id = Some(owner_id.to_string());
        log::debug!(
            "resize_authority: resize from {owner_id} ADOPTS unclaimed (terminal={terminal_id})"
        );
        return true;
    }

    if authority.last_local_seq > authority.last_remote_seq {
        log::debug!(
            "resize_authority: resize from {owner_id} DENIED, local owns (terminal={terminal_id})"
        );
        return false;
    }

    match authority.remote_owner_id.as_deref() {
        Some(existing) => {
            let allowed = existing == owner_id;
            log::debug!(
                "resize_authority: resize from {owner_id} {} (owner={existing}, terminal={terminal_id})",
                if allowed { "ALLOWED" } else { "DENIED" }
            );
            allowed
        }
        None => {
            authority.remote_owner_id = Some(owner_id.to_string());
            log::debug!(
                "resize_authority: resize from {owner_id} ADOPTS ownerless remote (terminal={terminal_id})"
            );
            true
        }
    }
}

/// Release resize ownership held by a disconnecting connection. Keeps the
/// remote side as authority but ownerless, so the next client's resize can
/// adopt it instead of being denied by a dead owner. Owner ids are unique per
/// process, so clearing them across every scope is safe.
pub fn release_remote_resize_owner(owner_id: &str) {
    let mut map = resize_authority().lock();
    for authority in map.values_mut() {
        if authority.remote_owner_id.as_deref() == Some(owner_id) {
            authority.remote_owner_id = None;
            log::debug!("resize_authority: RELEASED owner {owner_id}");
        }
    }
}

pub fn is_resize_authority_local(terminal_id: &str) -> bool {
    resize_authority_snapshot(terminal_id).local
}

pub fn resize_authority_snapshot(terminal_id: &str) -> ResizeAuthoritySnapshot {
    let map = resize_authority().lock();
    let Some(authority) = map.get(scope_of(terminal_id)) else {
        return ResizeAuthoritySnapshot {
            local: true,
            remote_owner_id: None,
            claimed: false,
        };
    };
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
    resize_authority().lock().clear();
}
