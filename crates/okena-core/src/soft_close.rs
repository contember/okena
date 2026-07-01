//! Shared encoding of soft-close toast-action ids, used by the daemon (which
//! builds the Undo/Close-now toast) and the GUI client (which decodes a clicked
//! action id back into the project/terminal it targets).

/// Toast-action id prefix for the soft-close "Undo" button.
pub const SOFT_CLOSE_UNDO_PREFIX: &str = "soft_close_undo";
/// Toast-action id prefix for the soft-close "Close now" button.
pub const SOFT_CLOSE_KILL_PREFIX: &str = "soft_close_kill";

/// Encode a toast-action id as `<prefix>:<project_id>:<terminal_id>`.
pub fn encode_action(prefix: &str, project_id: &str, terminal_id: &str) -> String {
    format!(
        "{prefix}:{}:{}",
        encode_component(project_id),
        encode_component(terminal_id)
    )
}

/// Decode `<prefix>:<project_id>:<terminal_id>` → `(project_id, terminal_id)`
/// when `prefix` matches.
pub fn decode_action(id: &str, prefix: &str) -> Option<(String, String)> {
    let rest = id.strip_prefix(prefix)?.strip_prefix(':')?;
    let (project_id, terminal_id) = rest.split_once(':')?;
    Some((
        decode_component(project_id)?,
        decode_component(terminal_id)?,
    ))
}

fn encode_component(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '%' => encoded.push_str("%25"),
            ':' => encoded.push_str("%3A"),
            _ => encoded.push(ch),
        }
    }
    encoded
}

fn decode_component(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hi = *bytes.get(i + 1)?;
            let lo = *bytes.get(i + 2)?;
            decoded.push(hex_value(hi)? << 4 | hex_value(lo)?);
            i += 3;
        } else {
            decoded.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

fn hex_value(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn round_trips() {
        let id = encode_action(SOFT_CLOSE_UNDO_PREFIX, "p", "t");
        assert_eq!(id, "soft_close_undo:p:t");
        assert_eq!(
            decode_action(&id, SOFT_CLOSE_UNDO_PREFIX),
            Some(("p".into(), "t".into()))
        );
        assert_eq!(decode_action(&id, SOFT_CLOSE_KILL_PREFIX), None);
    }
    #[test]
    fn rejects_malformed() {
        assert_eq!(
            decode_action("soft_close_undo:onlyone", SOFT_CLOSE_UNDO_PREFIX),
            None
        );
        assert_eq!(decode_action("garbage", SOFT_CLOSE_UNDO_PREFIX), None);
    }

    #[test]
    fn round_trips_remote_prefixed_ids() {
        let project_id = "remote:local-daemon:p";
        let terminal_id = "remote:local-daemon:t";
        let id = encode_action(SOFT_CLOSE_UNDO_PREFIX, project_id, terminal_id);
        assert_eq!(
            id,
            "soft_close_undo:remote%3Alocal-daemon%3Ap:remote%3Alocal-daemon%3At"
        );
        assert_eq!(
            decode_action(&id, SOFT_CLOSE_UNDO_PREFIX),
            Some((project_id.into(), terminal_id.into()))
        );
    }

    #[test]
    fn round_trips_percent_signs() {
        let id = encode_action(SOFT_CLOSE_UNDO_PREFIX, "p%25", "t%3A");
        assert_eq!(
            decode_action(&id, SOFT_CLOSE_UNDO_PREFIX),
            Some(("p%25".into(), "t%3A".into()))
        );
    }

    #[test]
    fn rejects_bad_escape_sequences() {
        assert_eq!(
            decode_action("soft_close_undo:p%:t", SOFT_CLOSE_UNDO_PREFIX),
            None
        );
        assert_eq!(
            decode_action("soft_close_undo:p%xx:t", SOFT_CLOSE_UNDO_PREFIX),
            None
        );
    }
}
