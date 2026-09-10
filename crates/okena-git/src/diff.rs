//! Git diff parsing and execution.
//!
//! Provides structures and functions for parsing unified diff output
//! and executing git diff commands.
//!
//! One path base for the diff and per-file mutation surface: `FileDiff` paths
//! and the `file_path` arguments here and in `repository::branch` are relative
//! to the git worktree root, while `repo_path` may be any directory inside the
//! repository. `diff.relative` would make git print a different base, so the
//! invocations below pin it with `--no-relative`. Blame and file history take
//! their own bases — see their modules.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use okena_core::process::{command, safe_output};
use serde::{Deserialize, Serialize};

/// Type of a diff line.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiffLineType {
    /// Context line (unchanged).
    Context,
    /// Added line.
    Added,
    /// Removed line.
    Removed,
    /// Hunk header line (@@).
    Header,
}

/// A single line in a diff.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiffLine {
    /// Type of this line.
    pub line_type: DiffLineType,
    /// Content of the line (without +/- prefix).
    pub content: String,
    /// Line number in the old file (None for added lines).
    pub old_line_num: Option<usize>,
    /// Line number in the new file (None for removed lines).
    pub new_line_num: Option<usize>,
}

/// A hunk in a diff (section of changes).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiffHunk {
    /// The hunk header (e.g., "@@ -10,5 +10,7 @@ fn example()").
    pub header: String,
    /// Starting line number in old file.
    pub old_start: usize,
    /// Starting line number in new file.
    pub new_start: usize,
    /// Lines in this hunk.
    pub lines: Vec<DiffLine>,
}

/// Diff for a single file.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileDiff {
    /// Old file path (None for new files).
    pub old_path: Option<String>,
    /// New file path (None for deleted files).
    pub new_path: Option<String>,
    /// Hunks in this file.
    pub hunks: Vec<DiffHunk>,
    /// Whether this is a binary file.
    pub is_binary: bool,
    /// Number of lines added.
    pub lines_added: usize,
    /// Number of lines removed.
    pub lines_removed: usize,
}

impl FileDiff {
    /// Get the display name for this file.
    pub fn display_name(&self) -> &str {
        self.new_path
            .as_deref()
            .or(self.old_path.as_deref())
            .unwrap_or("unknown")
    }
}

pub use okena_core::types::DiffMode;

/// Result of a diff operation.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DiffResult {
    /// Files with changes.
    pub files: Vec<FileDiff>,
}

impl DiffResult {
    /// Check if the diff is empty.
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Get total lines added across all files.
    #[allow(dead_code)]
    pub fn total_added(&self) -> usize {
        self.files.iter().map(|f| f.lines_added).sum()
    }

    /// Get total lines removed across all files.
    #[allow(dead_code)]
    pub fn total_removed(&self) -> usize {
        self.files.iter().map(|f| f.lines_removed).sum()
    }
}

/// A hunk that is still absorbing lines, plus how many it still owes each side.
///
/// Git's `@@` counts are exact, and they are the only thing that tells hunk
/// *content* apart from the next file's metadata: an added line may itself read
/// `+++ b/other.rs`, and taking that for a header retargets the whole file.
struct OpenHunk {
    hunk: DiffHunk,
    old_remaining: usize,
    new_remaining: usize,
}

/// What an open hunk did with a line.
enum Consumed {
    Added,
    Removed,
    /// Context, or the `\ No newline at end of file` marker (which owes nothing).
    Other,
}

impl OpenHunk {
    fn is_complete(&self) -> bool {
        self.old_remaining == 0 && self.new_remaining == 0
    }

    /// Append `line` as hunk content, or return `None` when it is not content.
    fn consume(
        &mut self,
        line: &str,
        old_line: &mut usize,
        new_line: &mut usize,
    ) -> Option<Consumed> {
        if let Some(content) = line.strip_prefix('+') {
            self.hunk.lines.push(DiffLine {
                line_type: DiffLineType::Added,
                content: content.to_string(),
                old_line_num: None,
                new_line_num: Some(*new_line),
            });
            *new_line += 1;
            self.new_remaining = self.new_remaining.saturating_sub(1);
            Some(Consumed::Added)
        } else if let Some(content) = line.strip_prefix('-') {
            self.hunk.lines.push(DiffLine {
                line_type: DiffLineType::Removed,
                content: content.to_string(),
                old_line_num: Some(*old_line),
                new_line_num: None,
            });
            *old_line += 1;
            self.old_remaining = self.old_remaining.saturating_sub(1);
            Some(Consumed::Removed)
        } else if line.starts_with('\\') {
            Some(Consumed::Other)
        } else if line.is_empty() || line.starts_with(' ') {
            self.hunk.lines.push(DiffLine {
                line_type: DiffLineType::Context,
                content: line.strip_prefix(' ').unwrap_or("").to_string(),
                old_line_num: Some(*old_line),
                new_line_num: Some(*new_line),
            });
            *old_line += 1;
            *new_line += 1;
            self.old_remaining = self.old_remaining.saturating_sub(1);
            self.new_remaining = self.new_remaining.saturating_sub(1);
            Some(Consumed::Other)
        } else {
            None
        }
    }
}

/// Move a finished hunk into its file.
fn close_hunk(open: &mut Option<OpenHunk>, file: &mut Option<FileDiff>) {
    if let Some(open) = open.take()
        && let Some(file) = file.as_mut()
    {
        file.hunks.push(open.hunk);
    }
}

/// Parse a unified diff output into structured form.
pub fn parse_unified_diff(output: &str) -> DiffResult {
    let mut files = Vec::new();
    let mut current_file: Option<FileDiff> = None;
    let mut current_hunk: Option<OpenHunk> = None;
    let mut old_line = 0usize;
    let mut new_line = 0usize;

    for line in output.lines() {
        // While a hunk still owes lines, every line is its content — header
        // shapes included. Only outside one do the metadata rules below apply.
        if let Some(open) = current_hunk.as_mut() {
            match open.consume(line, &mut old_line, &mut new_line) {
                Some(kind) => {
                    if let Some(file) = current_file.as_mut() {
                        match kind {
                            Consumed::Added => file.lines_added += 1,
                            Consumed::Removed => file.lines_removed += 1,
                            Consumed::Other => {}
                        }
                    }
                    if !open.is_complete() {
                        continue;
                    }
                    close_hunk(&mut current_hunk, &mut current_file);
                    continue;
                }
                // Malformed: the hunk ran out of recognizable content early.
                None => close_hunk(&mut current_hunk, &mut current_file),
            }
        }

        // Check for diff header (new file)
        if line.starts_with("diff --git ") {
            // Save previous file
            if let Some(file) = current_file.take() {
                files.push(file);
            }

            // Start new file. Use the "diff --git a/<old> b/<new>" header as a
            // fallback source of paths: pure renames/copies and mode-only
            // changes emit no `---`/`+++` lines, so without this fallback the
            // FileDiff would have both paths None and display_name() would
            // return "unknown".
            let (old_path, new_path) = parse_diff_git_header(line);
            current_file = Some(FileDiff {
                old_path,
                new_path,
                hunks: Vec::new(),
                is_binary: false,
                lines_added: 0,
                lines_removed: 0,
            });
            continue;
        }

        // Skip if no current file
        let file = match current_file.as_mut() {
            Some(f) => f,
            None => continue,
        };

        // Parse rename/copy headers. A pure rename (100% similarity) emits
        // `rename from <old>` / `rename to <new>` with no `---`/`+++` lines;
        // copies emit `copy from`/`copy to` analogously. These override the
        // `diff --git` fallback with the authoritative (unprefixed) paths.
        if let Some(old) = line
            .strip_prefix("rename from ")
            .or_else(|| line.strip_prefix("copy from "))
        {
            file.old_path = decode_git_path(old);
            continue;
        }
        if let Some(new) = line
            .strip_prefix("rename to ")
            .or_else(|| line.strip_prefix("copy to "))
        {
            file.new_path = decode_git_path(new);
            continue;
        }

        if line.starts_with("new file mode ") {
            file.old_path = None;
            continue;
        }
        if line.starts_with("deleted file mode ") {
            file.new_path = None;
            continue;
        }

        // Parse old file path. These lines are authoritative and override the
        // `diff --git` header fallback (e.g. /dev/null clears the path for an
        // added file even though the header carried a fake `a/<new>`).
        if let Some(raw) = line.strip_prefix("--- ") {
            file.old_path = parse_diff_path_line(raw, "a/");
            continue;
        }

        // Parse new file path
        if let Some(raw) = line.strip_prefix("+++ ") {
            file.new_path = parse_diff_path_line(raw, "b/");
            continue;
        }

        // Check for binary file
        // Git outputs "Binary files a/path and b/path differ" for binary files
        if line.starts_with("Binary files ") && line.ends_with(" differ") {
            file.is_binary = true;
            continue;
        }

        // Parse hunk header
        if line.starts_with("@@ ") {
            // Parse hunk header: @@ -old_start,old_count +new_start,new_count @@ context
            let (old_start, old_count, new_start, new_count) = parse_hunk_header(line);
            old_line = old_start;
            new_line = new_start;

            current_hunk = Some(OpenHunk {
                hunk: DiffHunk {
                    header: line.to_string(),
                    old_start,
                    new_start,
                    lines: vec![DiffLine {
                        line_type: DiffLineType::Header,
                        content: line.to_string(),
                        old_line_num: None,
                        new_line_num: None,
                    }],
                },
                old_remaining: old_count,
                new_remaining: new_count,
            });
            // An empty hunk owes nothing, so nothing would ever close it.
            if current_hunk.as_ref().is_some_and(OpenHunk::is_complete) {
                close_hunk(&mut current_hunk, &mut current_file);
            }
            continue;
        }
    }

    // Save last file and hunk
    close_hunk(&mut current_hunk, &mut current_file);
    if let Some(file) = current_file {
        files.push(file);
    }

    DiffResult { files }
}

