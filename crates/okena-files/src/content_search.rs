//! File content search engine using grep-searcher + ignore.
//!
//! Provides async file content search with streaming results,
//! supporting literal, regex, and fuzzy matching modes.

use grep_matcher::Matcher;
use grep_regex::RegexMatcherBuilder;
use grep_searcher::Searcher;
use grep_searcher::sinks::UTF8;
use ignore::{WalkBuilder, WalkState};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// Skip files larger than this for content search. Lockfiles, bundles, and
/// generated code are typically uninteresting and dominate I/O time.
const MAX_FILE_SIZE: u64 = 1_000_000;
const MAX_RESULTS: usize = 10_000;
const MAX_CONTEXT_LINES: usize = 20;
const MAX_MATCH_RANGES_PER_LINE: usize = 2_048;
const MAX_SNIPPET_BYTES: usize = 64 * 1024;
const MAX_PAYLOAD_BYTES: usize = 16 * 1024 * 1024;

/// A single search match within a file.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ContentMatch {
    /// 1-based line number.
    pub line_number: usize,
    /// The full line content (trimmed of trailing newline).
    pub line_content: String,
    /// Byte ranges within `line_content` that matched.
    pub match_ranges: Vec<Range<usize>>,
    /// Context lines before the match (line_number, content). Empty if context_lines = 0.
    pub context_before: Vec<(usize, String)>,
    /// Context lines after the match (line_number, content). Empty if context_lines = 0.
    pub context_after: Vec<(usize, String)>,
}

/// Expand tabs to 4 spaces in a string and remap byte ranges accordingly.
///
/// The syntax highlighter expands tabs to spaces, so match ranges computed on
/// the raw text would be misaligned. This function applies the same expansion
/// and adjusts all ranges to match the expanded string.
fn expand_tabs(text: &str, ranges: &[Range<usize>]) -> (String, Vec<Range<usize>>) {
    let mut expanded = String::with_capacity(text.len().min(MAX_SNIPPET_BYTES));
    // Match boundaries are UTF-8 boundaries, so only character boundaries need
    // entries in this byte-indexed map.
    let mut offset_map = vec![None; text.len() + 1];
    let mut expanded_pos: usize = 0;

    for (orig_pos, ch) in text.char_indices() {
        offset_map[orig_pos] = Some(expanded_pos);
        let expanded_len = if ch == '\t' { 4 } else { ch.len_utf8() };
        if expanded_pos + expanded_len > MAX_SNIPPET_BYTES {
            break;
        }
        if ch == '\t' {
            expanded.push_str("    ");
            expanded_pos += 4;
        } else {
            expanded.push(ch);
            expanded_pos += ch.len_utf8();
        }
        offset_map[orig_pos + ch.len_utf8()] = Some(expanded_pos);
    }

    let new_ranges = ranges
        .iter()
        .filter_map(|r| {
            let start = offset_map.get(r.start).copied().flatten()?;
            let end = offset_map
                .get(r.end)
                .copied()
                .flatten()
                .unwrap_or(expanded.len());
            Some(start..end)
        })
        .filter(|range| range.start < range.end && range.end <= expanded.len())
        .collect();

    (expanded, new_ranges)
}

fn expand_bounded_match_line(text: &str, ranges: &[Range<usize>]) -> (String, Vec<Range<usize>>) {
    if text.len() <= MAX_SNIPPET_BYTES {
        return expand_tabs(text, ranges);
    }

    // Keep the first match visible when a generated/minified line exceeds the
    // snippet cap. Matcher offsets are UTF-8 boundaries.
    let start = ranges.first().map_or(0, |range| range.start);
    let mut end = (start + MAX_SNIPPET_BYTES).min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let adjusted_ranges: Vec<_> = ranges
        .iter()
        .filter(|range| range.end > start && range.start < end)
        .map(|range| range.start.saturating_sub(start)..range.end.min(end) - start)
        .collect();
    expand_tabs(&text[start..end], &adjusted_ranges)
}

