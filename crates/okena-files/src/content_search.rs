//! File content search engine using grep-searcher + ignore.
//!
//! Provides async file content search with streaming results,
//! supporting literal, regex, and fuzzy matching modes.

use grep_matcher::Matcher;
use grep_regex::RegexMatcherBuilder;
use grep_searcher::sinks::UTF8;
use grep_searcher::Searcher;
use ignore::{WalkBuilder, WalkState};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// Skip files larger than this for content search. Lockfiles, bundles, and
/// generated code are typically uninteresting and dominate I/O time.
const MAX_FILE_SIZE: u64 = 1_000_000;

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
    let mut expanded = String::with_capacity(text.len());
    // Map from original byte offset to expanded byte offset
    let mut offset_map: Vec<usize> = Vec::with_capacity(text.len() + 1);
    let mut expanded_pos: usize = 0;

    for (orig_pos, ch) in text.char_indices() {
        offset_map.resize(orig_pos + 1, expanded_pos);
        if ch == '\t' {
            expanded.push_str("    ");
            expanded_pos += 4;
        } else {
            expanded.push(ch);
            expanded_pos += ch.len_utf8();
        }
    }
    // Sentinel for end-of-string
    offset_map.resize(text.len() + 1, expanded_pos);

    let new_ranges = ranges
        .iter()
        .filter_map(|r| {
            let start = *offset_map.get(r.start)?;
            let end = *offset_map.get(r.end)?;
            Some(start..end)
        })
        .collect();

    (expanded, new_ranges)
}

/// Expand tabs to 4 spaces in a string (no range remapping needed).
fn expand_tabs_simple(text: &str) -> String {
    if text.contains('\t') {
        text.replace('\t', "    ")
    } else {
        text.to_string()
    }
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
fn configure_walker(project_path: &Path, config: &ContentSearchConfig) -> WalkBuilder {
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
        let _ = override_builder.add(pattern);
    }
    if let Some(ref glob) = config.file_glob {
        let _ = override_builder.add(glob);
    }
    if let Ok(overrides) = override_builder.build() {
        walk_builder.overrides(overrides);
    }

    walk_builder
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
            m.context_before.push((
                i + 1,
                expand_tabs_simple(all_lines.get(i).unwrap_or(&"")),
            ));
        }

        // Context after
        let end = (line_idx + 1 + context_lines).min(all_lines.len());
        for i in (line_idx + 1)..end {
            m.context_after.push((
                i + 1,
                expand_tabs_simple(all_lines.get(i).unwrap_or(&"")),
            ));
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
) {
    if query.is_empty() {
        return;
    }

    match config.mode {
        SearchMode::Fuzzy => search_content_fuzzy(project_path, query, config, cancelled, on_result),
        _ => search_content_grep(project_path, query, config, cancelled, on_result),
    }
}

/// Search using grep-searcher (literal or regex mode).
///
/// Walks the project tree in parallel. Each worker thread keeps its own
/// `Searcher` (it's stateful) and a clone of the matcher; results are funneled
/// through a `Mutex` around the caller's callback.
fn search_content_grep(
    project_path: &Path,
    query: &str,
    config: &ContentSearchConfig,
    cancelled: &AtomicBool,
    on_result: &mut (dyn FnMut(FileSearchResult) + Send),
) {
    let matcher = {
        let mut builder = RegexMatcherBuilder::new();
        builder.case_insensitive(!config.case_sensitive);

        let pattern = if config.mode == SearchMode::Regex {
            query.to_string()
        } else {
            escape_regex(query)
        };
        match builder.build(&pattern) {
            Ok(m) => m,
            Err(_) => return,
        }
    };

    let total_matches = AtomicUsize::new(0);
    let max_results = config.max_results;
    let context_lines = config.context_lines;
    let on_result = Mutex::new(on_result);

    configure_walker(project_path, config).build_parallel().run(|| {
        let matcher = matcher.clone();
        let mut searcher = Searcher::new();
        let total_matches = &total_matches;
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
                    matcher.find_iter(line_content.as_bytes(), |m| {
                        let start = m.start();
                        let end = m.end().min(line_trimmed.len());
                        if start < line_trimmed.len() {
                            match_ranges.push(start..end);
                        }
                        true
                    }).ok();

                    // Expand tabs to match syntax highlighter output
                    let (line_expanded, match_ranges) = expand_tabs(line_trimmed, &match_ranges);

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

            total_matches.fetch_add(file_matches.len(), Ordering::Relaxed);

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

            if let Ok(mut cb) = on_result.lock() {
                cb(result);
            }

            WalkState::Continue
        })
    });
}

/// Search using nucleo-matcher (fuzzy mode).
///
/// Walks the project tree in parallel; each worker thread keeps its own
/// `Matcher` (it's stateful) and reads file contents independently.
fn search_content_fuzzy(
    project_path: &Path,
    query: &str,
    config: &ContentSearchConfig,
    cancelled: &AtomicBool,
    on_result: &mut (dyn FnMut(FileSearchResult) + Send),
) {
    use nucleo_matcher::{Config as NucleoConfig, Matcher, Utf32Str};

    let total_matches = AtomicUsize::new(0);
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

    configure_walker(project_path, config).build_parallel().run(|| {
        let mut matcher = Matcher::new(NucleoConfig::DEFAULT);
        let total_matches = &total_matches;
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
                        .filter_map(|&idx| {
                            let (byte_pos, ch) = char_to_byte.get(idx as usize)?;
                            Some(*byte_pos..*byte_pos + ch.len_utf8())
                        })
                        .collect();

                    // Expand tabs to match syntax highlighter output
                    let (line_expanded, match_ranges) = expand_tabs(line, &match_ranges);

                    scored_matches.push((score, ContentMatch {
                        line_number: line_idx + 1,
                        line_content: line_expanded,
                        match_ranges,
                        context_before: Vec::new(),
                        context_after: Vec::new(),
                    }));
                }
            }

            if scored_matches.is_empty() {
                return WalkState::Continue;
            }

            // Sort by score descending — best matches first
            scored_matches.sort_by(|a, b| b.0.cmp(&a.0));

            let best_score = scored_matches.first().map(|(s, _)| *s).unwrap_or(0);
            let mut file_matches: Vec<ContentMatch> = scored_matches
                .into_iter()
                .map(|(_, m)| m)
                .collect();

            total_matches.fetch_add(file_matches.len(), Ordering::Relaxed);

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

            if let Ok(mut cb) = on_result.lock() {
                cb(result);
            }

            WalkState::Continue
        })
    });
}

/// Handle for cancelling a running search.
#[derive(Clone)]
pub struct SearchHandle {
    cancelled: Arc<AtomicBool>,
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
            '\\' | '.' | '+' | '*' | '?' | '(' | ')' | '|' | '[' | ']' | '{' | '}' | '^'
            | '$' => {
                escaped.push('\\');
                escaped.push(c);
            }
            _ => escaped.push(c),
        }
    }
    escaped
}