/// Parse the `diff --git a/<old> b/<new>` header into (old_path, new_path).
///
/// This is a best-effort fallback used when a file section carries no
/// `---`/`+++` lines (pure renames/copies, mode-only changes). The
/// authoritative paths come from `rename from`/`rename to` or `---`/`+++`
/// lines when present, which override this.
///
/// Caveat: git quotes each side independently, so both forms can share one
/// header. Two unquoted paths stay ambiguous when the old one contains " b/";
/// returns `(None, None)` if the header can't be split unambiguously.
fn parse_diff_git_header(line: &str) -> (Option<String>, Option<String>) {
    let Some(rest) = line.strip_prefix("diff --git ") else {
        return (None, None);
    };
    let Some((old, new)) = split_diff_git_paths(rest) else {
        return (None, None);
    };

    match (
        decode_git_path(old).and_then(|path| strip_path_prefix(&path, "a/")),
        decode_git_path(new).and_then(|path| strip_path_prefix(&path, "b/")),
    ) {
        (Some(old), Some(new)) => (Some(old), Some(new)),
        _ => (None, None),
    }
}

/// The path a `---`/`+++` line names, or `None` for `/dev/null`.
///
/// Git terminates the name with a tab whenever it contains a space, so the
/// token ends at the closing quote or at the first tab — never at end of line.
fn parse_diff_path_line(raw: &str, prefix: &str) -> Option<String> {
    let token = if raw.starts_with('"') {
        raw.get(..quoted_token_end(raw)?)?
    } else {
        raw.split('\t').next()?
    };

    let path = decode_git_path(token)?;
    if path == "/dev/null" {
        return None;
    }
    Some(path.strip_prefix(prefix).unwrap_or(&path).to_string())
}

/// Split a `diff --git` header's two path tokens, each possibly C-quoted.
fn split_diff_git_paths(rest: &str) -> Option<(&str, &str)> {
    if rest.starts_with('"') {
        let (old, tail) = rest.split_at(quoted_token_end(rest)?);
        return Some((old, tail.strip_prefix(' ')?));
    }

    // An unquoted old path cannot contain `"`, so a trailing one means the new
    // side is quoted; otherwise the last " b/" is the least-bad guess.
    let at = if rest.ends_with('"') {
        rest.rfind(" \"")?
    } else {
        rest.rfind(" b/")?
    };
    Some((&rest[..at], &rest[at + 1..]))
}

/// Byte index just past the closing `"` of the quoted token starting at 0.
fn quoted_token_end(token: &str) -> Option<usize> {
    let bytes = token.as_bytes();
    let mut index = 1;
    while let Some(&byte) = bytes.get(index) {
        match byte {
            b'\\' => index += 2,
            b'"' => return Some(index + 1),
            _ => index += 1,
        }
    }
    None
}

/// Strip a `a/`-style diff prefix, rejecting a path that is nothing but it.
fn strip_path_prefix(path: &str, prefix: &str) -> Option<String> {
    let stripped = path.strip_prefix(prefix)?;
    (!stripped.is_empty()).then(|| stripped.to_string())
}

/// Decode git's C-quoting (`core.quotePath`): `\NNN` escapes are *bytes*, so
/// they decode into a byte buffer before the UTF-8 check. A malformed escape
/// or a non-UTF-8 result yields `None` — a lossy name is a wrong identity.
fn decode_git_path(raw: &str) -> Option<String> {
    let Some(inner) = raw
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
    else {
        return Some(raw.to_string());
    };

    let mut decoded = Vec::with_capacity(inner.len());
    let mut rest = inner.bytes();
    while let Some(byte) = rest.next() {
        if byte != b'\\' {
            decoded.push(byte);
            continue;
        }
        decoded.push(match rest.next()? {
            b'a' => 0x07,
            b'b' => 0x08,
            b'f' => 0x0c,
            b'n' => b'\n',
            b'r' => b'\r',
            b't' => b'\t',
            b'v' => 0x0b,
            verbatim @ (b'"' | b'\\') => verbatim,
            first @ b'0'..=b'7' => {
                // Git always writes three octal digits, and never above \377.
                let mut value = u32::from(first - b'0');
                for _ in 0..2 {
                    let digit = rest.next().filter(|d| (b'0'..=b'7').contains(d))?;
                    value = value * 8 + u32::from(digit - b'0');
                }
                u8::try_from(value).ok()?
            }
            _ => return None,
        });
    }

    String::from_utf8(decoded).ok()
}

/// Parse a hunk header into `(old_start, old_count, new_start, new_count)`.
fn parse_hunk_header(header: &str) -> (usize, usize, usize, usize) {
    // Format: @@ -old_start,old_count +new_start,new_count @@ context
    // or: @@ -old_start +new_start @@ context (count of 1 is implicit)
    let mut old = (1usize, 1usize);
    let mut new = (1usize, 1usize);

    // Find the range part between @@ markers
    if let Some(range_part) = header
        .strip_prefix("@@ ")
        .and_then(|s| s.split(" @@").next())
    {
        for part in range_part.split_whitespace() {
            if let Some(spec) = part.strip_prefix('-') {
                old = parse_hunk_range(spec);
            } else if let Some(spec) = part.strip_prefix('+') {
                new = parse_hunk_range(spec);
            }
        }
    }

    (old.0, old.1, new.0, new.1)
}

/// Parse a `start,count` (or bare `start`, count 1 implied) hunk range.
fn parse_hunk_range(spec: &str) -> (usize, usize) {
    let mut parts = spec.split(',');
    let start = parts.next().and_then(|s| s.parse().ok()).unwrap_or(1);
    let count = parts.next().map_or(1, |s| s.parse().unwrap_or(1));
    (start, count)
}

/// Whether `base` and `head` have a common ancestor, i.e. whether a three-dot
/// range between them resolves at all. False in a shallow clone whose grafted
/// history no longer reaches the fork point, and for unrelated histories.
///
/// Callers must have validated both refs — this feeds them to git verbatim.
fn has_merge_base(repo_path: &str, base: &str, head: &str) -> bool {
    merge_base(repo_path, base, head).is_some()
}