/// Expand tabs to 4 spaces in a string (no range remapping needed).
fn expand_tabs_simple(text: &str) -> String {
    expand_tabs(text, &[]).0
}

/// Search results grouped by file.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct FileSearchResult {
    pub file_path: PathBuf,
    pub relative_path: String,
    pub matches: Vec<ContentMatch>,
    /// Best match score in this file (for sorting files by relevance). 0 for non-fuzzy.
    pub best_score: u16,
}

/// Search mode.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum SearchMode {
    /// Literal text match.
    #[default]
    Literal,
    /// Regex match.
    Regex,
    /// Fuzzy match using nucleo-matcher.
    Fuzzy,
}

/// Configuration for a content search.
#[derive(Clone, Debug)]
pub struct ContentSearchConfig {
    pub case_sensitive: bool,
    pub mode: SearchMode,
    pub max_results: usize,
    pub file_glob: Option<String>,
    /// Number of context lines before/after each match (0 = no context).
    pub context_lines: usize,
    /// When true, include gitignored files in search.
    pub show_ignored: bool,
}

impl Default for ContentSearchConfig {
    fn default() -> Self {
        Self {
            case_sensitive: false,
            mode: SearchMode::Literal,
            max_results: 1000,
            file_glob: None,
            context_lines: 0,
            show_ignored: false,
        }
    }
}

/// Always ignored regardless of `.gitignore` or the user's "Include gitignored" toggle.
/// `.git/` itself isn't covered by gitignore patterns and there's no reason to ever walk it.
/// `.claude/worktrees/` are agent worktrees (full repo checkouts) — gitignore inside each
/// sub-worktree masks them from the parent's view, so they slip past gitignore-based filtering
/// and can blow the file scan budget.
pub const ALWAYS_IGNORE: &[&str] = &["!.git/", "!.claude/worktrees/"];

/// Configure a walker with the project's ignore rules and our defaults.
fn configure_walker(
    project_path: &Path,
    config: &ContentSearchConfig,
) -> Result<WalkBuilder, String> {
    let mut walk_builder = WalkBuilder::new(project_path);
    walk_builder
        .hidden(false)
        .git_ignore(!config.show_ignored)
        .git_global(!config.show_ignored)
        .git_exclude(!config.show_ignored)
        .max_depth(Some(20))
        .max_filesize(Some(MAX_FILE_SIZE));

    // Build overrides: always-ignore dirs + optional user glob filter
    let mut override_builder = ignore::overrides::OverrideBuilder::new(project_path);
    for pattern in ALWAYS_IGNORE {
        override_builder
            .add(pattern)
            .map_err(|error| format!("Invalid built-in file glob '{pattern}': {error}"))?;
    }
    if let Some(ref glob) = config.file_glob {
        override_builder
            .add(glob)
            .map_err(|error| format!("Invalid file glob '{glob}': {error}"))?;
    }
    let overrides = override_builder
        .build()
        .map_err(|error| format!("Invalid file glob: {error}"))?;
    walk_builder.overrides(overrides);

    Ok(walk_builder)
}

/// Add context lines to matches by reading the file content.
fn add_context_lines(matches: &mut [ContentMatch], file_path: &Path, context_lines: usize) {
    if context_lines == 0 || matches.is_empty() {
        return;
    }

    let content = match std::fs::read_to_string(file_path) {
        Ok(c) => c,
        Err(_) => return,
    };

    let all_lines: Vec<&str> = content.lines().collect();

    for m in matches.iter_mut() {
        let line_idx = m.line_number.saturating_sub(1); // 0-based

        // Context before
        let start = line_idx.saturating_sub(context_lines);
        for i in start..line_idx {
            m.context_before
                .push((i + 1, expand_tabs_simple(all_lines.get(i).unwrap_or(&""))));
        }

        // Context after
        let end = (line_idx + 1 + context_lines).min(all_lines.len());
        for i in (line_idx + 1)..end {
            m.context_after
                .push((i + 1, expand_tabs_simple(all_lines.get(i).unwrap_or(&""))));
        }
    }
}

