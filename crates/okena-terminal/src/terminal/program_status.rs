//! OSC 7501 — the Program Status Protocol.
//!
//! A program reports `ESC ] 7501 ; key=value(:key=value)* ST` about itself:
//! `state` (required), `id` (hierarchical record path), `kind`, `progress`,
//! `app`, and base64 `title` / `msg`. A pane holds a set of records keyed by
//! `id`; the one with the highest attention priority becomes the pane's agent
//! status. `ESC ] 7501 ; ? ST` is the support probe, answered with the same
//! body. Spec: <https://mitchellh.com/writing/program-status-osc7501>.
//!
//! Every report is untrusted and validated whole before any record changes: a
//! report that breaks a limit is discarded, never partially applied.

use std::collections::{BTreeMap, HashMap};

use base64::Engine as _;
use okena_core::agent_status::{AgentLifecycle, AgentStatus};

/// The reply to the `?` probe. A bare `?` advertises exactly the base states
/// and kinds, which is what Okena supports.
pub(super) const QUERY_REPLY: &[u8] = b"\x1b]7501;?\x1b\\";

const MAX_SEQUENCE_LEN: usize = 4096;
const MAX_KEY_LEN: usize = 16;
const MAX_MSG_ENCODED: usize = 2732;
const MAX_MSG_DECODED: usize = 2048;
const MAX_TITLE_ENCODED: usize = 256;
const MAX_TITLE_DECODED: usize = 192;
const MAX_APP_LEN: usize = 32;
const MAX_ID_LEN: usize = 128;
const MAX_SEGMENT_LEN: usize = 32;
const MAX_ID_DEPTH: usize = 8;
/// Records kept per pane; the least recently updated one is evicted.
const MAX_RECORDS: usize = 256;

/// One parsed OSC 7501 sequence.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Report {
    /// The `?` support probe.
    Query,
    /// Replace the record at `id` (`None` = the root record).
    Set { id: Option<String>, record: Record },
    /// Remove the record at `id` and everything beneath it; `None` removes all.
    Clear { id: Option<String> },
}

/// What a program last reported for one `id`. Each report replaces it whole.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Record {
    pub state: AgentLifecycle,
    pub kind: Option<&'static str>,
    pub progress: Option<u8>,
    pub app: Option<String>,
    pub title: Option<String>,
    pub msg: Option<String>,
}

/// Parse the body of an `OSC 7501` (everything after `7501;`). Returns `None`
/// for a report the spec says to ignore.
pub(super) fn parse(body: &[u8]) -> Option<Report> {
    if body.len() + b"7501;".len() > MAX_SEQUENCE_LEN {
        return None;
    }
    let body = std::str::from_utf8(body).ok()?;
    if body.trim() == "?" {
        return Some(Report::Query);
    }

    let mut fields: HashMap<&str, &str> = HashMap::new();
    for pair in body.split(':') {
        let Some((key, value)) = pair.split_once('=') else {
            continue;
        };
        let (key, value) = (key.trim(), value.trim());
        if key.len() > MAX_KEY_LEN {
            return None;
        }
        if key.is_empty()
            || !key.bytes().all(|b| b.is_ascii_lowercase())
            || !value.bytes().all(is_value_byte)
        {
            continue;
        }
        // Last value wins for a repeated key.
        fields.insert(key, value);
    }

    let state = fields.get("state").copied()?;
    let id = match fields.get("id") {
        Some(id) => Some(valid_id(id)?.to_string()),
        None => None,
    };
    if state == "clear" {
        return Some(Report::Clear { id });
    }
    let state = AgentLifecycle::from_token(state)?;

    let title = decode_text(fields.get("title"), MAX_TITLE_ENCODED, MAX_TITLE_DECODED)?;
    let msg = decode_text(fields.get("msg"), MAX_MSG_ENCODED, MAX_MSG_DECODED)?;
    let app = fields
        .get("app")
        .filter(|app| (1..=MAX_APP_LEN).contains(&app.len()) && app.bytes().all(is_segment_byte))
        .map(|app| app.to_string());
    let kind = fields
        .get("kind")
        .filter(|_| state == AgentLifecycle::Blocked)
        .and_then(|kind| match *kind {
            "permission" => Some("permission"),
            "question" => Some("question"),
            "auth" => Some("auth"),
            _ => None,
        });
    let progress = fields
        .get("progress")
        .filter(|_| matches!(state, AgentLifecycle::Working | AgentLifecycle::Blocked))
        .and_then(|p| p.parse::<u8>().ok())
        .filter(|p| *p <= 100);

    Some(Report::Set {
        id,
        record: Record {
            state,
            kind,
            progress,
            app,
            title,
            msg,
        },
    })
}