/// The merge base of two refs, or `None` when they share no history.
///
/// Shells out, so a caller working over many files should resolve it once
/// rather than through a per-file content lookup.
pub fn merge_base(repo_path: &str, base: &str, head: &str) -> Option<String> {
    crate::validate_git_ref(base).ok()?;
    crate::validate_git_ref(head).ok()?;
    let output =
        safe_output(command("git").args(["-C", repo_path, "merge-base", base, head])).ok()?;
    if !output.status.success() {
        return None;
    }
    let revision = String::from_utf8(output.stdout).ok()?;
    let revision = revision.trim();
    (!revision.is_empty()).then(|| revision.to_string())
}

fn commit_diff_range(repo_path: &Path, revision: &str) -> crate::GitResult<String> {
    use crate::error::GitError;

    let repo = crate::gix_helpers::open(repo_path)
        .ok_or_else(|| GitError::ParseError("not a git repository".to_string()))?;
    let commit_id = repo
        .rev_parse_single(revision)
        .map_err(|error| GitError::ParseError(error.to_string()))?
        .detach();
    let commit = repo
        .find_commit(commit_id)
        .map_err(|error| GitError::ParseError(error.to_string()))?;
    let base_id = commit
        .parent_ids()
        .next()
        .map(|parent| parent.detach())
        .unwrap_or_else(|| gix::ObjectId::empty_tree(commit_id.kind()));
    Ok(format!("{base_id}..{commit_id}"))
}

/// Get diff for a repository path.
#[allow(dead_code)]
pub fn get_diff(path: &Path, mode: DiffMode) -> crate::GitResult<DiffResult> {
    get_diff_with_options(path, mode, false)
}

/// Get diff for a repository path with options.
pub fn get_diff_with_options(
    path: &Path,
    mode: DiffMode,
    ignore_whitespace: bool,
) -> crate::GitResult<DiffResult> {
    use crate::error::GitError;

    let t_total = std::time::Instant::now();
    let path_str = path
        .to_str()
        .ok_or_else(|| GitError::InvalidPath(path.to_path_buf()))?;

    // Build git diff command based on mode
    // WorkingTree: unstaged changes (working tree vs index)
    // Staged: staged changes (index vs HEAD)
    // --no-color: prevent ANSI codes when user has color.ui=always
    // --no-ext-diff: prevent external diff tools from intercepting output
    let range_str;
    let mut args = match mode {
        DiffMode::WorkingTree => vec!["-C", path_str, "diff", "--no-color", "--no-ext-diff"],
        DiffMode::Staged => vec![
            "-C",
            path_str,
            "diff",
            "--cached",
            "--no-color",
            "--no-ext-diff",
        ],
        DiffMode::Commit(ref hash) => {
            crate::validate_git_ref(hash)?;
            range_str = commit_diff_range(path, hash)?;
            vec![
                "-C",
                path_str,
                "diff",
                &range_str,
                "--no-color",
                "--no-ext-diff",
            ]
        }
        DiffMode::BranchCompare { ref base, ref head } => {
            crate::validate_git_ref(base)?;
            crate::validate_git_ref(head)?;
            // Three-dot diff: changes on head since it diverged from base.
            // Without a merge base git refuses the range outright, so fall back
            // to the two-dot diff (see `has_merge_base`).
            range_str = if has_merge_base(path_str, base, head) {
                format!("{}...{}", base, head)
            } else {
                log::debug!("no merge base for {base}...{head}, diffing {base}..{head} instead");
                format!("{}..{}", base, head)
            };
            vec![
                "-C",
                path_str,
                "diff",
                &range_str,
                "--no-color",
                "--no-ext-diff",
            ]
        }
    };

    // `diff.relative` (or a `--relative` in a user's alias) would print paths
    // relative to the project subdir instead of the worktree root, silently
    // moving the base the whole path contract rests on. Pin it.
    args.push("--no-relative");

    // Add -w flag to ignore whitespace changes
    if ignore_whitespace {
        args.push("-w");
    }

    let t0 = std::time::Instant::now();
    let output = safe_output(command("git").args(&args))?;
    log::debug!(
        "[get_diff_with_options] git diff command: {:?}, stdout: {} bytes",
        t0.elapsed(),
        output.stdout.len()
    );

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(GitError::GitExitError {
            status: output.status.code().unwrap_or(-1),
            stderr,
        });
    }

    let t1 = std::time::Instant::now();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut result = parse_unified_diff(&stdout);
    log::debug!(
        "[get_diff_with_options] parse_unified_diff: {:?}, files: {}",
        t1.elapsed(),
        result.files.len()
    );

    // For unstaged mode, also include untracked files
    if matches!(mode, DiffMode::WorkingTree) {
        let t2 = std::time::Instant::now();
        let untracked = get_untracked_files(path);
        log::debug!(
            "[get_diff_with_options] get_untracked_files: {:?}, count: {}",
            t2.elapsed(),
            untracked.len()
        );
        for file_path in untracked {
            if let Some(file_diff) = create_untracked_file_diff(path, &file_path) {
                result.files.push(file_diff);
            }
        }
    }

    log::debug!("[get_diff_with_options] total: {:?}", t_total.elapsed());
    Ok(result)
}

/// Get list of untracked files in a repository.
/// Best-effort: returns an empty list if the gix status walk fails transiently.
fn get_untracked_files(path: &Path) -> Vec<String> {
    crate::gix_helpers::list_untracked_files(path).unwrap_or_default()
}

/// Create a FileDiff for an untracked file (shows entire file as added).
fn create_untracked_file_diff(repo_path: &Path, file_path: &str) -> Option<FileDiff> {
    let full_path = safe_repo_path(repo_path, file_path)?;

    // Check if it's a binary file (simple heuristic)
    let content = match std::fs::read(&full_path) {
        Ok(bytes) => {
            // Check for binary content (null bytes in first 8KB)
            if bytes.iter().take(8192).any(|&b| b == 0) {
                return Some(FileDiff {
                    old_path: None,
                    new_path: Some(file_path.to_string()),
                    hunks: vec![],
                    is_binary: true,
                    lines_added: 0,
                    lines_removed: 0,
                });
            }
            String::from_utf8_lossy(&bytes).to_string()
        }
        Err(_) => return None,
    };

    let lines: Vec<&str> = content.lines().collect();
    let line_count = lines.len();

    // Create a single hunk with all lines as added
    let diff_lines: Vec<DiffLine> = lines
        .into_iter()
        .enumerate()
        .map(|(i, line)| DiffLine {
            line_type: DiffLineType::Added,
            content: line.to_string(),
            old_line_num: None,
            new_line_num: Some(i + 1),
        })
        .collect();

    let hunk = DiffHunk {
        header: format!("@@ -0,0 +1,{} @@ (new file)", line_count),
        old_start: 0,
        new_start: 1,
        lines: vec![DiffLine {
            line_type: DiffLineType::Header,
            content: format!("@@ -0,0 +1,{} @@ (new file)", line_count),
            old_line_num: None,
            new_line_num: None,
        }]
        .into_iter()
        .chain(diff_lines)
        .collect(),
    };

    Some(FileDiff {
        old_path: None,
        new_path: Some(file_path.to_string()),
        hunks: vec![hunk],
        is_binary: false,
        lines_added: line_count,
        lines_removed: 0,
    })
}

// Shared cache for is_git_repo / batch_is_git_repo
static GIT_REPO_CACHE: Mutex<Option<HashMap<PathBuf, (bool, Instant)>>> = Mutex::new(None);
const GIT_REPO_TTL: Duration = Duration::from_secs(30);
const GIT_REPO_MAX_ENTRIES: usize = 256;