/// Atomically claim up to `requested` result slots from the shared limit.
fn reserve_match_slots(total: &AtomicUsize, max_results: usize, requested: usize) -> usize {
    let mut current = total.load(Ordering::Acquire);
    loop {
        let remaining = max_results.saturating_sub(current);
        let reserved = requested.min(remaining);
        if reserved == 0 {
            return 0;
        }
        match total.compare_exchange_weak(
            current,
            current + reserved,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => return reserved,
            Err(observed) => current = observed,
        }
    }
}

fn estimated_payload_bytes(result: &FileSearchResult) -> usize {
    // JSON may escape each input byte as a six-byte sequence. The fixed costs
    // also cover field names, numbers, commas, and collection delimiters.
    let escaped = |len: usize| len.saturating_mul(6);
    let mut bytes = 512usize
        .saturating_add(escaped(result.file_path.to_string_lossy().len()))
        .saturating_add(escaped(result.relative_path.len()));
    for found in &result.matches {
        bytes = bytes
            .saturating_add(256)
            .saturating_add(escaped(found.line_content.len()))
            .saturating_add(found.match_ranges.len().saturating_mul(48));
        for (_, line) in found.context_before.iter().chain(&found.context_after) {
            bytes = bytes.saturating_add(64).saturating_add(escaped(line.len()));
        }
    }
    bytes
}

fn reserve_payload_bytes(total: &AtomicUsize, requested: usize) -> bool {
    let mut current = total.load(Ordering::Acquire);
    loop {
        let Some(next) = current.checked_add(requested) else {
            return false;
        };
        if next > MAX_PAYLOAD_BYTES {
            return false;
        }
        match total.compare_exchange_weak(current, next, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => return true,
            Err(observed) => current = observed,
        }
    }
}

/// Run a content search in the given project directory.
///
/// Streams results back via the `on_result` callback. Returns when the search
/// is complete or cancelled (via the `cancelled` flag).
///
/// This is designed to be called from a background thread.
pub fn search_content(
    project_path: &Path,
    query: &str,
    config: &ContentSearchConfig,
    cancelled: &AtomicBool,
    on_result: &mut (dyn FnMut(FileSearchResult) + Send),
) -> Result<(), String> {
    if query.is_empty() {
        return Ok(());
    }

    let mut effective_config = config.clone();
    effective_config.max_results = effective_config.max_results.min(MAX_RESULTS);
    effective_config.context_lines = effective_config.context_lines.min(MAX_CONTEXT_LINES);

    let walker = configure_walker(project_path, &effective_config)?;
    match effective_config.mode {
        SearchMode::Fuzzy => search_content_fuzzy(
            walker,
            project_path,
            query,
            &effective_config,
            cancelled,
            on_result,
        ),
        _ => search_content_grep(
            walker,
            project_path,
            query,
            &effective_config,
            cancelled,
            on_result,
        ),
    }
}

