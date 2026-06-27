//! Shared encoding of soft-close toast-action ids, used by the daemon (which
//! builds the Undo/Close-now toast) and the GUI client (which decodes a clicked
//! action id back into the project/terminal it targets).

/// Toast-action id prefix for the soft-close "Undo" button.
pub const SOFT_CLOSE_UNDO_PREFIX: &str = "soft_close_undo";
/// Toast-action id prefix for the soft-close "Close now" button.
pub const SOFT_CLOSE_KILL_PREFIX: &str = "soft_close_kill";

/// Encode a toast-action id as `<prefix>:<project_id>:<terminal_id>`.
pub fn encode_action(prefix: &str, project_id: &str, terminal_id: &str) -> String {
    format!("{prefix}:{project_id}:{terminal_id}")
}

/// Decode `<prefix>:<project_id>:<terminal_id>` → `(project_id, terminal_id)`
/// when `prefix` matches.
pub fn decode_action(id: &str, prefix: &str) -> Option<(String, String)> {
    let rest = id.strip_prefix(prefix)?.strip_prefix(':')?;
    let (project_id, terminal_id) = rest.split_once(':')?;
    Some((project_id.to_string(), terminal_id.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn round_trips() {
        let id = encode_action(SOFT_CLOSE_UNDO_PREFIX, "p", "t");
        assert_eq!(id, "soft_close_undo:p:t");
        assert_eq!(decode_action(&id, SOFT_CLOSE_UNDO_PREFIX), Some(("p".into(), "t".into())));
        assert_eq!(decode_action(&id, SOFT_CLOSE_KILL_PREFIX), None);
    }
    #[test]
    fn rejects_malformed() {
        assert_eq!(decode_action("soft_close_undo:onlyone", SOFT_CLOSE_UNDO_PREFIX), None);
        assert_eq!(decode_action("garbage", SOFT_CLOSE_UNDO_PREFIX), None);
    }
}