/// Check if a path is inside a git repository.
/// Results are cached for 30 seconds to avoid spawning subprocesses on every render.
pub fn is_git_repo(path: &Path) -> bool {
    let path_buf = path.to_path_buf();

    // Check cache first
    {
        let guard = GIT_REPO_CACHE.lock();
        if let Some(ref cache) = *guard
            && let Some(&(result, ts)) = cache.get(&path_buf)
            && ts.elapsed() < GIT_REPO_TTL
        {
            return result;
        }
    }

    let result = crate::gix_helpers::open(path).is_some();

    // Store in cache and evict stale entries
    {
        let mut guard = GIT_REPO_CACHE.lock();
        let cache = guard.get_or_insert_with(HashMap::new);
        cache.insert(path_buf, (result, Instant::now()));
        // Always evict entries older than 5 minutes
        let max_age = Duration::from_secs(300);
        cache.retain(|_, (_, ts)| ts.elapsed() < max_age);
        // Aggressively evict stale entries when above capacity
        if cache.len() > GIT_REPO_MAX_ENTRIES {
            cache.retain(|_, (_, ts)| ts.elapsed() < GIT_REPO_TTL);
        }
    }

    result
}

/// Get the full content of a file from git at a specific revision.
///
/// - `revision` can be "HEAD", a commit hash, or empty for the index (staged version)
pub fn get_file_from_git(repo_path: &Path, revision: &str, file_path: &str) -> Option<String> {
    String::from_utf8(get_file_bytes_from_git(repo_path, revision, file_path)?).ok()
}

/// Get raw file bytes from git at a revision, or from the index for an empty revision.
pub fn get_file_bytes_from_git(
    repo_path: &Path,
    revision: &str,
    file_path: &str,
) -> Option<Vec<u8>> {
    let repo = crate::gix_helpers::open(repo_path)?;

    if revision.is_empty() {
        // Empty revision → stage-0 (staged) version from the index.
        let index = repo.open_index().ok()?;
        let id = index
            .entry_by_path(gix::bstr::BStr::new(file_path.as_bytes()))?
            .id;
        Some(repo.find_object(id).ok()?.data.clone())
    } else {
        // Validate to reject flag injection, then resolve <rev> → tree → blob.
        crate::validate_git_ref(revision).ok()?;
        let tree = repo
            .rev_parse_single(revision)
            .ok()?
            .object()
            .ok()?
            .peel_to_tree()
            .ok()?;
        let entry = tree.lookup_entry_by_path(file_path).ok()??;
        if !entry.mode().is_blob() {
            return None;
        }
        Some(entry.object().ok()?.data.clone())
    }
}

/// Safely join a worktree-root-relative file path to its repository, rejecting
/// path traversal attempts.
///
/// `repo_path` may be any directory inside the repository: diff paths are
/// relative to the worktree root, so resolving them against a monorepo subdir
/// project would read the wrong file (or none). Falls back to `repo_path` when
/// it is not a repository at all.
///
/// Returns `None` if the resolved path escapes the root (e.g. via `../`).
fn safe_repo_path(repo_path: &Path, file_path: &str) -> Option<PathBuf> {
    let root =
        crate::repository::get_repo_root(repo_path).unwrap_or_else(|| repo_path.to_path_buf());
    let canonical = root.join(file_path).canonicalize().ok()?;
    let root_canonical = root.canonicalize().ok()?;
    if canonical.starts_with(&root_canonical) {
        Some(canonical)
    } else {
        None
    }
}

/// Get the full content of a file from the working tree (filesystem).
pub fn get_file_from_working_tree(repo_path: &Path, file_path: &str) -> Option<String> {
    String::from_utf8(get_file_bytes_from_working_tree(repo_path, file_path)?).ok()
}

/// Get raw file bytes from the working tree.
pub fn get_file_bytes_from_working_tree(repo_path: &Path, file_path: &str) -> Option<Vec<u8>> {
    let full_path = safe_repo_path(repo_path, file_path)?;
    std::fs::read(full_path).ok()
}

/// Get both raw sides of a diff, using each side's own path for renames.
pub fn get_file_bytes_for_diff(
    repo_path: &Path,
    old_path: Option<&str>,
    new_path: Option<&str>,
    mode: DiffMode,
) -> (Option<Vec<u8>>, Option<Vec<u8>>) {
    match mode {
        DiffMode::WorkingTree => {
            let old = old_path.and_then(|path| {
                get_file_bytes_from_git(repo_path, "", path)
                    .or_else(|| get_file_bytes_from_git(repo_path, "HEAD", path))
            });
            let new = new_path.and_then(|path| get_file_bytes_from_working_tree(repo_path, path));
            (old, new)
        }
        DiffMode::Staged => {
            let old = old_path.and_then(|path| get_file_bytes_from_git(repo_path, "HEAD", path));
            let new = new_path.and_then(|path| get_file_bytes_from_git(repo_path, "", path));
            (old, new)
        }
        DiffMode::Commit(hash) => {
            let parent = format!("{hash}^");
            let old = old_path.and_then(|path| get_file_bytes_from_git(repo_path, &parent, path));
            let new = new_path.and_then(|path| get_file_bytes_from_git(repo_path, &hash, path));
            (old, new)
        }
        DiffMode::BranchCompare { base, head } => {
            let effective_base = repo_path
                .to_str()
                .and_then(|repo_path| merge_base(repo_path, &base, &head))
                .unwrap_or(base);
            let old =
                old_path.and_then(|path| get_file_bytes_from_git(repo_path, &effective_base, path));
            let new = new_path.and_then(|path| get_file_bytes_from_git(repo_path, &head, path));
            (old, new)
        }
    }
}