/// Search using grep-searcher (literal or regex mode).
///
/// Walks the project tree in parallel. Each worker thread keeps its own
/// `Searcher` (it's stateful) and a clone of the matcher; results are funneled
/// through a `Mutex` around the caller's callback.
fn search_content_grep(
    walker: WalkBuilder,
    project_path: &Path,
    query: &str,
    config: &ContentSearchConfig,
    cancelled: &AtomicBool,
    on_result: &mut (dyn FnMut(FileSearchResult) + Send),
) -> Result<(), String> {
    let matcher = {
        let mut builder = RegexMatcherBuilder::new();
        builder.case_insensitive(!config.case_sensitive);

        let pattern = if config.mode == SearchMode::Regex {
            query.to_string()
        } else {
            escape_regex(query)
        };
        builder
            .build(&pattern)
            .map_err(|error| format!("Invalid regular expression: {error}"))?
    };

    let total_matches = AtomicUsize::new(0);
    let total_payload_bytes = AtomicUsize::new(0);
    let max_results = config.max_results;
    let context_lines = config.context_lines;
    let on_result = Mutex::new(on_result);

    walker.build_parallel().run(|| {
        let matcher = matcher.clone();
        let mut searcher = Searcher::new();
        let total_matches = &total_matches;
        let total_payload_bytes = &total_payload_bytes;
        let on_result = &on_result;

        Box::new(move |entry| {
            if cancelled.load(Ordering::Relaxed) {
                return WalkState::Quit;
            }
            if total_matches.load(Ordering::Relaxed) >= max_results {
                return WalkState::Quit;
            }

            let entry = match entry {
                Ok(e) => e,
                Err(_) => return WalkState::Continue,
            };

            if !entry.file_type().is_some_and(|ft| ft.is_file()) {
                return WalkState::Continue;
            }

            let path = entry.path();
            let mut file_matches: Vec<ContentMatch> = Vec::new();

            let search_result = searcher.search_path(
                &matcher,
                path,
                UTF8(|line_number, line_content| {
                    if cancelled.load(Ordering::Relaxed) {
                        return Ok(false);
                    }
                    if total_matches.load(Ordering::Relaxed) + file_matches.len() >= max_results {
                        return Ok(false);
                    }

                    let line_trimmed = line_content.trim_end_matches(&['\n', '\r'][..]);

                    // Find match ranges within the line
                    let mut match_ranges = Vec::new();
                    matcher
                        .find_iter(line_content.as_bytes(), |m| {
                            let start = m.start();
                            let end = m.end().min(line_trimmed.len());
                            if start < line_trimmed.len() {
                                match_ranges.push(start..end);
                            }
                            match_ranges.len() < MAX_MATCH_RANGES_PER_LINE
                        })
                        .ok();

                    // Expand tabs to match syntax highlighter output
                    let (line_expanded, match_ranges) =
                        expand_bounded_match_line(line_trimmed, &match_ranges);

                    file_matches.push(ContentMatch {
                        line_number: line_number as usize,
                        line_content: line_expanded,
                        match_ranges,
                        context_before: Vec::new(),
                        context_after: Vec::new(),
                    });

                    Ok(true)
                }),
            );

            if search_result.is_err() || file_matches.is_empty() {
                return WalkState::Continue;
            }

            let reserved = reserve_match_slots(total_matches, max_results, file_matches.len());
            if reserved == 0 {
                return WalkState::Quit;
            }
            file_matches.truncate(reserved);

            if context_lines > 0 {
                add_context_lines(&mut file_matches, path, context_lines);
            }

            let relative_path = path
                .strip_prefix(project_path)
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| path.to_string_lossy().to_string());

            let result = FileSearchResult {
                file_path: path.to_path_buf(),
                relative_path,
                matches: file_matches,
                best_score: 0,
            };

            if !reserve_payload_bytes(total_payload_bytes, estimated_payload_bytes(&result)) {
                return WalkState::Quit;
            }

            if let Ok(mut cb) = on_result.lock() {
                cb(result);
            }

            WalkState::Continue
        })
    });
    Ok(())
}

