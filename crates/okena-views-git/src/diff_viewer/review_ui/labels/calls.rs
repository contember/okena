//! Call-change and signature wording — spec §9. Shared by the file view's
//! details bar and the navigator's inline outline, so both read the same.
//! Pure; no GPUI.

use super::super::model::CallRow;
use okena_review::CallChangeKind;

const DOT: &str = " \u{00B7} ";
const ARROW: &str = "\u{2192}";
/// A call outside every branch, when the other side of a move had one.
const TOP_LEVEL: &str = "top level";

/// `+`, `−` or `~` — which way the call went.
pub(crate) fn call_marker(change: CallChangeKind) -> &'static str {
    match change {
        CallChangeKind::Added => "+",
        CallChangeKind::Removed => "\u{2212}",
        CallChangeKind::Modified => "~",
    }
}

/// `retry(3) → (retries)` for a modified call whose arguments changed,
/// `callee(args)` otherwise — a call that only moved between branches keeps
/// its one text, and `call_context` tells the move.
pub(crate) fn call_text(row: &CallRow) -> String {
    let old = row.old_args.as_deref().unwrap_or_default();
    let new = row.new_args.as_deref().unwrap_or_default();
    match row.change {
        CallChangeKind::Added => format!("{}{new}", row.callee),
        CallChangeKind::Removed => format!("{}{old}", row.callee),
        CallChangeKind::Modified if old == new => format!("{}{new}", row.callee),
        CallChangeKind::Modified => format!("{}{old} {ARROW} {new}", row.callee),
    }
}

/// `in condition` — the branch the call sits in, outermost first; a call that
/// moved reads `in loop → loop · closure`. Top level on both sides says nothing.
pub(crate) fn call_context(row: &CallRow) -> Option<String> {
    let stack = |context: &[String]| {
        if context.is_empty() {
            TOP_LEVEL.to_string()
        } else {
            context.join(DOT)
        }
    };
    match row.old_context.as_deref() {
        Some(old) => Some(format!("in {} {ARROW} {}", stack(old), stack(&row.context))),
        None if row.context.is_empty() => None,
        None => Some(format!("in {}", stack(&row.context))),
    }
}

/// Widest a call reads before it is cut.
pub(crate) const CALL_TEXT_CHARS: usize = 96;
const ELLIPSIS: char = '\u{2026}';

/// One call as the details block lists it: one line, whatever the source did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CallLine {
    pub change: CallChangeKind,
    pub text: String,
    pub context: Option<String>,
}

/// The lines the details block shows, and how many it left out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CallLines {
    pub shown: Vec<CallLine>,
    pub hidden: usize,
}

impl CallLines {
    /// `… 12 more`, or nothing when every call is listed.
    pub(crate) fn hidden_note(&self) -> Option<String> {
        (self.hidden > 0).then(|| format!("{ELLIPSIS} {} more", self.hidden))
    }
}

/// Removed and modified calls first — they are what changed behaviour; then
/// added, each side in source order. At most `limit` lines.
pub(crate) fn call_lines(calls: &[CallRow], limit: usize) -> CallLines {
    let mut ordered: Vec<&CallRow> = calls.iter().collect();
    ordered.sort_by_key(|row| match row.change {
        CallChangeKind::Removed => 0,
        CallChangeKind::Modified => 1,
        CallChangeKind::Added => 2,
    });
    let shown = ordered
        .iter()
        .take(limit)
        .map(|row| CallLine {
            change: row.change,
            text: one_line(&call_text(row), CALL_TEXT_CHARS),
            context: call_context(row),
        })
        .collect();
    CallLines {
        shown,
        hidden: ordered.len().saturating_sub(limit),
    }
}

/// Collapse every whitespace run to one space and cut at `max_chars`.
fn one_line(text: &str, max_chars: usize) -> String {
    let joined = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if joined.chars().count() <= max_chars {
        return joined;
    }
    let mut out: String = joined.chars().take(max_chars.saturating_sub(1)).collect();
    out.push(ELLIPSIS);
    out
}

/// `(&self, t) → (&self, t, cx)` — the signature pair on one line, for a
/// column too narrow for the two-line token diff.
pub(crate) fn signature_pair(old: &str, new: &str, max_chars: usize) -> String {
    one_line(&format!("{old} {ARROW} {new}"), max_chars)
}

