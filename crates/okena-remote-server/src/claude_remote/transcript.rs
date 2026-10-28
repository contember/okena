//! Claude Code's session transcript (JSONL), reduced to chat entries.
//!
//! The format is undocumented and changes between Claude Code versions, so
//! every record the parser does not recognise is skipped, never an error.

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use okena_core::claude_remote::{
    ApiClaudeEntry, ApiClaudePrLink, ApiClaudePromptOrigin, ApiClaudeSubagent,
};
use serde::Deserialize;

use super::hook_input::is_valid_agent_id;

/// The first page reads at most this much of the transcript's tail. A cursor
/// further behind than this also gets a first page instead of a huge read.
const PAGE_WINDOW: u64 = 4 * 1024 * 1024;
const FIRST_PAGE_ENTRIES: usize = 200;
const TOOL_RESULT_MAX_CHARS: usize = 2000;
const SUMMARY_MAX_CHARS: usize = 200;
/// The newest subagents listed for a session.
const MAX_LISTED_SUBAGENTS: usize = 200;
/// A metadata file is a few hundred bytes; a bigger one is not Claude's.
const MAX_SUBAGENT_META_BYTES: u64 = 64 * 1024;
/// Pull requests kept per session; the least recently seen go first.
const MAX_PR_LINKS: usize = 50;

#[derive(Debug)]
pub(super) struct TranscriptChunk {
    pub entries: Vec<ApiClaudeEntry>,
    /// Byte offset just past the last complete line consumed.
    pub cursor: u64,
    pub reset: bool,
}

/// Read the entries after `resume_from` (the cursor of the client's previous
/// page), or a first page when `resume_from` is `None` or no longer valid.
pub(super) fn read_transcript(
    path: &Path,
    resume_from: Option<u64>,
) -> io::Result<TranscriptChunk> {
    let (mut file, len) = open_transcript(path)?;
    let resume =
        resume_from.filter(|&cursor| cursor > 0 && cursor <= len && len - cursor <= PAGE_WINDOW);

    if let Some(cursor) = resume {
        let (entries, cursor) = read_complete_lines(&mut file, cursor, len, false)?;
        return Ok(TranscriptChunk {
            entries,
            cursor,
            reset: false,
        });
    }

    let start = len.saturating_sub(PAGE_WINDOW);
    let (mut entries, cursor) = read_complete_lines(&mut file, start, len, start > 0)?;
    let excess = entries.len().saturating_sub(FIRST_PAGE_ENTRIES);
    entries.drain(..excess);
    Ok(TranscriptChunk {
        entries,
        cursor,
        reset: true,
    })
}

fn open_transcript(path: &Path) -> io::Result<(File, u64)> {
    // A FIFO or device at the reported path would block or never end.
    if !has_transcript_extension(path) || !std::fs::metadata(path)?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "not a .jsonl transcript file",
        ));
    }
    let file = File::open(path)?;
    let len = file.metadata()?.len();
    Ok((file, len))
}

/// The pull requests one transcript records, kept up to date by reading only
/// what was appended since the previous read.
#[derive(Debug, Default)]
pub(super) struct PrLinkScan {
    path: Option<PathBuf>,
    /// Byte offset just past the last complete line read.
    offset: u64,
    /// By URL.
    links: HashMap<String, ApiClaudePrLink>,
}

/// The `pr-link` records of a byte range of a transcript.
#[derive(Debug)]
pub(super) struct PrLinkRead {
    links: Vec<ApiClaudePrLink>,
    /// Byte offset just past the last complete line read.
    end: u64,
    /// The file was shorter than the requested start, so it was read from the
    /// beginning: the transcript was rewritten.
    restarted: bool,
}

impl PrLinkScan {
    /// Where the next read of `path` starts.
    pub(super) fn resume_from(&self, path: &Path) -> u64 {
        if self.path.as_deref() == Some(path) {
            self.offset
        } else {
            0
        }
    }

    /// Merge a read that started at `from`. A read that no longer continues
    /// this scan (another one landed first) is dropped. Returns whether the
    /// links changed.
    pub(super) fn apply(&mut self, path: &Path, from: u64, read: PrLinkRead) -> bool {
        if from != self.resume_from(path) {
            return false;
        }
        let mut changed = false;
        if self.path.as_deref() != Some(path) || read.restarted {
            changed = !self.links.is_empty();
            self.links.clear();
            self.path = Some(path.to_path_buf());
        }
        self.offset = read.end;
        for link in read.links {
            changed |= self.insert(link);
        }
        changed
    }