/// Search using nucleo-matcher (fuzzy mode).
///
/// Walks the project tree in parallel; each worker thread keeps its own
/// `Matcher` (it's stateful) and reads file contents independently.
fn search_content_fuzzy(
    walker: WalkBuilder,
    project_path: &Path,
    query: &str,
    config: &ContentSearchConfig,
    cancelled: &AtomicBool,
    on_result: &mut (dyn FnMut(FileSearchResult) + Send),
) -> Result<(), String> {
    use nucleo_matcher::{Config as NucleoConfig, Matcher, Utf32Str};

    let total_matches = AtomicUsize::new(0);
    let total_payload_bytes = AtomicUsize::new(0);
    let max_results = config.max_results;
    let context_lines = config.context_lines;
    let on_result = Mutex::new(on_result);

    // Minimum score threshold — scale with query length.
    // Short queries need higher threshold to avoid noise.
    let query_len = query.chars().count();
    let min_score: u16 = match query_len {
        0..=2 => 80,
        3..=4 => 50,
        _ => 30,
    };

    walker.build_parallel().run(|| {
        let mut matcher = Matcher::new(NucleoConfig::DEFAULT);
        let total_matches = &total_matches;
        let total_payload_bytes = &total_payload_bytes;
        let on_result = &on_result;

        Box::new(move |entry| {
            if cancelled.load(Ordering::Relaxed) {
                return WalkState::Quit;
            }
            if total_matches.load(Ordering::Relaxed) >= max_results {
                return WalkState::Quit;
            }

            let entry = match entry {
                Ok(e) => e,
                Err(_) => return WalkState::Continue,
            };

            if !entry.file_type().is_some_and(|ft| ft.is_file()) {
                return WalkState::Continue;
            }

            let path = entry.path();
            let content = match std::fs::read_to_string(path) {
                Ok(c) => c,
                Err(_) => return WalkState::Continue,
            };

            let mut scored_matches: Vec<(u16, ContentMatch)> = Vec::new();

            for (line_idx, line) in content.lines().enumerate() {
                if cancelled.load(Ordering::Relaxed) {
                    return WalkState::Quit;
                }
                if total_matches.load(Ordering::Relaxed) + scored_matches.len() >= max_results {
                    break;
                }

                let mut haystack_buf = Vec::new();
                let haystack = Utf32Str::new(line, &mut haystack_buf);

                let mut needle_buf2 = Vec::new();
                let needle = Utf32Str::new(query, &mut needle_buf2);

                let mut indices: Vec<u32> = Vec::new();
                if let Some(score) = matcher.fuzzy_indices(haystack, needle, &mut indices) {
                    if score < min_score {
                        continue;
                    }

                    let char_to_byte: Vec<(usize, char)> = line.char_indices().collect();
                    let match_ranges: Vec<Range<usize>> = indices
                        .iter()
                        .take(MAX_MATCH_RANGES_PER_LINE)
                        .filter_map(|&idx| {
                            let (byte_pos, ch) = char_to_byte.get(idx as usize)?;
                            Some(*byte_pos..*byte_pos + ch.len_utf8())
                        })
                        .collect();

                    // Expand tabs to match syntax highlighter output
                    let (line_expanded, match_ranges) =
                        expand_bounded_match_line(line, &match_ranges);

                    scored_matches.push((
                        score,
                        ContentMatch {
                            line_number: line_idx + 1,
                            line_content: line_expanded,
                            match_ranges,
                            context_before: Vec::new(),
                            context_after: Vec::new(),
                        },
                    ));
                }
            }

            if scored_matches.is_empty() {
                return WalkState::Continue;
            }

            // Sort by score descending — best matches first
            scored_matches.sort_by_key(|b| std::cmp::Reverse(b.0));

            let reserved = reserve_match_slots(total_matches, max_results, scored_matches.len());
            if reserved == 0 {
                return WalkState::Quit;
            }
            scored_matches.truncate(reserved);

            let best_score = scored_matches.first().map(|(s, _)| *s).unwrap_or(0);
            let mut file_matches: Vec<ContentMatch> =
                scored_matches.into_iter().map(|(_, m)| m).collect();

            if context_lines > 0 {
                add_context_lines(&mut file_matches, path, context_lines);
            }

            let relative_path = path
                .strip_prefix(project_path)
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| path.to_string_lossy().to_string());

            let result = FileSearchResult {
                file_path: path.to_path_buf(),
                relative_path,
                matches: file_matches,
                best_score,
            };

            if !reserve_payload_bytes(total_payload_bytes, estimated_payload_bytes(&result)) {
                return WalkState::Quit;
            }

            if let Ok(mut cb) = on_result.lock() {
                cb(result);
            }

            WalkState::Continue
        })
    });
    Ok(())
}

/// Handle for cancelling a running search.
#[derive(Clone)]
pub struct SearchHandle {
    cancelled: Arc<AtomicBool>,
}

impl Default for SearchHandle {
    fn default() -> Self {
        Self::new()
    }
}

