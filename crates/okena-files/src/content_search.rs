//! File content search engine using grep-searcher + ignore.
//!
//! Provides async file content search with streaming results,
//! supporting literal, regex, and fuzzy matching modes.

use grep_matcher::Matcher;
use grep_regex::RegexMatcherBuilder;
use grep_searcher::sinks::UTF8;
use grep_searcher::Searcher;
use ignore::WalkBuilder;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// A single search match within a file.
#[derive(Clone, Debug)]
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

/// Search results grouped by file.
#[derive(Clone, Debug)]
pub struct FileSearchResult {
    pub file_path: PathBuf,
    pub relative_path: String,
    pub matches: Vec<ContentMatch>,
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
}

impl Default for ContentSearchConfig {
    fn default() -> Self {
        Self {
            case_sensitive: false,
            mode: SearchMode::Literal,
            max_results: 1000,
            file_glob: None,
            context_lines: 0,
        }
    }
}

/// Build an ignore walker for the given project and config.
fn build_walker(project_path: &Path, config: &ContentSearchConfig) -> ignore::Walk {
    let mut walk_builder = WalkBuilder::new(project_path);
    walk_builder
        .hidden(true)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .max_depth(Some(20));

    if let Some(ref glob) = config.file_glob {
        let mut override_builder = ignore::overrides::OverrideBuilder::new(project_path);
        if override_builder.add(glob).is_ok() {
            if let Ok(g) = override_builder.build() {
                walk_builder.overrides(g);
            }
        }
    }

    walk_builder.build()
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
                all_lines.get(i).unwrap_or(&"").to_string(),
            ));
        }

        // Context after
        let end = (line_idx + 1 + context_lines).min(all_lines.len());
        for i in (line_idx + 1)..end {
            m.context_after.push((
                i + 1,
                all_lines.get(i).unwrap_or(&"").to_string(),
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
    on_result: &mut dyn FnMut(FileSearchResult),
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
fn search_content_grep(
    project_path: &Path,
    query: &str,
    config: &ContentSearchConfig,
    cancelled: &AtomicBool,
    on_result: &mut dyn FnMut(FileSearchResult),
) {
    let matcher = {
        let mut builder = RegexMatcherBuilder::new();
        builder.case_insensitive(!config.case_sensitive);

        if config.mode == SearchMode::Regex {
            match builder.build(query) {
                Ok(m) => m,
                Err(_) => return,
            }
        } else {
            match builder.build(&escape_regex(query)) {
                Ok(m) => m,
                Err(_) => return,
            }
        }
    };

    let mut total_matches: usize = 0;
    let mut searcher = Searcher::new();

    for entry in build_walker(project_path, config).flatten() {
        if cancelled.load(Ordering::Relaxed) {
            return;
        }

        if total_matches >= config.max_results {
            return;
        }

        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        let mut file_matches: Vec<ContentMatch> = Vec::new();

        let search_result = searcher.search_path(
            &matcher,
            path,
            UTF8(|line_number, line_content| {
                if cancelled.load(Ordering::Relaxed) {
                    return Ok(false);
                }
                if total_matches + file_matches.len() >= config.max_results {
                    return Ok(false);
                }

                let line_trimmed = line_content.trim_end_matches(&['\n', '\r'][..]).to_string();

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

                file_matches.push(ContentMatch {
                    line_number: line_number as usize,
                    line_content: line_trimmed,
                    match_ranges,
                    context_before: Vec::new(),
                    context_after: Vec::new(),
                });

                Ok(true)
            }),
        );

        if search_result.is_err() {
            continue;
        }

        if !file_matches.is_empty() {
            total_matches += file_matches.len();

            if config.context_lines > 0 {
                add_context_lines(&mut file_matches, path, config.context_lines);
            }

            let relative_path = path
                .strip_prefix(project_path)
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| path.to_string_lossy().to_string());

            on_result(FileSearchResult {
                file_path: path.to_path_buf(),
                relative_path,
                matches: file_matches,
            });
        }
    }
}

/// Search using nucleo-matcher (fuzzy mode).
fn search_content_fuzzy(
    project_path: &Path,
    query: &str,
    config: &ContentSearchConfig,
    cancelled: &AtomicBool,
    on_result: &mut dyn FnMut(FileSearchResult),
) {
    use nucleo_matcher::{Config as NucleoConfig, Matcher, Utf32Str};

    let mut matcher = Matcher::new(NucleoConfig::DEFAULT);
    let mut total_matches: usize = 0;

    for entry in build_walker(project_path, config).flatten() {
        if cancelled.load(Ordering::Relaxed) {
            return;
        }

        if total_matches >= config.max_results {
            return;
        }

        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let mut file_matches: Vec<ContentMatch> = Vec::new();

        for (line_idx, line) in content.lines().enumerate() {
            if cancelled.load(Ordering::Relaxed) {
                return;
            }
            if total_matches + file_matches.len() >= config.max_results {
                break;
            }

            let mut haystack_buf = Vec::new();
            let haystack = Utf32Str::new(line, &mut haystack_buf);

            let mut needle_buf2 = Vec::new();
            let needle = Utf32Str::new(query, &mut needle_buf2);

            let mut indices: Vec<u32> = Vec::new();
            if let Some(_score) = matcher.fuzzy_indices(haystack, needle, &mut indices) {
                // Convert char indices to byte ranges (each matched char is a separate range)
                let char_to_byte: Vec<(usize, char)> = line.char_indices().collect();
                let match_ranges: Vec<Range<usize>> = indices
                    .iter()
                    .filter_map(|&idx| {
                        let (byte_pos, ch) = char_to_byte.get(idx as usize)?;
                        Some(*byte_pos..*byte_pos + ch.len_utf8())
                    })
                    .collect();

                file_matches.push(ContentMatch {
                    line_number: line_idx + 1,
                    line_content: line.to_string(),
                    match_ranges,
                    context_before: Vec::new(),
                    context_after: Vec::new(),
                });
            }
        }

        if !file_matches.is_empty() {
            total_matches += file_matches.len();

            if config.context_lines > 0 {
                add_context_lines(&mut file_matches, path, config.context_lines);
            }

            let relative_path = path
                .strip_prefix(project_path)
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| path.to_string_lossy().to_string());

            on_result(FileSearchResult {
                file_path: path.to_path_buf(),
                relative_path,
                matches: file_matches,
            });
        }
    }
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
