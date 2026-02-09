/// Prefix format: "remote:{connection_id}:{terminal_id}"
pub fn make_prefixed_id(connection_id: &str, terminal_id: &str) -> String {
    format!("remote:{}:{}", connection_id, terminal_id)
}

/// Strip the "remote:{connection_id}:" prefix from a terminal ID.
/// If the ID doesn't have the expected prefix, returns it unchanged.
pub fn strip_prefix(terminal_id: &str, connection_id: &str) -> String {
    let prefix = format!("remote:{}:", connection_id);
    if let Some(stripped) = terminal_id.strip_prefix(&prefix) {
        stripped.to_string()
    } else {
        terminal_id.to_string()
    }
}

/// Check if a terminal ID belongs to a specific remote connection.
pub fn is_remote_terminal(terminal_id: &str, connection_id: &str) -> bool {
    let prefix = format!("remote:{}:", connection_id);
    terminal_id.starts_with(&prefix)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn make_prefixed_id_format() {
        assert_eq!(
            make_prefixed_id("conn-1", "term-a"),
            "remote:conn-1:term-a"
        );
    }

    #[test]
    fn strip_prefix_valid() {
        assert_eq!(
            strip_prefix("remote:conn-1:term-a", "conn-1"),
            "term-a"
        );
    }

    #[test]
    fn strip_prefix_no_match_returns_original() {
        assert_eq!(
            strip_prefix("remote:other:term-a", "conn-1"),
            "remote:other:term-a"
        );
        assert_eq!(strip_prefix("plain-id", "conn-1"), "plain-id");
    }

    #[test]
    fn is_remote_terminal_true_and_false() {
        assert!(is_remote_terminal("remote:conn-1:term-a", "conn-1"));
        assert!(!is_remote_terminal("remote:other:term-a", "conn-1"));
        assert!(!is_remote_terminal("plain-id", "conn-1"));
    }
}