impl SearchHandle {
    pub fn new() -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    pub fn flag(&self) -> Arc<AtomicBool> {
        self.cancelled.clone()
    }
}

/// Escape special regex characters in a string for literal matching.
fn escape_regex(s: &str) -> String {
    let mut escaped = String::with_capacity(s.len() * 2);
    for c in s.chars() {
        match c {
            '\\' | '.' | '+' | '*' | '?' | '(' | ')' | '|' | '[' | ']' | '{' | '}' | '^' | '$' => {
                escaped.push('\\');
                escaped.push(c);
            }
            _ => escaped.push(c),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::AtomicBool;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new() -> Self {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "okena-content-search-{}-{}",
                std::process::id(),
                now
            ));
            fs::create_dir(&path).unwrap();
            Self { path }
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn search_content_skips_files_larger_than_cap() {
        let dir = TempDir::new();
        fs::write(dir.path.join("small.txt"), "needle\n").unwrap();
        let mut big = b"needle\n".to_vec();
        big.resize(MAX_FILE_SIZE as usize + 1, b'a');
        fs::write(dir.path.join("big.log"), big).unwrap();

        let cancelled = AtomicBool::new(false);
        let config = ContentSearchConfig::default();
        let mut results: Vec<String> = Vec::new();
        let mut on_result = |result: FileSearchResult| {
            results.push(result.relative_path);
        };

        search_content(&dir.path, "needle", &config, &cancelled, &mut on_result).unwrap();

        assert!(results.iter().any(|path| path == "small.txt"));
        assert!(!results.iter().any(|path| path == "big.log"));
    }

    fn assert_dense_parallel_search_respects_cap(mode: SearchMode) {
        let dir = TempDir::new();
        for file_index in 0..32 {
            let content = (0..32)
                .map(|line_index| format!("needle {file_index} {line_index}\n"))
                .collect::<String>();
            fs::write(dir.path.join(format!("dense-{file_index}.txt")), content).unwrap();
        }

        let cancelled = AtomicBool::new(false);
        let config = ContentSearchConfig {
            mode,
            max_results: 17,
            ..ContentSearchConfig::default()
        };
        let mut matches = Vec::new();
        search_content(&dir.path, "needle", &config, &cancelled, &mut |result| {
            matches.extend(
                result
                    .matches
                    .into_iter()
                    .map(|found| (result.relative_path.clone(), found.line_number)),
            );
        })
        .unwrap();

        let unique = matches.iter().collect::<std::collections::HashSet<_>>();
        assert_eq!(matches.len(), config.max_results);
        assert_eq!(unique.len(), matches.len());
    }

    #[test]
    fn parallel_grep_search_has_a_strict_result_cap() {
        assert_dense_parallel_search_respects_cap(SearchMode::Literal);
    }

    #[test]
    fn parallel_fuzzy_search_has_a_strict_result_cap() {
        assert_dense_parallel_search_respects_cap(SearchMode::Fuzzy);
    }

    #[test]
    fn invalid_file_glob_returns_an_error_without_results() {
        let dir = TempDir::new();
        fs::write(dir.path.join("match.txt"), "needle\n").unwrap();
        let cancelled = AtomicBool::new(false);
        let config = ContentSearchConfig {
            file_glob: Some("[".to_string()),
            ..ContentSearchConfig::default()
        };
        let mut result_count = 0;

        let error = search_content(&dir.path, "needle", &config, &cancelled, &mut |_| {
            result_count += 1
        })
        .unwrap_err();

        assert!(error.contains("Invalid file glob"), "{error}");
        assert_eq!(result_count, 0);
    }

    #[test]
    fn invalid_regex_returns_an_error_without_results() {
        let dir = TempDir::new();
        fs::write(dir.path.join("match.txt"), "needle\n").unwrap();
        let cancelled = AtomicBool::new(false);
        let config = ContentSearchConfig {
            mode: SearchMode::Regex,
            ..ContentSearchConfig::default()
        };
        let mut result_count = 0;

        let error = search_content(&dir.path, "(", &config, &cancelled, &mut |_| {
            result_count += 1
        })
        .unwrap_err();

        assert!(error.contains("Invalid regular expression"), "{error}");
        assert_eq!(result_count, 0);
    }

    #[test]
    fn minified_line_has_bounded_ranges_and_payload() {
        let dir = TempDir::new();
        fs::write(dir.path.join("minified.js"), "a".repeat(900_000)).unwrap();
        let cancelled = AtomicBool::new(false);
        let mut results = Vec::new();

        search_content(
            &dir.path,
            "a",
            &ContentSearchConfig::default(),
            &cancelled,
            &mut |result| results.push(result),
        )
        .unwrap();

        assert_eq!(results.len(), 1);
        let found = &results[0].matches[0];
        assert!(found.line_content.len() <= MAX_SNIPPET_BYTES);
        assert_eq!(found.match_ranges.len(), MAX_MATCH_RANGES_PER_LINE);
        assert!(
            found
                .match_ranges
                .iter()
                .all(|range| range.end <= found.line_content.len())
        );
        assert!(serde_json::to_vec(&results).unwrap().len() < 1_000_000);
    }

    #[test]
    fn parallel_files_share_one_payload_budget() {
        let dir = TempDir::new();
        let line = format!("needle{}", "x".repeat(MAX_SNIPPET_BYTES));
        for file_index in 0..300 {
            fs::write(dir.path.join(format!("large-{file_index}.txt")), &line).unwrap();
        }
        let cancelled = AtomicBool::new(false);
        let config = ContentSearchConfig {
            max_results: usize::MAX,
            ..ContentSearchConfig::default()
        };
        let mut results = Vec::new();

        search_content(&dir.path, "needle", &config, &cancelled, &mut |result| {
            results.push(result)
        })
        .unwrap();

        let estimated: usize = results.iter().map(estimated_payload_bytes).sum();
        assert!(estimated <= MAX_PAYLOAD_BYTES);
        assert!(results.len() < 300);
    }

    #[test]
    fn unicode_lines_and_context_are_bounded_at_char_boundaries() {
        let dir = TempDir::new();
        let long_unicode = "🦀".repeat(MAX_SNIPPET_BYTES);
        fs::write(
            dir.path.join("unicode.txt"),
            format!("{long_unicode}\nneedle{long_unicode}\n{long_unicode}\n"),
        )
        .unwrap();
        let cancelled = AtomicBool::new(false);
        let config = ContentSearchConfig {
            context_lines: usize::MAX,
            ..ContentSearchConfig::default()
        };
        let mut results = Vec::new();

        search_content(&dir.path, "needle", &config, &cancelled, &mut |result| {
            results.push(result)
        })
        .unwrap();

        let found = &results[0].matches[0];
        assert!(found.line_content.len() <= MAX_SNIPPET_BYTES);
        assert!(
            found
                .context_before
                .iter()
                .chain(&found.context_after)
                .all(|(_, line)| line.len() <= MAX_SNIPPET_BYTES
                    && line.is_char_boundary(line.len()))
        );
        assert!(std::str::from_utf8(found.line_content.as_bytes()).is_ok());
    }

    #[test]
    fn ordinary_search_results_remain_complete() {
        let dir = TempDir::new();
        fs::write(
            dir.path.join("ordinary.txt"),
            "before\n\tneedle and needle\nafter\n",
        )
        .unwrap();
        let cancelled = AtomicBool::new(false);
        let config = ContentSearchConfig {
            case_sensitive: true,
            context_lines: 1,
            ..ContentSearchConfig::default()
        };
        let mut results = Vec::new();

        search_content(&dir.path, "needle", &config, &cancelled, &mut |result| {
            results.push(result)
        })
        .unwrap();

        let found = &results[0].matches[0];
        assert_eq!(found.line_content, "    needle and needle");
        assert_eq!(found.match_ranges, vec![4..10, 15..21]);
        assert_eq!(found.context_before, vec![(1, "before".to_string())]);
        assert_eq!(found.context_after, vec![(3, "after".to_string())]);
    }
}