fn is_value_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b',' | b'+' | b'/' | b'=' | b'-')
}

fn is_segment_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'+' | b'-')
}

fn valid_id(id: &str) -> Option<&str> {
    if id.len() > MAX_ID_LEN {
        return None;
    }
    let mut depth = 0;
    for segment in id.split('/') {
        depth += 1;
        if depth > MAX_ID_DEPTH
            || !(1..=MAX_SEGMENT_LEN).contains(&segment.len())
            || !segment.bytes().all(is_segment_byte)
        {
            return None;
        }
    }
    Some(id)
}

/// Decode a base64 `title` / `msg`. The outer `None` discards the whole report
/// (over a limit, undecodable, or carrying control characters); the inner one
/// is an absent or empty field.
fn decode_text(
    field: Option<&&str>,
    max_encoded: usize,
    max_decoded: usize,
) -> Option<Option<String>> {
    let Some(encoded) = field else {
        return Some(None);
    };
    if encoded.len() > max_encoded {
        return None;
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(encoded))
        .ok()?;
    if bytes.len() > max_decoded {
        return None;
    }
    let text = String::from_utf8(bytes).ok()?;
    if text.chars().any(char::is_control) {
        return None;
    }
    let text: String = text.chars().filter(|c| !is_invisible_format(*c)).collect();
    Some((!text.is_empty()).then_some(text))
}

/// Bidi overrides and invisible formatting characters, which could make text
/// shown outside the grid read differently from what the program sent.
fn is_invisible_format(c: char) -> bool {
    matches!(
        c,
        '\u{00AD}'
            | '\u{061C}'
            | '\u{180E}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{2069}'
            | '\u{FEFF}'
    )
}

/// The records of one pane.
#[derive(Default)]
pub(super) struct ProgramStatus {
    /// Keyed by `id`; the root record is `""`. The `u64` is the update clock,
    /// for eviction and for breaking priority ties toward the newest.
    records: HashMap<String, (Record, u64)>,
    clock: u64,
}

impl ProgramStatus {
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Apply a `Set` or `Clear` report. Returns whether any record changed.
    pub fn apply(&mut self, report: Report) -> bool {
        match report {
            Report::Query => false,
            Report::Set { id, record } => {
                let id = id.unwrap_or_default();
                self.clock += 1;
                if !self.records.contains_key(&id) && self.records.len() >= MAX_RECORDS {
                    self.evict_oldest();
                }
                let previous = self.records.insert(id, (record.clone(), self.clock));
                previous.is_none_or(|(old, _)| old != record)
            }
            Report::Clear { id: None } => self.clear(),
            Report::Clear { id: Some(id) } => {
                let before = self.records.len();
                let prefix = format!("{id}/");
                self.records
                    .retain(|key, _| *key != id && !key.starts_with(&prefix));
                self.records.len() != before
            }
        }
    }

    /// Remove every record (RIS). Returns whether there were any.
    pub fn clear(&mut self) -> bool {
        let had = !self.records.is_empty();
        self.records.clear();
        had
    }

    /// Drop `working` / `blocked` records, which end at the next shell prompt.
    /// Returns whether any were dropped.
    pub fn end_transient(&mut self) -> bool {
        let before = self.records.len();
        self.records.retain(|_, (record, _)| {
            !matches!(
                record.state,
                AgentLifecycle::Working | AgentLifecycle::Blocked
            )
        });
        self.records.len() != before
    }