    /// Newest first; empty unless the scan follows `path`.
    pub(super) fn links(&self, path: &Path) -> Vec<ApiClaudePrLink> {
        if self.path.as_deref() != Some(path) {
            return Vec::new();
        }
        let mut links: Vec<ApiClaudePrLink> = self.links.values().cloned().collect();
        links.sort_by(|a, b| {
            b.last_seen_at
                .cmp(&a.last_seen_at)
                .then_with(|| a.url.cmp(&b.url))
        });
        links
    }

    fn insert(&mut self, link: ApiClaudePrLink) -> bool {
        if let Some(existing) = self.links.get_mut(&link.url) {
            // RFC 3339 UTC timestamps order as strings.
            if link.last_seen_at <= existing.last_seen_at {
                return false;
            }
            *existing = link;
            return true;
        }
        if self.links.len() >= MAX_PR_LINKS {
            let oldest = self
                .links
                .values()
                .min_by(|a, b| a.last_seen_at.cmp(&b.last_seen_at))
                .map(|oldest| (oldest.url.clone(), oldest.last_seen_at.clone()));
            match oldest {
                Some((_, last_seen_at)) if last_seen_at >= link.last_seen_at => return false,
                Some((url, _)) => {
                    self.links.remove(&url);
                }
                None => {}
            }
        }
        self.links.insert(link.url.clone(), link);
        true
    }
}

/// Read the `pr-link` records in the complete lines from `from` on, or from
/// the start when the file is now shorter than `from`.
pub(super) fn read_pr_links(path: &Path, from: u64) -> io::Result<PrLinkRead> {
    let (mut file, len) = open_transcript(path)?;
    let restarted = len < from;
    let start = if restarted { 0 } else { from };
    file.seek(SeekFrom::Start(start))?;
    let mut reader = BufReader::new(file.take(len - start));

    let mut links = Vec::new();
    let mut end = start;
    let mut line = Vec::new();
    loop {
        line.clear();
        let read = reader.read_until(b'\n', &mut line)?;
        // An incomplete trailing line is left for the next read.
        if read == 0 || line.last() != Some(&b'\n') {
            break;
        }
        end += u64::try_from(read).map_err(io::Error::other)?;
        // Most lines are chat records; skip them without parsing.
        let Ok(text) = std::str::from_utf8(&line) else {
            continue;
        };
        if !text.contains(r#""pr-link""#) {
            continue;
        }
        if let Ok(Record::PrLink(record)) = serde_json::from_str::<Record>(text)
            && let Some(link) = record.into_link()
        {
            links.push(link);
        }
    }
    Ok(PrLinkRead {
        links,
        end,
        restarted,
    })
}

/// `<session>.jsonl` → `<session>/subagents`, where Claude Code keeps one
/// `agent-<id>.jsonl` (and `agent-<id>.meta.json`) per subagent.
fn subagents_dir(session_transcript: &Path) -> PathBuf {
    session_transcript.with_extension("").join("subagents")
}

/// `agent_id` must pass [`is_valid_agent_id`].
pub(super) fn subagent_transcript_path(session_transcript: &Path, agent_id: &str) -> PathBuf {
    subagents_dir(session_transcript).join(format!("agent-{agent_id}.jsonl"))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SubagentMeta {
    #[serde(default)]
    agent_type: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    tool_use_id: Option<String>,
}

/// The session's subagents, oldest first, none marked running. A session
/// without subagents has no directory, which is an empty list.
pub(super) fn list_subagents(session_transcript: &Path) -> io::Result<Vec<ApiClaudeSubagent>> {
    let dir = subagents_dir(session_transcript);
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };

    let mut found: Vec<(SystemTime, String)> = Vec::new();
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        let Some(agent_id) = name
            .to_str()
            .and_then(|name| name.strip_prefix("agent-"))
            .and_then(|name| name.strip_suffix(".jsonl"))
            .filter(|agent_id| is_valid_agent_id(agent_id))
        else {
            continue;
        };
        let metadata = entry.metadata()?;
        if !metadata.is_file() {
            continue;
        }
        let started = metadata.created().or_else(|_| metadata.modified())?;
        found.push((started, agent_id.to_string()));
    }
    found.sort();
    let excess = found.len().saturating_sub(MAX_LISTED_SUBAGENTS);

    Ok(found
        .into_iter()
        .skip(excess)
        .map(|(_, agent_id)| {
            let meta = read_subagent_meta(&dir.join(format!("agent-{agent_id}.meta.json")));
            ApiClaudeSubagent {
                agent_id,
                tool_use_id: meta.as_ref().and_then(|meta| meta.tool_use_id.clone()),
                agent_type: meta.as_ref().and_then(|meta| meta.agent_type.clone()),
                description: meta.and_then(|meta| meta.description),
                running: false,
            }
        })
        .collect())
}