/// Get the "old" and "new" file content for a file diff based on the diff mode.
///
/// Returns (old_content, new_content).
/// - For WorkingTree mode: old = HEAD (or index), new = working tree
/// - For Staged mode: old = HEAD, new = index
pub fn get_file_contents_for_diff(
    repo_path: &Path,
    file_path: &str,
    mode: DiffMode,
) -> (Option<String>, Option<String>) {
    let t0 = std::time::Instant::now();
    let result = match mode {
        DiffMode::WorkingTree => {
            // Unstaged: comparing index vs working tree
            // Try index first, fall back to HEAD (they're equal if nothing staged)
            let old = get_file_from_git(repo_path, "", file_path)
                .or_else(|| get_file_from_git(repo_path, "HEAD", file_path));
            let new = get_file_from_working_tree(repo_path, file_path);
            (old, new)
        }
        DiffMode::Staged => {
            // Staged: comparing HEAD vs index
            let old = get_file_from_git(repo_path, "HEAD", file_path);
            let new = get_file_from_git(repo_path, "", file_path);
            (old, new)
        }
        DiffMode::Commit(ref hash) => {
            // Commit: comparing parent^ vs commit
            let parent = format!("{}^", hash);
            let old = get_file_from_git(repo_path, &parent, file_path);
            let new = get_file_from_git(repo_path, hash, file_path);
            (old, new)
        }
        DiffMode::BranchCompare { ref base, ref head } => {
            let effective_base = repo_path
                .to_str()
                .and_then(|repo_path| merge_base(repo_path, base, head))
                .unwrap_or_else(|| base.clone());
            let old = get_file_from_git(repo_path, &effective_base, file_path);
            let new = get_file_from_git(repo_path, head, file_path);
            (old, new)
        }
    };
    log::debug!(
        "[get_file_contents_for_diff] {:?}, file: {}",
        t0.elapsed(),
        file_path
    );
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_file_from_git_reads_head_and_staged_versions() {
        use crate::repository::test_support::{git_in, init_temp_repo};

        // init_temp_repo commits file.txt = "x".
        let (_tmp, repo) = init_temp_repo();
        assert_eq!(
            get_file_from_git(&repo, "HEAD", "file.txt").as_deref(),
            Some("x")
        );

        // Stage a modified version: HEAD stays "x", the index becomes "y".
        std::fs::write(repo.join("file.txt"), "y").unwrap();
        git_in(&repo, &["add", "file.txt"]);
        assert_eq!(
            get_file_from_git(&repo, "HEAD", "file.txt").as_deref(),
            Some("x")
        );
        assert_eq!(
            get_file_from_git(&repo, "", "file.txt").as_deref(),
            Some("y")
        );

        // Missing path resolves to nothing rather than erroring.
        assert!(get_file_from_git(&repo, "HEAD", "nope.txt").is_none());
    }

    #[test]
    fn staged_contents_do_not_fall_back_to_the_working_tree() {
        use crate::repository::test_support::{git_in, init_temp_repo};

        let (_tmp, repo) = init_temp_repo();
        std::fs::remove_file(repo.join("file.txt")).unwrap();
        git_in(&repo, &["add", "file.txt"]);
        std::fs::write(repo.join("file.txt"), "working again").unwrap();

        let (old, new) = get_file_contents_for_diff(&repo, "file.txt", DiffMode::Staged);
        assert_eq!(old.as_deref(), Some("x"));
        assert_eq!(new, None);
    }

    #[test]
    fn binary_diff_contents_use_each_side_path() {
        use crate::repository::test_support::{git_in, init_temp_repo};

        let (_tmp, repo) = init_temp_repo();
        git_in(&repo, &["mv", "file.txt", "renamed.bin"]);
        let bytes = vec![0, 159, 146, 150, 255];
        std::fs::write(repo.join("renamed.bin"), &bytes).unwrap();
        git_in(&repo, &["add", "renamed.bin"]);

        let (old, new) = get_file_bytes_for_diff(
            &repo,
            Some("file.txt"),
            Some("renamed.bin"),
            DiffMode::Staged,
        );
        assert_eq!(old.as_deref(), Some(b"x".as_slice()));
        assert_eq!(new, Some(bytes));
    }

    #[test]
    fn branch_contents_use_the_merge_base_as_the_old_side() {
        use crate::repository::test_support::{git_in, init_temp_repo};

        let (_tmp, repo) = init_temp_repo();
        git_in(&repo, &["checkout", "-b", "feature"]);
        std::fs::write(repo.join("file.txt"), "feature").unwrap();
        git_in(&repo, &["add", "file.txt"]);
        git_in(
            &repo,
            &["-c", "commit.gpgsign=false", "commit", "-m", "feature"],
        );
        git_in(&repo, &["checkout", "main"]);
        std::fs::write(repo.join("file.txt"), "main advanced").unwrap();
        git_in(&repo, &["add", "file.txt"]);
        git_in(
            &repo,
            &["-c", "commit.gpgsign=false", "commit", "-m", "main"],
        );

        let (old, new) = get_file_contents_for_diff(
            &repo,
            "file.txt",
            DiffMode::BranchCompare {
                base: "main".to_string(),
                head: "feature".to_string(),
            },
        );
        assert_eq!(old.as_deref(), Some("x"));
        assert_eq!(new.as_deref(), Some("feature"));
    }

    #[test]
    fn commit_diff_supports_the_root_commit() {
        use crate::repository::test_support::init_temp_repo;

        let (_tmp, repo) = init_temp_repo();
        let diff = get_diff_with_options(&repo, DiffMode::Commit("HEAD".to_string()), false)
            .expect("root commit diff");

        assert_eq!(diff.files.len(), 1);
        assert_eq!(diff.files[0].new_path.as_deref(), Some("file.txt"));
        assert_eq!(diff.files[0].lines_added, 1);
    }

    #[test]
    fn branch_compare_falls_back_to_two_dot_without_a_merge_base() {
        use crate::repository::test_support::{git_in, init_temp_repo};

        // Two roots that share no history — the same shape a shallow clone has
        // once its grafted history no longer reaches the fork point.
        let (_tmp, repo) = init_temp_repo();
        git_in(&repo, &["checkout", "--orphan", "other"]);
        git_in(&repo, &["rm", "-f", "--cached", "file.txt"]);
        std::fs::write(repo.join("other.txt"), "y").unwrap();
        git_in(&repo, &["add", "other.txt"]);
        git_in(
            &repo,
            &["-c", "commit.gpgsign=false", "commit", "-m", "orphan"],
        );

        assert!(!has_merge_base(repo.to_str().unwrap(), "main", "other"));

        let result = get_diff_with_options(
            &repo,
            DiffMode::BranchCompare {
                base: "main".to_string(),
                head: "other".to_string(),
            },
            false,
        )
        .expect("branch compare without a merge base still diffs");

        let paths: Vec<_> = result
            .files
            .iter()
            .map(|f| f.new_path.clone().or_else(|| f.old_path.clone()))
            .collect();
        assert!(paths.contains(&Some("other.txt".to_string())), "{paths:?}");
    }

    #[test]
    fn test_parse_hunk_header() {
        assert_eq!(parse_hunk_header("@@ -1,5 +1,7 @@ fn main()"), (1, 5, 1, 7));
        assert_eq!(parse_hunk_header("@@ -10,3 +15,5 @@"), (10, 3, 15, 5));
        // Omitted counts mean one line on that side.
        assert_eq!(parse_hunk_header("@@ -1 +1 @@"), (1, 1, 1, 1));
        assert_eq!(
            parse_hunk_header("@@ -100,20 +95,15 @@ impl Foo"),
            (100, 20, 95, 15)
        );
        assert_eq!(parse_hunk_header("@@ -0,0 +1,2 @@"), (0, 0, 1, 2));
    }

    #[test]
    fn test_parse_unified_diff() {
        let diff = r#"diff --git a/src/main.rs b/src/main.rs
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,3 +1,4 @@
 fn main() {
+    println!("Hello");
     println!("World");
 }
"#;
        let result = parse_unified_diff(diff);
        assert_eq!(result.files.len(), 1);
        assert_eq!(result.files[0].new_path, Some("src/main.rs".to_string()));
        assert_eq!(result.files[0].lines_added, 1);
        assert_eq!(result.files[0].lines_removed, 0);
        assert_eq!(result.files[0].hunks.len(), 1);
        assert_eq!(result.files[0].hunks[0].lines.len(), 5); // header + 4 lines
    }

    #[test]
    fn test_parse_new_file() {
        let diff = r#"diff --git a/new_file.txt b/new_file.txt
--- /dev/null
+++ b/new_file.txt
@@ -0,0 +1,2 @@
+line 1
+line 2
"#;
        let result = parse_unified_diff(diff);
        assert_eq!(result.files.len(), 1);
        assert!(result.files[0].old_path.is_none());
        assert_eq!(result.files[0].new_path, Some("new_file.txt".to_string()));
        assert_eq!(result.files[0].lines_added, 2);
    }

    #[test]
    fn test_parse_deleted_file() {
        let diff = r#"diff --git a/deleted.txt b/deleted.txt
--- a/deleted.txt
+++ /dev/null
@@ -1,2 +0,0 @@
-line 1
-line 2
"#;
        let result = parse_unified_diff(diff);
        assert_eq!(result.files.len(), 1);
        assert_eq!(result.files[0].old_path, Some("deleted.txt".to_string()));
        assert!(result.files[0].new_path.is_none());
        assert_eq!(result.files[0].lines_removed, 2);
    }

    #[test]
    fn test_diff_mode_toggle() {
        assert_eq!(DiffMode::WorkingTree.toggle(), DiffMode::Staged);
        assert_eq!(DiffMode::Staged.toggle(), DiffMode::WorkingTree);
    }

    #[test]
    fn test_parse_multiple_hunks() {
        let diff = r#"diff --git a/src/main.rs b/src/main.rs
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,3 +1,4 @@
 fn main() {
+    println!("Hello");
     println!("World");
 }
@@ -10,3 +11,4 @@
 fn other() {
+    println!("Added");
     println!("Existing");
 }
"#;
        let result = parse_unified_diff(diff);
        assert_eq!(result.files.len(), 1);
        assert_eq!(result.files[0].hunks.len(), 2);
        assert_eq!(result.files[0].lines_added, 2);
    }

    #[test]
    fn test_parse_multiple_files() {
        let diff = r#"diff --git a/file1.rs b/file1.rs
--- a/file1.rs
+++ b/file1.rs
@@ -1,2 +1,3 @@
 line1
+added
 line2
diff --git a/file2.rs b/file2.rs
--- a/file2.rs
+++ b/file2.rs
@@ -1,3 +1,2 @@
 line1
-removed
 line2
"#;
        let result = parse_unified_diff(diff);
        assert_eq!(result.files.len(), 2);
        assert_eq!(result.files[0].new_path, Some("file1.rs".to_string()));
        assert_eq!(result.files[0].lines_added, 1);
        assert_eq!(result.files[1].new_path, Some("file2.rs".to_string()));
        assert_eq!(result.files[1].lines_removed, 1);
    }

    #[test]
    fn test_parse_binary_file() {
        let diff = r#"diff --git a/image.png b/image.png
Binary files a/image.png and b/image.png differ
"#;
        let result = parse_unified_diff(diff);
        assert_eq!(result.files.len(), 1);
        assert!(result.files[0].is_binary);
        assert!(result.files[0].hunks.is_empty());
    }

    #[test]
    fn test_parse_added_and_deleted_binary_paths() {
        let diff = r#"diff --git a/added.png b/added.png
new file mode 100644
Binary files /dev/null and b/added.png differ
diff --git a/deleted.png b/deleted.png
deleted file mode 100644
Binary files a/deleted.png and /dev/null differ
"#;
        let result = parse_unified_diff(diff);

        assert_eq!(result.files[0].old_path, None);
        assert_eq!(result.files[0].new_path.as_deref(), Some("added.png"));
        assert_eq!(result.files[1].old_path.as_deref(), Some("deleted.png"));
        assert_eq!(result.files[1].new_path, None);
    }

    #[test]
    fn test_parse_empty_diff() {
        let result = parse_unified_diff("");
        assert!(result.is_empty());
        assert_eq!(result.total_added(), 0);
        assert_eq!(result.total_removed(), 0);
    }

    #[test]
    fn test_diff_result_stats() {
        let diff = r#"diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
@@ -1,3 +1,4 @@
 ctx
+add1
+add2
-rem1
 ctx
diff --git a/b.rs b/b.rs
--- a/b.rs
+++ b/b.rs
@@ -1,2 +1,3 @@
 ctx
+add3
"#;
        let result = parse_unified_diff(diff);
        assert_eq!(result.total_added(), 3);
        assert_eq!(result.total_removed(), 1);
    }

    #[test]
    fn test_file_diff_display_name() {
        let file = FileDiff {
            old_path: Some("old.rs".to_string()),
            new_path: Some("new.rs".to_string()),
            hunks: vec![],
            is_binary: false,
            lines_added: 0,
            lines_removed: 0,
        };
        assert_eq!(file.display_name(), "new.rs");

        let deleted = FileDiff {
            old_path: Some("old.rs".to_string()),
            new_path: None,
            hunks: vec![],
            is_binary: false,
            lines_added: 0,
            lines_removed: 0,
        };
        assert_eq!(deleted.display_name(), "old.rs");

        let unknown = FileDiff {
            old_path: None,
            new_path: None,
            hunks: vec![],
            is_binary: false,
            lines_added: 0,
            lines_removed: 0,
        };
        assert_eq!(unknown.display_name(), "unknown");
    }

    #[test]
    fn test_safe_repo_path_normal_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("src/main.rs");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "fn main() {}").unwrap();

        let result = safe_repo_path(dir.path(), "src/main.rs");
        assert!(result.is_some());
        assert!(
            result
                .unwrap()
                .starts_with(dir.path().canonicalize().unwrap())
        );
    }

    #[test]
    fn test_safe_repo_path_traversal_rejected() {
        use crate::repository::test_support::init_temp_repo;

        // A real repository, so the guard is measured against the worktree
        // root it actually resolves against — not the non-repo fallback.
        let (_tmp, repo) = init_temp_repo();
        let project = repo.join("packages").join("app");
        std::fs::create_dir_all(&project).unwrap();

        assert!(safe_repo_path(&repo, "../../../etc/passwd").is_none());
        // Escaping the root from inside a subdirectory project is rejected
        // even though the path stays above the project.
        assert!(safe_repo_path(&project, "../../../etc/passwd").is_none());
        // Climbing out of the project but staying in the repo is allowed:
        // repo-root-relative paths are the contract.
        assert_eq!(
            safe_repo_path(&project, "file.txt"),
            Some(repo.canonicalize().unwrap().join("file.txt"))
        );
    }

    #[test]
    fn test_safe_repo_path_absolute_outside_rejected() {
        let dir = tempfile::tempdir().unwrap();
        // Absolute path outside repo
        let result = safe_repo_path(dir.path(), "/etc/passwd");
        // On Unix, join with an absolute path replaces the base entirely,
        // so this should be rejected since /etc/passwd is outside the repo.
        // On systems where /etc/passwd doesn't exist, canonicalize returns None → safe.
        if let Some(path) = result {
            // If it somehow resolved, it must still be inside the repo
            assert!(path.starts_with(dir.path().canonicalize().unwrap()));
        }
    }

    #[test]
    fn test_get_file_from_working_tree_traversal() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("hello.txt"), "world").unwrap();

        // Normal file works
        assert_eq!(
            get_file_from_working_tree(dir.path(), "hello.txt"),
            Some("world".to_string())
        );

        // Traversal attempt returns None
        assert_eq!(
            get_file_from_working_tree(dir.path(), "../../../etc/passwd"),
            None
        );
    }

    #[test]
    fn test_parse_pure_rename() {
        // A 100%-similarity rename emits no `---`/`+++` lines and no hunks.
        let diff = "diff --git a/src/old_name.rs b/src/new_name.rs\n\
                    similarity index 100%\n\
                    rename from src/old_name.rs\n\
                    rename to src/new_name.rs\n";
        let result = parse_unified_diff(diff);
        assert_eq!(result.files.len(), 1);
        assert_eq!(
            result.files[0].old_path,
            Some("src/old_name.rs".to_string())
        );
        assert_eq!(
            result.files[0].new_path,
            Some("src/new_name.rs".to_string())
        );
        assert!(result.files[0].hunks.is_empty());
        // Bug fix: previously returned "unknown" because both paths were None.
        assert_eq!(result.files[0].display_name(), "src/new_name.rs");
    }

    #[test]
    fn test_parse_rename_with_changes() {
        // A rename with content edits emits both rename headers and `---`/`+++`.
        let diff = r#"diff --git a/src/old.rs b/src/new.rs
similarity index 80%
rename from src/old.rs
rename to src/new.rs
--- a/src/old.rs
+++ b/src/new.rs
@@ -1,3 +1,4 @@
 fn main() {
+    println!("added");
     println!("World");
 }
"#;
        let result = parse_unified_diff(diff);
        assert_eq!(result.files.len(), 1);
        assert_eq!(result.files[0].old_path, Some("src/old.rs".to_string()));
        assert_eq!(result.files[0].new_path, Some("src/new.rs".to_string()));
        assert_eq!(result.files[0].lines_added, 1);
        assert_eq!(result.files[0].hunks.len(), 1);
        assert_eq!(result.files[0].display_name(), "src/new.rs");
    }

    #[test]
    fn test_parse_copy() {
        // `copy from`/`copy to` headers are handled like renames.
        let diff = "diff --git a/src/orig.rs b/src/copy.rs\n\
                    similarity index 100%\n\
                    copy from src/orig.rs\n\
                    copy to src/copy.rs\n";
        let result = parse_unified_diff(diff);
        assert_eq!(result.files.len(), 1);
        assert_eq!(result.files[0].old_path, Some("src/orig.rs".to_string()));
        assert_eq!(result.files[0].new_path, Some("src/copy.rs".to_string()));
        assert_eq!(result.files[0].display_name(), "src/copy.rs");
    }

    #[test]
    fn test_parse_diff_git_header_helper() {
        assert_eq!(
            parse_diff_git_header("diff --git a/src/main.rs b/src/main.rs"),
            (
                Some("src/main.rs".to_string()),
                Some("src/main.rs".to_string())
            )
        );
        assert_eq!(
            parse_diff_git_header("diff --git a/old.rs b/new.rs"),
            (Some("old.rs".to_string()), Some("new.rs".to_string()))
        );
        // Quoted (special-char) paths are decoded, both sides independently.
        assert_eq!(
            parse_diff_git_header("diff --git \"a/has space.rs\" \"b/has space.rs\""),
            (
                Some("has space.rs".to_string()),
                Some("has space.rs".to_string())
            )
        );
        assert_eq!(
            parse_diff_git_header(r#"diff --git a/plain.rs "b/p\303\251.rs""#),
            (Some("plain.rs".to_string()), Some("pé.rs".to_string()))
        );
        assert_eq!(
            parse_diff_git_header(r#"diff --git "a/p\303\251.rs" b/plain.rs"#),
            (Some("pé.rs".to_string()), Some("plain.rs".to_string()))
        );
        // Non-header input.
        assert_eq!(parse_diff_git_header("@@ -1 +1 @@"), (None, None));
    }

    #[test]
    fn decode_git_path_follows_gits_byte_escapes() {
        // Octal escapes are bytes, so `\305\231` is the single char `ř`.
        assert_eq!(
            decode_git_path(r#""sekce/p\305\231ehled.md""#).as_deref(),
            Some("sekce/přehled.md")
        );
        assert_eq!(
            decode_git_path(r#""a\tb.txt""#).as_deref(),
            Some("a\tb.txt")
        );
        assert_eq!(
            decode_git_path(r#""q\"uote.txt""#).as_deref(),
            Some("q\"uote.txt")
        );
        assert_eq!(
            decode_git_path(r#""back\\slash.txt""#).as_deref(),
            Some(r"back\slash.txt")
        );
        assert_eq!(
            decode_git_path(r#""nl\n.txt""#).as_deref(),
            Some("nl\n.txt")
        );
        assert_eq!(
            decode_git_path(r#""bell\a.txt""#).as_deref(),
            Some("bell\u{7}.txt")
        );

        // Only a fully double-quoted string is quoted; the rest passes through.
        assert_eq!(
            decode_git_path("src/main.rs").as_deref(),
            Some("src/main.rs")
        );
        assert_eq!(decode_git_path(r"a\303b").as_deref(), Some(r"a\303b"));
        assert_eq!(decode_git_path("\"").as_deref(), Some("\""));

        // Bytes that are not UTF-8, and malformed escapes, have no path form.
        assert_eq!(decode_git_path(r#""\377.txt""#), None);
        assert_eq!(decode_git_path(r#""\q""#), None);
        assert_eq!(decode_git_path(r#""\30""#), None);
    }

    #[test]
    fn quoted_paths_decode_across_the_whole_file_section() {
        let diff = "diff --git \"a/sekce/p\\305\\231ehled.md\" \"b/sekce/p\\305\\231ehled.md\"\n\
                    --- \"a/sekce/p\\305\\231ehled.md\"\n\
                    +++ \"b/sekce/p\\305\\231ehled.md\"\n\
                    @@ -1,1 +1,1 @@\n\
                    -a\n\
                    +b\n";
        let result = parse_unified_diff(diff);

        assert_eq!(result.files.len(), 1);
        assert_eq!(
            result.files[0].old_path.as_deref(),
            Some("sekce/přehled.md")
        );
        assert_eq!(
            result.files[0].new_path.as_deref(),
            Some("sekce/přehled.md")
        );
        // display_name feeds content loading, stage and discard.
        assert_eq!(result.files[0].display_name(), "sekce/přehled.md");
    }

    #[test]
    fn quoted_rename_decodes_both_sides() {
        let diff = "diff --git \"a/st\\303\\241r\\303\\251.md\" \"b/nov\\303\\251 \\\"one\\\".md\"\n\
                    similarity index 100%\n\
                    rename from \"st\\303\\241r\\303\\251.md\"\n\
                    rename to \"nov\\303\\251 \\\"one\\\".md\"\n";
        let result = parse_unified_diff(diff);

        assert_eq!(result.files.len(), 1);
        assert_eq!(result.files[0].old_path.as_deref(), Some("stáré.md"));
        assert_eq!(result.files[0].new_path.as_deref(), Some("nové \"one\".md"));
    }

    #[test]
    fn tabs_and_backslashes_in_a_new_file_path_decode() {
        let diff = "diff --git \"a/od\\\\tud\\there.md\" \"b/od\\\\tud\\there.md\"\n\
                    new file mode 100644\n\
                    --- /dev/null\n\
                    +++ \"b/od\\\\tud\\there.md\"\n\
                    @@ -0,0 +1,1 @@\n\
                    +x\n";
        let result = parse_unified_diff(diff);

        assert_eq!(result.files.len(), 1);
        assert_eq!(result.files[0].old_path, None);
        assert_eq!(
            result.files[0].new_path.as_deref(),
            Some("od\\tud\there.md")
        );
    }

    #[test]
    fn a_name_with_a_space_drops_gits_tab_terminator() {
        // git ends a `---`/`+++` name containing a space with a tab, quoted or
        // not — keeping it would put the tab in the file identity.
        let diff = "diff --git a/has space.md b/has space.md\n\
                    --- a/has space.md\t\n\
                    +++ b/has space.md\t\n\
                    @@ -1,1 +1,1 @@\n\
                    -a\n\
                    +b\n";
        let result = parse_unified_diff(diff);
        assert_eq!(result.files[0].old_path.as_deref(), Some("has space.md"));
        assert_eq!(result.files[0].new_path.as_deref(), Some("has space.md"));

        let quoted = "diff --git \"a/sekce/nov\\303\\251 jm\\303\\251no.md\" \"b/sekce/nov\\303\\251 jm\\303\\251no.md\"\n\
                      --- \"a/sekce/nov\\303\\251 jm\\303\\251no.md\"\t\n\
                      +++ \"b/sekce/nov\\303\\251 jm\\303\\251no.md\"\t\n\
                      @@ -1,1 +1,1 @@\n\
                      -a\n\
                      +b\n";
        let result = parse_unified_diff(quoted);
        assert_eq!(
            result.files[0].old_path.as_deref(),
            Some("sekce/nové jméno.md")
        );
        assert_eq!(result.files[0].display_name(), "sekce/nové jméno.md");
    }

    #[test]
    fn an_accented_path_survives_a_real_git_diff() {
        use crate::repository::test_support::{git_in, init_temp_repo};

        // Accent (quoting) plus a space (tab terminator) in one real name.
        let (_tmp, repo) = init_temp_repo();
        let name = "sekce/nové jméno.md";
        std::fs::create_dir_all(repo.join("sekce")).unwrap();
        std::fs::write(repo.join(name), "a\n").unwrap();
        git_in(&repo, &["add", "."]);
        git_in(
            &repo,
            &["-c", "commit.gpgsign=false", "commit", "-m", "accented"],
        );
        std::fs::write(repo.join(name), "b\n").unwrap();

        let result = get_diff_with_options(&repo, DiffMode::WorkingTree, false)
            .expect("diff an accented path");
        let names: Vec<&str> = result.files.iter().map(|f| f.display_name()).collect();
        assert_eq!(names, vec![name]);
        // The name is a file identity, not just a label: it has to resolve.
        assert_eq!(
            get_file_from_working_tree(&repo, result.files[0].display_name()).as_deref(),
            Some("b\n")
        );
        assert_eq!(
            get_file_from_git(&repo, "HEAD", result.files[0].display_name()).as_deref(),
            Some("a\n")
        );
    }

    #[test]
    fn test_parse_rename_falls_back_to_git_header() {
        // No explicit rename/`---`/`+++` lines (e.g. mode-only change): paths
        // come from the `diff --git` header so display_name isn't "unknown".
        let diff = "diff --git a/script.sh b/script.sh\n\
                    old mode 100644\n\
                    new mode 100755\n";
        let result = parse_unified_diff(diff);
        assert_eq!(result.files.len(), 1);
        assert_eq!(result.files[0].new_path, Some("script.sh".to_string()));
        assert_eq!(result.files[0].display_name(), "script.sh");
    }

    #[test]
    fn test_numstat_rename_arrow_and_brace_forms() {
        // Documents the shapes `git diff --numstat` (without --no-renames) emits
        // for renames, and confirms the `--no-renames` choice avoids them: the
        // path column would otherwise carry an arrow that we'd store verbatim.
        // Bare arrow form.
        let bare = "0\t0\told.rs => new.rs";
        let parts: Vec<&str> = bare.split('\t').collect();
        assert!(parts[2].contains(" => "));
        // Brace form.
        let brace = "3\t1\tdir/{old => new}/file.rs";
        let parts: Vec<&str> = brace.split('\t').collect();
        assert!(parts[2].contains("{old => new}"));
        // Binary rename uses "-" for the counts.
        let binary = "-\t-\tassets/{a => b}/logo.png";
        let parts: Vec<&str> = binary.split('\t').collect();
        assert_eq!(parts[0], "-");
        assert_eq!(parts[1], "-");
    }

    #[test]
    fn hunk_content_shaped_like_a_file_header_stays_content() {
        // Added source beginning "++ " and removed source beginning "-- "
        // produce lines indistinguishable from `+++`/`---` headers. Only the
        // hunk's `@@` counts say which is which.
        let diff = r#"diff --git a/src/main.rs b/src/main.rs
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,3 +1,4 @@
 fn main() {}
+++ b/attacker.rs
+--- a/attacker.rs
--- a/victim.rs
 tail
"#;
        let result = parse_unified_diff(diff);

        assert_eq!(result.files.len(), 1);
        let file = &result.files[0];
        assert_eq!(file.new_path.as_deref(), Some("src/main.rs"));
        assert_eq!(file.old_path.as_deref(), Some("src/main.rs"));
        assert_eq!(
            file.display_name(),
            "src/main.rs",
            "display_name feeds discard/delete, so hunk text must never reach it"
        );
        assert_eq!(file.lines_added, 2);
        assert_eq!(file.lines_removed, 1);
        let contents: Vec<&str> = file.hunks[0]
            .lines
            .iter()
            .map(|line| line.content.as_str())
            .collect();
        assert!(contents.contains(&"++ b/attacker.rs"), "{contents:?}");
        assert!(contents.contains(&"--- a/attacker.rs"), "{contents:?}");
        assert!(contents.contains(&"-- a/victim.rs"), "{contents:?}");
    }

    #[test]
    fn hunk_content_shaped_like_a_diff_header_does_not_start_a_file() {
        let diff = r#"diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
@@ -1,1 +1,2 @@
 keep
+diff --git a/evil.rs b/evil.rs
diff --git a/b.rs b/b.rs
--- a/b.rs
+++ b/b.rs
@@ -1,1 +1,1 @@
-x
+y
"#;
        let result = parse_unified_diff(diff);

        let names: Vec<&str> = result.files.iter().map(|f| f.display_name()).collect();
        assert_eq!(names, vec!["a.rs", "b.rs"]);
        assert_eq!(result.files[0].lines_added, 1);
        assert_eq!(result.files[1].lines_added, 1);
        assert_eq!(result.files[1].lines_removed, 1);
    }

    #[test]
    fn working_tree_reads_resolve_against_the_worktree_root() {
        use crate::repository::test_support::init_temp_repo;

        let (_tmp, repo) = init_temp_repo();
        let project = repo.join("packages").join("app");
        std::fs::create_dir_all(project.join("packages").join("app")).unwrap();
        std::fs::write(project.join("f.txt"), "the real file\n").unwrap();
        // Resolving a repo-relative diff path against the project instead of
        // the worktree root lands on this colliding decoy.
        std::fs::write(
            project.join("packages").join("app").join("f.txt"),
            "the decoy\n",
        )
        .unwrap();

        assert_eq!(
            get_file_from_working_tree(&project, "packages/app/f.txt").as_deref(),
            Some("the real file\n")
        );
    }

    #[test]
    fn tracked_and_untracked_diff_paths_share_one_base() {
        use crate::repository::test_support::{git_in, init_temp_repo};

        let (_tmp, repo) = init_temp_repo();
        let project = repo.join("packages").join("app");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(project.join("tracked.txt"), "base\n").unwrap();
        git_in(&repo, &["add", "."]);
        git_in(
            &repo,
            &["-c", "commit.gpgsign=false", "commit", "-m", "package"],
        );
        std::fs::write(project.join("tracked.txt"), "changed\n").unwrap();
        std::fs::write(project.join("fresh.txt"), "new\n").unwrap();

        let result = get_diff_with_options(&project, DiffMode::WorkingTree, false)
            .expect("diff a subdirectory project");
        let names: Vec<&str> = result.files.iter().map(|f| f.display_name()).collect();
        assert!(names.contains(&"packages/app/tracked.txt"), "{names:?}");
        assert!(names.contains(&"packages/app/fresh.txt"), "{names:?}");
        // The untracked entry must render its content, which only works when
        // the path resolves against the same root.
        let fresh = result
            .files
            .iter()
            .find(|f| f.display_name() == "packages/app/fresh.txt")
            .expect("untracked file diff");
        assert_eq!(fresh.lines_added, 1);
    }

    #[test]
    fn diff_paths_keep_their_base_when_diff_relative_is_configured() {
        use crate::repository::test_support::{git_in, init_temp_repo};

        // Two same-named files, one at the root and one in the project, so a
        // path that loses its base still resolves — onto the wrong file.
        let (_tmp, repo) = init_temp_repo();
        let project = repo.join("packages").join("app");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(repo.join("f.txt"), "root base\n").unwrap();
        std::fs::write(project.join("f.txt"), "project base\n").unwrap();
        git_in(&repo, &["add", "."]);
        git_in(
            &repo,
            &["-c", "commit.gpgsign=false", "commit", "-m", "two f.txt"],
        );
        git_in(&repo, &["config", "diff.relative", "true"]);
        std::fs::write(repo.join("f.txt"), "root edited\n").unwrap();
        std::fs::write(project.join("f.txt"), "project edited\n").unwrap();

        let result = get_diff_with_options(&project, DiffMode::WorkingTree, false)
            .expect("diff a subdirectory project");
        let mut names: Vec<&str> = result.files.iter().map(|f| f.display_name()).collect();
        names.sort_unstable();
        assert_eq!(
            names,
            vec!["f.txt", "packages/app/f.txt"],
            "diff.relative must not move the base the mutations resolve against"
        );

        // Discard exactly what the file tree offers for the project's file —
        // with the base unpinned that name is a bare "f.txt", which reverts the
        // root file instead.
        let clicked = result
            .files
            .iter()
            .find(|file| {
                file.hunks.iter().flat_map(|hunk| &hunk.lines).any(|line| {
                    line.line_type == DiffLineType::Added && line.content == "project edited"
                })
            })
            .expect("the project's file is in the diff")
            .display_name()
            .to_string();
        crate::repository::discard_file_changes(&project, &clicked)
            .expect("discard the project's file");
        assert_eq!(
            std::fs::read_to_string(project.join("f.txt")).unwrap(),
            "project base\n"
        );
        assert_eq!(
            std::fs::read_to_string(repo.join("f.txt")).unwrap(),
            "root edited\n",
            "the root file's uncommitted work must be untouched"
        );
    }

    #[test]
    fn test_parse_no_newline_at_eof() {
        let diff = r#"diff --git a/file.txt b/file.txt
--- a/file.txt
+++ b/file.txt
@@ -1,2 +1,2 @@
 line1
-line2
\ No newline at end of file
+line2_modified
\ No newline at end of file
"#;
        let result = parse_unified_diff(diff);
        assert_eq!(result.files.len(), 1);
        assert_eq!(result.files[0].lines_added, 1);
        assert_eq!(result.files[0].lines_removed, 1);
    }
}