    fn evict_oldest(&mut self) {
        if let Some(oldest) = self
            .records
            .iter()
            .min_by_key(|(_, (_, updated))| *updated)
            .map(|(id, _)| id.clone())
        {
            self.records.remove(&oldest);
        }
    }

    /// The record that needs the most attention, as the pane's agent status.
    pub fn top(&self) -> Option<AgentStatus> {
        let (id, (record, _)) = self
            .records
            .iter()
            .max_by_key(|(_, (record, updated))| (record.state.priority(), *updated))?;

        let mut labels = BTreeMap::new();
        if let Some(app) = self.app_of(id) {
            labels.insert("app".to_string(), app.to_string());
        }
        if let Some(kind) = record.kind {
            labels.insert("kind".to_string(), kind.to_string());
        }
        if let Some(progress) = record.progress {
            labels.insert("progress".to_string(), progress.to_string());
        }
        if let Some(title) = &record.title {
            labels.insert("title".to_string(), title.clone());
        }
        let custom = record.msg.clone().or_else(|| record.title.clone());
        Some(AgentStatus::new_clamped(record.state, custom, labels))
    }

    /// `app` of the record at `id`, else of its nearest ancestor that has one.
    fn app_of(&self, id: &str) -> Option<&str> {
        let mut path = Some(id);
        while let Some(current) = path {
            if let Some(app) = self
                .records
                .get(current)
                .and_then(|(r, _)| r.app.as_deref())
            {
                return Some(app);
            }
            path = match current.rsplit_once('/') {
                Some((parent, _)) => Some(parent),
                None if !current.is_empty() => Some(""),
                None => None,
            };
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b64(s: &str) -> String {
        base64::engine::general_purpose::STANDARD.encode(s)
    }

    fn set(body: &str) -> (Option<String>, Record) {
        match parse(body.as_bytes()) {
            Some(Report::Set { id, record }) => (id, record),
            other => panic!("expected a Set report for {body:?}, got {other:?}"),
        }
    }

    #[test]
    fn parses_a_full_report() {
        let (id, record) = set(&format!(
            "state=blocked:kind=permission:progress=40:app=terraform:id=plan/apply:msg={}",
            b64("approve the plan")
        ));
        assert_eq!(id.as_deref(), Some("plan/apply"));
        assert_eq!(record.state, AgentLifecycle::Blocked);
        assert_eq!(record.kind, Some("permission"));
        assert_eq!(record.progress, Some(40));
        assert_eq!(record.app.as_deref(), Some("terraform"));
        assert_eq!(record.msg.as_deref(), Some("approve the plan"));
    }

    #[test]
    fn query_and_clear() {
        assert_eq!(parse(b"?"), Some(Report::Query));
        assert_eq!(parse(b"state=clear"), Some(Report::Clear { id: None }));
        assert_eq!(
            parse(b"state=clear:id=a/b"),
            Some(Report::Clear {
                id: Some("a/b".into())
            })
        );
    }

    #[test]
    fn reports_the_spec_says_to_ignore() {
        // Missing or unknown state.
        assert_eq!(parse(b"app=x"), None);
        assert_eq!(parse(b"state=sleeping"), None);
        // Invalid id does not fall back to the root record.
        assert_eq!(parse(b"state=working:id=a//b"), None);
        assert_eq!(parse(b"state=working:id=a/b/c/d/e/f/g/h/i"), None);
        assert_eq!(
            parse(format!("state=working:id={}", "x".repeat(33)).as_bytes()),
            None
        );
        // Over-long key or sequence.
        assert_eq!(parse(b"state=working:abcdefghijklmnopq=1"), None);
        assert_eq!(
            parse(format!("state=working:{}", "a".repeat(4096)).as_bytes()),
            None
        );
        // msg / title: undecodable, over the limit, or carrying control characters.
        assert_eq!(parse(b"state=done:msg=a"), None);
        assert_eq!(
            parse(format!("state=done:msg={}", b64("line\nbreak")).as_bytes()),
            None
        );
        assert_eq!(
            parse(format!("state=done:title={}", b64(&"t".repeat(193))).as_bytes()),
            None
        );
        assert_eq!(
            parse(format!("state=done:msg={}", b64(&"m".repeat(2049))).as_bytes()),
            None
        );
    }

    #[test]
    fn malformed_pairs_and_invalid_optional_values_are_skipped() {
        let (_, record) =
            set("junk:state=working:=x:Key=1:app=bad app:kind=permission:progress=101");
        assert_eq!(record.state, AgentLifecycle::Working);
        // `kind` only belongs to `blocked`; `progress` 101 is out of range.
        assert_eq!(record.kind, None);
        assert_eq!(record.progress, None);
        assert_eq!(record.app, None);
        // Last value wins.
        assert_eq!(
            set("state=working:state=done").1.state,
            AgentLifecycle::Done
        );
    }

    #[test]
    fn invisible_formatting_is_removed_from_text() {
        let (_, record) = set(&format!("state=done:msg={}", b64("a\u{202E}b\u{200B}c")));
        assert_eq!(record.msg.as_deref(), Some("abc"));
    }

    fn apply(status: &mut ProgramStatus, body: &str) -> bool {
        status.apply(parse(body.as_bytes()).expect("valid report"))
    }

    #[test]
    fn top_record_is_the_most_urgent_then_the_newest() {
        let mut status = ProgramStatus::default();
        apply(&mut status, "state=working:app=claude-code");
        apply(&mut status, "state=blocked:id=sub:kind=question");
        apply(&mut status, "state=done:id=other");
        let top = status.top().expect("top");
        assert_eq!(top.lifecycle, AgentLifecycle::Blocked);
        // `app` is inherited from the root record.
        assert_eq!(
            top.labels.get("app").map(String::as_str),
            Some("claude-code")
        );
        assert_eq!(top.labels.get("kind").map(String::as_str), Some("question"));

        apply(&mut status, "state=clear:id=sub");
        assert_eq!(
            status.top().map(|s| s.lifecycle),
            Some(AgentLifecycle::Done)
        );
    }

    #[test]
    fn clear_removes_the_record_and_its_descendants_only() {
        let mut status = ProgramStatus::default();
        apply(&mut status, "state=working:id=a");
        apply(&mut status, "state=working:id=a/b");
        apply(&mut status, "state=working:id=ab");
        assert!(apply(&mut status, "state=clear:id=a"));
        assert_eq!(status.records.keys().collect::<Vec<_>>(), vec!["ab"]);
        assert!(apply(&mut status, "state=clear"));
        assert!(status.is_empty());
        assert!(!apply(&mut status, "state=clear"));
    }

    #[test]
    fn a_prompt_ends_working_and_blocked_but_keeps_done_and_error() {
        let mut status = ProgramStatus::default();
        apply(&mut status, "state=working:id=w");
        apply(&mut status, "state=blocked:id=b");
        apply(&mut status, "state=done:id=d");
        apply(&mut status, "state=error:id=e");
        assert!(status.end_transient());
        let mut kept: Vec<_> = status.records.keys().cloned().collect();
        kept.sort();
        assert_eq!(kept, vec!["d", "e"]);
        assert!(!status.end_transient());
    }

    #[test]
    fn a_repeated_identical_report_is_not_a_change() {
        let mut status = ProgramStatus::default();
        assert!(apply(&mut status, "state=working"));
        assert!(!apply(&mut status, "state=working"));
        assert!(apply(&mut status, "state=working:progress=10"));
    }

    #[test]
    fn the_least_recently_updated_record_is_evicted_at_the_cap() {
        let mut status = ProgramStatus::default();
        for i in 0..MAX_RECORDS {
            apply(&mut status, &format!("state=working:id=r{i}"));
        }
        // Touch r0 so r1 becomes the oldest.
        apply(&mut status, "state=done:id=r0");
        apply(&mut status, "state=working:id=new");
        assert_eq!(status.records.len(), MAX_RECORDS);
        assert!(status.records.contains_key("r0"));
        assert!(!status.records.contains_key("r1"));
    }
}