/// `None` for a missing, oversized or unreadable file: older Claude Code
/// versions wrote no metadata.
fn read_subagent_meta(path: &Path) -> Option<SubagentMeta> {
    let file = File::open(path).ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(MAX_SUBAGENT_META_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > MAX_SUBAGENT_META_BYTES {
        return None;
    }
    serde_json::from_slice(&bytes).ok()
}

pub(super) fn has_transcript_extension(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension == "jsonl")
}

/// Parse the complete lines in `start..end`. An incomplete trailing line is
/// left for the next read. `skip_first_line` drops the line `start` cuts into.
fn read_complete_lines(
    file: &mut File,
    start: u64,
    end: u64,
    skip_first_line: bool,
) -> io::Result<(Vec<ApiClaudeEntry>, u64)> {
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.take(end - start).read_to_end(&mut bytes)?;

    let begin = if skip_first_line {
        match bytes.iter().position(|&byte| byte == b'\n') {
            Some(newline) => newline + 1,
            None => return Ok((Vec::new(), start)),
        }
    } else {
        0
    };
    let complete_end = match bytes[begin..].iter().rposition(|&byte| byte == b'\n') {
        Some(newline) => begin + newline + 1,
        None => begin,
    };

    let entries = parse_lines(&String::from_utf8_lossy(&bytes[begin..complete_end]));
    let consumed = u64::try_from(complete_end).map_err(io::Error::other)?;
    Ok((entries, start + consumed))
}