#[cfg(test)]
mod tests {
    use super::super::super::model::CallRow;
    use super::{call_context, call_lines, call_marker, call_text, one_line, signature_pair};
    use okena_review::CallChangeKind;

    #[test]
    fn call_rows_read_as_signed_lines_with_their_branch() {
        let row = |change, old: Option<&str>, new: Option<&str>, context: Vec<String>| CallRow {
            change,
            callee: "retry".into(),
            old_args: old.map(str::to_string),
            new_args: new.map(str::to_string),
            context,
            old_context: None,
        };

        let added = row(CallChangeKind::Added, None, Some("(value)"), Vec::new());
        assert_eq!(call_marker(added.change), "+");
        assert_eq!(call_text(&added), "retry(value)");
        assert_eq!(call_context(&added), None);

        let removed = row(
            CallChangeKind::Removed,
            Some("(input)"),
            None,
            vec!["error branch".into()],
        );
        assert_eq!(call_marker(removed.change), "\u{2212}");
        assert_eq!(call_text(&removed), "retry(input)");
        assert_eq!(call_context(&removed), Some("in error branch".to_string()));

        let modified = row(
            CallChangeKind::Modified,
            Some("(3)"),
            Some("(retries)"),
            vec!["condition".into(), "loop".into()],
        );
        assert_eq!(call_marker(modified.change), "~");
        assert_eq!(call_text(&modified), "retry(3) \u{2192} (retries)");
        assert_eq!(
            call_context(&modified),
            Some("in condition \u{00B7} loop".to_string())
        );

        // Same arguments, moved into a closure: one text, the move in the context.
        let mut moved = row(
            CallChangeKind::Modified,
            Some("(3)"),
            Some("(3)"),
            vec!["loop".into(), "closure".into()],
        );
        moved.old_context = Some(vec!["loop".into()]);
        assert_eq!(call_text(&moved), "retry(3)");
        assert_eq!(
            call_context(&moved),
            Some("in loop \u{2192} loop \u{00B7} closure".to_string())
        );
        moved.old_context = Some(Vec::new());
        assert_eq!(
            call_context(&moved),
            Some("in top level \u{2192} loop \u{00B7} closure".to_string())
        );
    }

    #[test]
    fn call_lines_read_one_line_each_and_list_what_changed_first() {
        let row = |change: CallChangeKind, callee: &str, args: &str| CallRow {
            change,
            callee: callee.to_string(),
            old_args: Some(args.to_string()),
            new_args: Some(args.to_string()),
            context: Vec::new(),
            old_context: None,
        };
        let calls = vec![
            row(CallChangeKind::Added, "log", "(a)"),
            row(
                CallChangeKind::Removed,
                "result.set",
                "(name, {\n    fields: [],\n})",
            ),
            row(CallChangeKind::Added, "warn", "(b)"),
            row(CallChangeKind::Modified, "retry", "(3)"),
        ];
        let lines = call_lines(&calls, 3);
        assert_eq!(lines.hidden, 1);
        assert_eq!(lines.hidden_note().as_deref(), Some("\u{2026} 1 more"));
        assert_eq!(
            lines
                .shown
                .iter()
                .map(|line| line.change)
                .collect::<Vec<_>>(),
            vec![
                CallChangeKind::Removed,
                CallChangeKind::Modified,
                CallChangeKind::Added
            ]
        );
        assert_eq!(lines.shown[0].text, "result.set(name, { fields: [], })");
        assert_eq!(lines.shown[2].text, "log(a)");
        assert_eq!(call_lines(&calls, 8).hidden_note(), None);
    }

    #[test]
    fn one_line_collapses_whitespace_and_cuts_with_an_ellipsis() {
        assert_eq!(one_line("a  b\n\t c", 10), "a b c");
        assert_eq!(one_line("abcdefghij", 10), "abcdefghij");
        assert_eq!(one_line("abcdefghijk", 10), "abcdefghi\u{2026}");
    }

    #[test]
    fn a_signature_pair_reads_on_one_line() {
        assert_eq!(
            signature_pair("fn f(&self, t)", "fn f(&self,\n  t, cx)", 96),
            "fn f(&self, t) \u{2192} fn f(&self, t, cx)"
        );
    }
}
