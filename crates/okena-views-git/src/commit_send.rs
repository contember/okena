//! Shared formatting for "Send commit to terminal" — produces a `git log`-style block
//! used by the diff viewer and the commit graph context menu.

use okena_git::{format_relative_time, CommitLogEntry};

/// Format a commit reference for pasting into a terminal. Mirrors the
/// `git log` default output:
///
/// ```text
/// commit <hash> (HEAD -> main)
/// Author: Jane Doe
/// Date:   2 days ago
///
///     Subject line
/// ```
///
/// The returned string may end with a newline; the central send-to-terminal
/// dispatcher (`SendPayload::format`) strips trailing LF before pasting so the
/// bracketed-paste isn't interpreted as Enter by receivers like Claude/Codex.
pub fn format_commit_send_text(
    hash: &str,
    message: Option<&str>,
    author: Option<&str>,
    timestamp: Option<i64>,
    refs: &[String],
) -> String {
    let mut out = String::new();
    out.push_str("commit ");
    out.push_str(hash);
    if !refs.is_empty() {
        out.push_str(" (");
        out.push_str(&refs.join(", "));
        out.push(')');
    }
    out.push('\n');
    if let Some(author) = author {
        out.push_str("Author: ");
        out.push_str(author);
        out.push('\n');
    }
    if let Some(ts) = timestamp {
        out.push_str("Date:   ");
        out.push_str(&format_relative_time(ts));
        out.push('\n');
    }
    if let Some(msg) = message {
        let trimmed = msg.trim();
        if !trimmed.is_empty() {
            out.push('\n');
            let lines: Vec<&str> = trimmed.lines().collect();
            for (i, line) in lines.iter().enumerate() {
                out.push_str("    ");
                out.push_str(line);
                if i + 1 < lines.len() {
                    out.push('\n');
                }
            }
        }
    }
    out
}

/// Convenience: format a `CommitLogEntry` for sending.
pub fn format_commit_entry(entry: &CommitLogEntry) -> String {
    format_commit_send_text(
        &entry.hash,
        Some(&entry.message),
        Some(&entry.author),
        Some(entry.timestamp),
        &entry.refs,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A timestamp far enough in the past that `format_relative_time` produces a
    /// stable "Nw ago" string regardless of when the test runs.
    /// (Roughly 30 weeks before the test was written.)
    const STABLE_PAST_TIMESTAMP: i64 = 1_700_000_000;

    #[test]
    fn bare_hash_produces_single_line() {
        let s = format_commit_send_text("abc1234", None, None, None, &[]);
        // Trailing LF is stripped centrally by SendPayload::format.
        assert_eq!(s, "commit abc1234\n");
    }

    #[test]
    fn refs_appended_in_parens_after_hash() {
        let s = format_commit_send_text(
            "abc1234",
            None,
            None,
            None,
            &["HEAD -> main".to_string(), "origin/main".to_string()],
        );
        assert_eq!(s, "commit abc1234 (HEAD -> main, origin/main)\n");
    }

    #[test]
    fn full_form_matches_git_log_default_layout() {
        let s = format_commit_send_text(
            "abc1234",
            Some("fix the thing"),
            Some("Jane Doe"),
            Some(STABLE_PAST_TIMESTAMP),
            &["HEAD -> main".to_string()],
        );
        let mut lines = s.lines();
        assert_eq!(lines.next(), Some("commit abc1234 (HEAD -> main)"));
        assert_eq!(lines.next(), Some("Author: Jane Doe"));
        assert!(lines.next().unwrap().starts_with("Date:"));
        assert_eq!(lines.next(), Some(""));
        assert_eq!(lines.next(), Some("    fix the thing"));
        assert_eq!(lines.next(), None);
    }

    #[test]
    fn multi_line_message_is_indented_and_preserved() {
        let s = format_commit_send_text(
            "abc1234",
            Some("subject\n\nfirst body line\nsecond body line"),
            None,
            None,
            &[],
        );
        let mut lines = s.lines();
        assert_eq!(lines.next(), Some("commit abc1234"));
        assert_eq!(lines.next(), Some(""));
        assert_eq!(lines.next(), Some("    subject"));
        assert_eq!(lines.next(), Some("    "));
        assert_eq!(lines.next(), Some("    first body line"));
        assert_eq!(lines.next(), Some("    second body line"));
        assert_eq!(lines.next(), None);
    }

    #[test]
    fn whitespace_only_message_is_dropped() {
        let s = format_commit_send_text("abc1234", Some("   \n\n  "), None, None, &[]);
        assert_eq!(s, "commit abc1234\n");
    }

    #[test]
    fn wrapping_in_send_payload_strips_trailing_lf() {
        use okena_core::send_payload::SendPayload;
        let text = format_commit_send_text("abc1234", Some("subject"), None, None, &[]);
        let formatted = SendPayload::Text(text).format(None);
        assert!(!formatted.ends_with('\n'), "got: {:?}", formatted);
        assert!(formatted.ends_with("    subject"));
    }

    #[test]
    fn entry_wrapper_uses_all_fields() {
        let entry = CommitLogEntry {
            hash: "abc1234".into(),
            message: "subject".into(),
            author: "Jane".into(),
            timestamp: STABLE_PAST_TIMESTAMP,
            is_merge: false,
            graph: String::new(),
            refs: vec!["main".into()],
        };
        let s = format_commit_entry(&entry);
        assert!(s.starts_with("commit abc1234 (main)"));
        assert!(s.contains("Author: Jane"));
        assert!(s.contains("Date:"));
        assert!(s.contains("    subject"));
    }
}