pub(super) fn parse_lines(text: &str) -> Vec<ApiClaudeEntry> {
    text.lines()
        .filter_map(|line| serde_json::from_str::<Record>(line).ok())
        .flat_map(record_entries)
        .collect()
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Record {
    User(UserRecord),
    Assistant(AssistantRecord),
    #[serde(rename = "pr-link")]
    PrLink(PrLinkRecord),
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PrLinkRecord {
    pr_number: u64,
    pr_url: String,
    pr_repository: String,
    timestamp: String,
}

impl PrLinkRecord {
    /// `None` for a record a client must not turn into a link.
    fn into_link(self) -> Option<ApiClaudePrLink> {
        let valid = self.pr_number > 0
            && self.pr_url.starts_with("https://")
            && !self.pr_repository.is_empty()
            && !self.timestamp.is_empty();
        valid.then_some(ApiClaudePrLink {
            number: self.pr_number,
            url: self.pr_url,
            repository: self.pr_repository,
            last_seen_at: self.timestamp,
        })
    }
}

#[derive(Deserialize)]
struct UserRecord {
    uuid: String,
    #[serde(default, rename = "isMeta")]
    is_meta: Option<bool>,
    #[serde(default, rename = "isCompactSummary")]
    is_compact_summary: Option<bool>,
    #[serde(default)]
    origin: Option<RecordOrigin>,
    message: Message<UserBlock>,
}

#[derive(Deserialize)]
struct RecordOrigin {
    kind: String,
}

#[derive(Deserialize)]
struct AssistantRecord {
    uuid: String,
    message: Message<AssistantBlock>,
}

#[derive(Deserialize)]
struct Message<Block> {
    content: Content<Block>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Content<Block> {
    Text(String),
    Blocks(Vec<Block>),
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum UserBlock {
    Text {
        text: String,
    },
    ToolResult {
        tool_use_id: String,
        #[serde(default)]
        content: Option<ToolResultContent>,
        #[serde(default)]
        is_error: Option<bool>,
    },
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ToolResultContent {
    Text(String),
    Blocks(Vec<ToolResultBlock>),
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ToolResultBlock {
    Text {
        text: String,
    },
    Image,
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AssistantBlock {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        #[serde(default)]
        input: serde_json::Value,
    },
    #[serde(other)]
    Other,
}

fn record_entries(record: Record) -> Vec<ApiClaudeEntry> {
    match record {
        Record::User(user) => user_entries(user),
        Record::Assistant(assistant) => assistant_entries(assistant),
        Record::PrLink(_) | Record::Other => Vec::new(),
    }
}

fn user_entries(record: UserRecord) -> Vec<ApiClaudeEntry> {
    let UserRecord {
        uuid,
        is_meta,
        is_compact_summary,
        origin,
        message,
    } = record;

    if is_compact_summary == Some(true) {
        return Vec::new();
    }
    let origin_kind = origin.map(|origin| origin.kind);
    if origin_kind.as_deref() == Some("channel") {
        let text = strip_channel_wrapper(&content_text(&message.content)).to_string();
        return vec![ApiClaudeEntry::User {
            uuid,
            text,
            origin: ApiClaudePromptOrigin::Channel,
        }];
    }
    if is_meta == Some(true) {
        return Vec::new();
    }

    match message.content {
        // Task notifications and other injected prompts carry their own origin.
        Content::Text(_) if origin_kind.as_deref().is_some_and(|kind| kind != "human") => {
            Vec::new()
        }
        Content::Text(content) => terminal_prompt_text(&content)
            .map(|text| ApiClaudeEntry::User {
                uuid,
                text,
                origin: ApiClaudePromptOrigin::Terminal,
            })
            .into_iter()
            .collect(),
        Content::Blocks(blocks) => user_block_entries(uuid, blocks),
    }
}

fn user_block_entries(uuid: String, blocks: Vec<UserBlock>) -> Vec<ApiClaudeEntry> {
    let mut texts = Vec::new();
    let mut results = Vec::new();
    for block in blocks {
        match block {
            UserBlock::Text { text } => texts.push(text),
            UserBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => results.push(ApiClaudeEntry::ToolResult {
                uuid: uuid.clone(),
                tool_use_id,
                text: truncate_chars(&tool_result_text(content), TOOL_RESULT_MAX_CHARS),
                is_error: is_error.unwrap_or(false),
            }),
            UserBlock::Other => {}
        }
    }

    let mut entries = Vec::with_capacity(results.len() + 1);
    if !texts.is_empty() {
        entries.push(ApiClaudeEntry::User {
            uuid,
            text: texts.join("\n"),
            origin: ApiClaudePromptOrigin::Terminal,
        });
    }
    entries.extend(results);
    entries
}

fn assistant_entries(record: AssistantRecord) -> Vec<ApiClaudeEntry> {
    let AssistantRecord { uuid, message } = record;
    let blocks = match message.content {
        Content::Text(text) => vec![AssistantBlock::Text { text }],
        Content::Blocks(blocks) => blocks,
    };
    blocks
        .into_iter()
        .filter_map(|block| match block {
            AssistantBlock::Text { text } if !text.trim().is_empty() => {
                Some(ApiClaudeEntry::Assistant {
                    uuid: uuid.clone(),
                    text,
                })
            }
            AssistantBlock::ToolUse { id, name, input } => Some(ApiClaudeEntry::ToolUse {
                uuid: uuid.clone(),
                tool_use_id: id,
                summary: tool_summary(&name, &input),
                name,
            }),
            AssistantBlock::Text { .. } | AssistantBlock::Other => None,
        })
        .collect()
}

fn content_text(content: &Content<UserBlock>) -> String {
    match content {
        Content::Text(text) => text.clone(),
        Content::Blocks(blocks) => blocks
            .iter()
            .filter_map(|block| match block {
                UserBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

/// `<channel source="okena-remote" ...>\n…\n</channel>` → `…`.
fn strip_channel_wrapper(content: &str) -> &str {
    let inner = content
        .trim()
        .strip_prefix("<channel")
        .and_then(|rest| rest.find('>').map(|tag_end| &rest[tag_end + 1..]))
        .unwrap_or(content);
    inner
        .trim_end()
        .strip_suffix("</channel>")
        .unwrap_or(inner)
        .trim()
}

/// The prompt as the user typed it, or `None` for records that only carry a
/// local command's output.
fn terminal_prompt_text(content: &str) -> Option<String> {
    if content.starts_with("<local-command-") {
        return None;
    }
    if content.starts_with("<command-")
        && let Some(name) = tag_text(content, "command-name")
    {
        let name = name.trim();
        let command = if name.starts_with('/') {
            name.to_string()
        } else {
            format!("/{name}")
        };
        let args = tag_text(content, "command-args").map_or("", str::trim);
        return Some(if args.is_empty() {
            command
        } else {
            format!("{command} {args}")
        });
    }
    Some(content.to_string())
}

fn tag_text<'a>(content: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = content.find(&open)? + open.len();
    let end = start + content[start..].find(&close)?;
    Some(&content[start..end])
}

fn tool_result_text(content: Option<ToolResultContent>) -> String {
    match content {
        None => String::new(),
        Some(ToolResultContent::Text(text)) => text,
        Some(ToolResultContent::Blocks(blocks)) => blocks
            .into_iter()
            .filter_map(|block| match block {
                ToolResultBlock::Text { text } => Some(text),
                ToolResultBlock::Image => Some("[image]".to_string()),
                ToolResultBlock::Other => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

fn tool_summary(name: &str, input: &serde_json::Value) -> String {
    let field = match name {
        "Bash" => input["command"].as_str(),
        "Read" | "Edit" | "Write" | "NotebookEdit" => input["file_path"].as_str(),
        "Grep" | "Glob" => input["pattern"].as_str(),
        "WebFetch" => input["url"].as_str(),
        "WebSearch" => input["query"].as_str(),
        "Task" | "Agent" => input["description"].as_str(),
        "AskUserQuestion" => input["questions"][0]["question"].as_str(),
        // Without a plan file Claude sends no plan; the summary is then empty.
        "ExitPlanMode" => Some(input["plan"].as_str().map_or("", plan_title)),
        _ => None,
    };
    let summary = match field {
        Some(value) => value.to_string(),
        None => input.to_string(),
    };
    let single_line = summary.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate_chars(&single_line, SUMMARY_MAX_CHARS)
}

/// The first non-empty line of a Markdown plan, without its heading marks.
fn plan_title(plan: &str) -> &str {
    plan.lines()
        .map(|line| line.trim().trim_start_matches('#').trim())
        .find(|line| !line.is_empty())
        .unwrap_or("")
}

/// At most `max_chars` characters; a cut text ends with `…`.
fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().nth(max_chars).is_none() {
        return text.to_string();
    }
    let kept: String = text.chars().take(max_chars.saturating_sub(1)).collect();
    format!("{kept}…")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Write;

    fn line(value: serde_json::Value) -> String {
        format!("{value}\n")
    }

    fn user_text(uuid: &str, content: &str) -> String {
        line(json!({
            "type": "user",
            "uuid": uuid,
            "sessionId": "s1",
            "origin": {"kind": "human"},
            "message": {"role": "user", "content": content}
        }))
    }

    fn assistant_text(uuid: &str, text: &str) -> String {
        line(json!({
            "type": "assistant",
            "uuid": uuid,
            "message": {"role": "assistant", "content": [{"type": "text", "text": text}]}
        }))
    }

    fn user(uuid: &str, text: &str, origin: ApiClaudePromptOrigin) -> ApiClaudeEntry {
        ApiClaudeEntry::User {
            uuid: uuid.into(),
            text: text.into(),
            origin,
        }
    }

    fn assistant(uuid: &str, text: &str) -> ApiClaudeEntry {
        ApiClaudeEntry::Assistant {
            uuid: uuid.into(),
            text: text.into(),
        }
    }

    #[test]
    fn terminal_prompt() {
        assert_eq!(
            parse_lines(&user_text("u1", "fix the build")),
            vec![user("u1", "fix the build", ApiClaudePromptOrigin::Terminal)]
        );
    }

    #[test]
    fn channel_prompt_is_unwrapped() {
        let text = line(json!({
            "type": "user",
            "uuid": "u1",
            "isMeta": true,
            "origin": {"kind": "channel", "server": "okena-remote"},
            "message": {"role": "user", "content": "<channel source=\"okena-remote\" sender=\"phone\">\nrun the tests\n</channel>"}
        }));
        assert_eq!(
            parse_lines(&text),
            vec![user("u1", "run the tests", ApiClaudePromptOrigin::Channel)]
        );
    }

    #[test]
    fn other_meta_records_are_skipped() {
        let text = line(json!({
            "type": "user",
            "uuid": "u1",
            "isMeta": true,
            "origin": {"kind": "auto-continuation"},
            "message": {"role": "user", "content": "Continue."}
        })) + &line(json!({
            "type": "user",
            "uuid": "u2",
            "isMeta": true,
            "message": {"role": "user", "content": [{"type": "text", "text": "skill body"}]}
        }));
        assert_eq!(parse_lines(&text), Vec::new());
    }

    #[test]
    fn only_human_or_unmarked_string_prompts_are_terminal_prompts() {
        let unmarked = line(json!({
            "type": "user",
            "uuid": "u1",
            "message": {"role": "user", "content": "typed by hand"}
        }));
        let task_notification = line(json!({
            "type": "user",
            "uuid": "u2",
            "origin": {"kind": "task-notification", "producer": "session-task"},
            "message": {"role": "user", "content": "<task-notification>\n<task-id>a1</task-id>\n</task-notification>"}
        }));
        let compact_summary = line(json!({
            "type": "user",
            "uuid": "u3",
            "isCompactSummary": true,
            "message": {"role": "user", "content": "This session is being continued from a previous conversation."}
        }));
        let tool_result = line(json!({
            "type": "user",
            "uuid": "u4",
            "message": {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "toolu_1", "content": "ok"}
            ]}
        }));

        assert_eq!(
            parse_lines(&(unmarked + &task_notification + &compact_summary + &tool_result)),
            vec![
                user("u1", "typed by hand", ApiClaudePromptOrigin::Terminal),
                ApiClaudeEntry::ToolResult {
                    uuid: "u4".into(),
                    tool_use_id: "toolu_1".into(),
                    text: "ok".into(),
                    is_error: false,
                },
            ]
        );
    }

    #[test]
    fn slash_command_becomes_its_command_line() {
        let with_args = user_text(
            "u1",
            "<command-message>review</command-message>\n<command-name>/review</command-name>\n<command-args>src/main.rs</command-args>",
        );
        let without_args = user_text(
            "u2",
            "<command-name>/copy</command-name>\n            <command-message>copy</command-message>\n            <command-args></command-args>",
        );
        assert_eq!(
            parse_lines(&(with_args + &without_args)),
            vec![
                user("u1", "/review src/main.rs", ApiClaudePromptOrigin::Terminal),
                user("u2", "/copy", ApiClaudePromptOrigin::Terminal),
            ]
        );
    }

    #[test]
    fn local_command_output_is_skipped() {
        let text = user_text(
            "u1",
            "<local-command-stdout>Copied to clipboard</local-command-stdout>",
        ) + &user_text(
            "u2",
            "<local-command-caveat>Caveat: The messages below were generated by the user while running local commands.</local-command-caveat>",
        );
        assert_eq!(parse_lines(&text), Vec::new());
    }

    #[test]
    fn tool_results_as_string_blocks_and_images() {
        let text = line(json!({
            "type": "user",
            "uuid": "u1",
            "message": {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "toolu_1", "content": "ok"},
                {"type": "tool_result", "tool_use_id": "toolu_2", "is_error": true, "content": [
                    {"type": "text", "text": "line one"},
                    {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "AAAA"}},
                    {"type": "text", "text": "line two"}
                ]},
                {"type": "tool_result", "tool_use_id": "toolu_3"}
            ]}
        }));
        assert_eq!(
            parse_lines(&text),
            vec![
                ApiClaudeEntry::ToolResult {
                    uuid: "u1".into(),
                    tool_use_id: "toolu_1".into(),
                    text: "ok".into(),
                    is_error: false,
                },
                ApiClaudeEntry::ToolResult {
                    uuid: "u1".into(),
                    tool_use_id: "toolu_2".into(),
                    text: "line one\n[image]\nline two".into(),
                    is_error: true,
                },
                ApiClaudeEntry::ToolResult {
                    uuid: "u1".into(),
                    tool_use_id: "toolu_3".into(),
                    text: String::new(),
                    is_error: false,
                },
            ]
        );
    }

    #[test]
    fn user_text_blocks_are_joined_into_one_entry() {
        let text = line(json!({
            "type": "user",
            "uuid": "u1",
            "message": {"role": "user", "content": [
                {"type": "text", "text": "first"},
                {"type": "text", "text": "second"}
            ]}
        }));
        assert_eq!(
            parse_lines(&text),
            vec![user("u1", "first\nsecond", ApiClaudePromptOrigin::Terminal)]
        );
    }

    #[test]
    fn long_tool_result_is_truncated_on_a_char_boundary() {
        let long = "ž".repeat(TOOL_RESULT_MAX_CHARS + 50);
        let text = line(json!({
            "type": "user",
            "uuid": "u1",
            "message": {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "toolu_1", "content": long}
            ]}
        }));
        let entries = parse_lines(&text);
        let [ApiClaudeEntry::ToolResult { text, .. }] = entries.as_slice() else {
            panic!("expected one tool result");
        };
        assert_eq!(text.chars().count(), TOOL_RESULT_MAX_CHARS);
        assert!(text.ends_with('…'));
        assert!(text.starts_with("žž"));
    }

    #[test]
    fn assistant_text_and_tool_use_keep_the_record_uuid() {
        let text = line(json!({
            "type": "assistant",
            "uuid": "a1",
            "message": {"role": "assistant", "content": [
                {"type": "thinking", "thinking": "", "signature": "sig"},
                {"type": "text", "text": "Running the tests."},
                {"type": "tool_use", "id": "toolu_1", "name": "Bash", "input": {"command": "cargo test\n  --workspace", "description": "Run tests"}}
            ]}
        }));
        assert_eq!(
            parse_lines(&text),
            vec![
                assistant("a1", "Running the tests."),
                ApiClaudeEntry::ToolUse {
                    uuid: "a1".into(),
                    tool_use_id: "toolu_1".into(),
                    name: "Bash".into(),
                    summary: "cargo test --workspace".into(),
                },
            ]
        );
    }

    #[test]
    fn tool_summaries() {
        let cases = [
            ("Read", json!({"file_path": "/src/main.rs"}), "/src/main.rs"),
            (
                "Edit",
                json!({"file_path": "/a.rs", "old_string": "x"}),
                "/a.rs",
            ),
            ("Grep", json!({"pattern": "fn main"}), "fn main"),
            ("Glob", json!({"pattern": "**/*.rs"}), "**/*.rs"),
            (
                "WebFetch",
                json!({"url": "https://example.com"}),
                "https://example.com",
            ),
            ("WebSearch", json!({"query": "rust serde"}), "rust serde"),
            (
                "Agent",
                json!({"description": "Find callers", "prompt": "…"}),
                "Find callers",
            ),
            (
                "AskUserQuestion",
                json!({"questions": [{"question": "Which color?"}, {"question": "Size?"}]}),
                "Which color?",
            ),
            (
                "ExitPlanMode",
                json!({"plan": "\n# Append \"phone\"\n\n## Context\nbody"}),
                "Append \"phone\"",
            ),
            ("ExitPlanMode", json!({}), ""),
            ("mcp__x__y", json!({"a": 1}), "{\"a\":1}"),
        ];
        for (name, input, expected) in cases {
            assert_eq!(tool_summary(name, &input), expected, "{name}");
        }

        let long = tool_summary("Bash", &json!({"command": "x".repeat(500)}));
        assert_eq!(long.chars().count(), SUMMARY_MAX_CHARS);
    }

    #[test]
    fn malformed_and_unknown_lines_are_skipped() {
        let text = "{not json\n".to_string()
            + &line(json!({"type": "file-history-snapshot", "messageId": "m"}))
            + &line(json!({"type": "user", "message": {"content": "no uuid"}}))
            + "\n"
            + &assistant_text("a1", "still parsed");
        assert_eq!(parse_lines(&text), vec![assistant("a1", "still parsed")]);
    }

    fn write_file(contents: &str) -> tempfile::NamedTempFile {
        let mut file = tempfile::Builder::new()
            .suffix(".jsonl")
            .tempfile()
            .expect("temp file");
        file.write_all(contents.as_bytes()).expect("write");
        file
    }

    #[test]
    fn incomplete_trailing_line_is_not_consumed() {
        let complete = assistant_text("a1", "one");
        let second = assistant_text("a2", "two");
        let (partial, rest) = second.split_at(20);
        let mut file = write_file(&(complete.clone() + partial));

        let first = read_transcript(file.path(), None).expect("read");
        assert!(first.reset);
        assert_eq!(first.entries, vec![assistant("a1", "one")]);
        assert_eq!(first.cursor, u64::try_from(complete.len()).expect("fits"));

        file.write_all(rest.as_bytes()).expect("append");

        let next = read_transcript(file.path(), Some(first.cursor)).expect("read");
        assert!(!next.reset);
        assert_eq!(next.entries, vec![assistant("a2", "two")]);

        let idle = read_transcript(file.path(), Some(next.cursor)).expect("read");
        assert!(!idle.reset);
        assert_eq!(idle.entries, Vec::new());
        assert_eq!(idle.cursor, next.cursor);
    }

    #[test]
    fn shrunk_file_resets_to_a_first_page() {
        let file = write_file(&(assistant_text("a1", "one") + &assistant_text("a2", "two")));
        let first = read_transcript(file.path(), None).expect("read");

        let rewritten = write_file(&assistant_text("b1", "new"));
        std::fs::copy(rewritten.path(), file.path()).expect("rewrite");

        let page = read_transcript(file.path(), Some(first.cursor)).expect("read");
        assert!(page.reset);
        assert_eq!(page.entries, vec![assistant("b1", "new")]);
    }

    #[test]
    fn first_page_keeps_the_last_entries() {
        let text: String = (0..FIRST_PAGE_ENTRIES + 5)
            .map(|n| assistant_text(&format!("a{n}"), "x"))
            .collect();
        let file = write_file(&text);

        let page = read_transcript(file.path(), Some(0)).expect("read");
        assert!(page.reset);
        assert_eq!(page.entries.len(), FIRST_PAGE_ENTRIES);
        assert_eq!(page.entries[0], assistant("a5", "x"));
    }

    #[test]
    fn first_page_skips_the_line_its_window_cuts_into() {
        let huge = assistant_text("a1", &"x".repeat(5 * 1024 * 1024));
        let file = write_file(&(huge + &assistant_text("a2", "tail")));

        let page = read_transcript(file.path(), None).expect("read");
        assert!(page.reset);
        assert_eq!(page.entries, vec![assistant("a2", "tail")]);
        assert_eq!(
            page.cursor,
            file.as_file().metadata().expect("metadata").len()
        );
    }

    fn pr_link_line(number: u64, timestamp: &str) -> String {
        line(json!({
            "type": "pr-link",
            "sessionId": "s1",
            "prNumber": number,
            "prUrl": format!("https://github.com/acme/widgets/pull/{number}"),
            "prRepository": "acme/widgets",
            "timestamp": timestamp,
        }))
    }

    fn pr_link(number: u64, timestamp: &str) -> ApiClaudePrLink {
        ApiClaudePrLink {
            number,
            url: format!("https://github.com/acme/widgets/pull/{number}"),
            repository: "acme/widgets".into(),
            last_seen_at: timestamp.into(),
        }
    }

    fn scan(scan: &mut PrLinkScan, path: &Path) -> bool {
        let from = scan.resume_from(path);
        let read = read_pr_links(path, from).expect("read");
        scan.apply(path, from, read)
    }

    #[test]
    fn pr_links_are_deduplicated_by_url_keeping_the_latest_timestamp() {
        let mut file = tempfile::Builder::new()
            .suffix(".jsonl")
            .tempfile()
            .expect("temp file");
        let malformed = line(json!({"type": "pr-link", "prNumber": "7"}));
        let not_https = line(json!({
            "type": "pr-link",
            "prNumber": 8,
            "prUrl": "javascript:alert(1)",
            "prRepository": "acme/widgets",
            "timestamp": "2026-01-01T00:00:00.000Z",
        }));
        let text = pr_link_line(1, "2026-01-01T10:00:00.000Z")
            + &assistant_text("a1", "opened \"pr-link\" for you")
            + &pr_link_line(2, "2026-01-01T11:00:00.000Z")
            + &pr_link_line(1, "2026-01-01T12:00:00.000Z")
            + &pr_link_line(1, "2026-01-01T09:00:00.000Z")
            + &malformed
            + &not_https
            + "not json\n";
        file.write_all(text.as_bytes()).expect("write");

        let mut links = PrLinkScan::default();
        assert!(scan(&mut links, file.path()));
        assert_eq!(
            links.links(file.path()),
            vec![
                pr_link(1, "2026-01-01T12:00:00.000Z"),
                pr_link(2, "2026-01-01T11:00:00.000Z"),
            ]
        );
        // The chat view does not show the records.
        assert_eq!(
            parse_lines(&pr_link_line(3, "2026-01-01T00:00:00Z")),
            Vec::new()
        );
    }

    #[test]
    fn pr_links_are_read_incrementally() {
        let mut file = tempfile::Builder::new()
            .suffix(".jsonl")
            .tempfile()
            .expect("temp file");
        let first = pr_link_line(1, "2026-01-01T10:00:00.000Z");
        file.write_all(first.as_bytes()).expect("write");
        let partial = pr_link_line(2, "2026-01-01T11:00:00.000Z");
        let (head, tail) = partial.split_at(20);
        file.write_all(head.as_bytes()).expect("write");

        let mut links = PrLinkScan::default();
        assert!(scan(&mut links, file.path()));
        assert_eq!(links.resume_from(file.path()), first.len() as u64);
        assert_eq!(links.links(file.path()).len(), 1);

        file.write_all(tail.as_bytes()).expect("write");
        assert!(scan(&mut links, file.path()));
        assert_eq!(links.links(file.path()).len(), 2);
        assert!(!scan(&mut links, file.path()), "nothing new was appended");
    }

    #[test]
    fn rewritten_transcript_or_another_path_starts_over() {
        let directory = tempfile::tempdir().expect("temp dir");
        let path = directory.path().join("s1.jsonl");
        std::fs::write(
            &path,
            pr_link_line(1, "2026-01-01T10:00:00.000Z")
                + &pr_link_line(2, "2026-01-01T11:00:00.000Z"),
        )
        .expect("write");
        let mut links = PrLinkScan::default();
        assert!(scan(&mut links, &path));

        std::fs::write(&path, pr_link_line(3, "2026-01-01T12:00:00.000Z")).expect("rewrite");
        assert!(scan(&mut links, &path));
        assert_eq!(
            links.links(&path),
            vec![pr_link(3, "2026-01-01T12:00:00.000Z")]
        );

        let other = directory.path().join("s2.jsonl");
        std::fs::write(&other, "").expect("write");
        assert_eq!(links.links(&other), Vec::new());
        assert!(scan(&mut links, &other), "the old session's links are gone");
        assert_eq!(links.links(&path), Vec::new());
    }

    #[test]
    fn a_read_that_lost_the_race_is_dropped() {
        let mut file = tempfile::Builder::new()
            .suffix(".jsonl")
            .tempfile()
            .expect("temp file");
        file.write_all(pr_link_line(1, "2026-01-01T10:00:00.000Z").as_bytes())
            .expect("write");
        let mut links = PrLinkScan::default();
        let stale = read_pr_links(file.path(), 0).expect("read");
        assert!(scan(&mut links, file.path()));
        assert!(!links.apply(file.path(), 0, stale));
        assert_eq!(links.links(file.path()).len(), 1);
    }

    #[test]
    fn pr_links_are_capped_dropping_the_least_recently_seen() {
        let mut links = PrLinkScan::default();
        for number in 1..=(MAX_PR_LINKS as u64 + 1) {
            links.insert(pr_link(number, &format!("2026-01-01T10:00:{number:02}Z")));
        }
        let path = Path::new("/t/s1.jsonl");
        links.path = Some(path.to_path_buf());
        let kept = links.links(path);
        assert_eq!(kept.len(), MAX_PR_LINKS);
        assert_eq!(
            kept.first().map(|link| link.number),
            Some(MAX_PR_LINKS as u64 + 1)
        );
        assert!(kept.iter().all(|link| link.number != 1));
    }
}
