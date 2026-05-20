//! File loading and syntax highlighting for the file viewer.

use super::{FileViewerTab, MAX_FILE_SIZE, MAX_LINES};
use crate::syntax::{highlight_content, HighlightedLine};
use okena_markdown::MarkdownDocument;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use syntect::parsing::SyntaxSet;

/// Result of a background freshness reload: the file changed on disk and was
/// re-read and re-highlighted off the UI thread. All the heavy work (stat,
/// read, syntax highlighting, markdown parse) has already happened; applying
/// this back on the UI thread is just a set of field assignments.
pub(super) struct FreshnessReload {
    pub content: String,
    pub highlighted_lines: Vec<HighlightedLine>,
    pub markdown_doc: Option<MarkdownDocument>,
    pub modified_at: Option<SystemTime>,
}

/// Stat `path` and, if its mtime differs from `old_mtime`, read and
/// re-highlight it. Returns `Ok(None)` when the file is unchanged (or can't be
/// stat'd), `Ok(Some(..))` with the recomputed content when it changed, and
/// `Err` when the file changed but could not be read.
///
/// Pure / blocking — meant to run on the background executor, so it captures no
/// GPUI handles and touches no entity state.
pub(super) fn compute_freshness_reload(
    path: &PathBuf,
    old_mtime: Option<SystemTime>,
    is_markdown: bool,
    syntax_set: &SyntaxSet,
    is_dark: bool,
) -> Result<Option<FreshnessReload>, String> {
    let Some(old_mtime) = old_mtime else {
        return Ok(None);
    };
    let Ok(metadata) = std::fs::metadata(path) else {
        return Ok(None);
    };
    let Ok(new_mtime) = metadata.modified() else {
        return Ok(None);
    };
    if new_mtime == old_mtime {
        return Ok(None);
    }
    if metadata.len() > MAX_FILE_SIZE {
        return Err(format!(
            "File too large ({:.1} MB). Maximum size is 5 MB.",
            metadata.len() as f64 / 1024.0 / 1024.0
        ));
    }
    let content = std::fs::read_to_string(path).map_err(|e| {
        // Distinguish binary files from other read errors, matching load_file.
        if let Ok(bytes) = std::fs::read(path)
            && bytes.iter().take(1024).any(|&b| b == 0) {
                return "Cannot display binary file".to_string();
            }
        format!("Cannot read file: {}", e)
    })?;
    let highlighted_lines = highlight_content(&content, path, syntax_set, MAX_LINES, is_dark);
    let markdown_doc = if is_markdown {
        Some(MarkdownDocument::parse(&content))
    } else {
        None
    };
    Ok(Some(FreshnessReload {
        content,
        highlighted_lines,
        markdown_doc,
        modified_at: Some(new_mtime),
    }))
}

impl FileViewerTab {
    /// Check if a file is a markdown file based on extension.
    pub(super) fn is_markdown_file(path: &Path) -> bool {
        path.extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| {
                let ext_lower = ext.to_lowercase();
                ext_lower == "md" || ext_lower == "markdown"
            })
            .unwrap_or(false)
    }

    /// Load file content and apply syntax highlighting.
    pub(super) fn load_file(&mut self, path: &PathBuf, syntax_set: &SyntaxSet, is_dark: bool) {
        // Check file size first
        match std::fs::metadata(path) {
            Ok(metadata) => {
                if metadata.len() > MAX_FILE_SIZE {
                    self.error_message = Some(format!(
                        "File too large ({:.1} MB). Maximum size is 5 MB.",
                        metadata.len() as f64 / 1024.0 / 1024.0
                    ));
                    return;
                }
                self.modified_at = metadata.modified().ok();
            }
            Err(e) => {
                self.error_message = Some(format!("Cannot read file: {}", e));
                return;
            }
        }

        // Read file content
        match std::fs::read_to_string(path) {
            Ok(content) => {
                self.content = content;
                self.do_highlight_content(path, syntax_set, is_dark);
                // Parse markdown if this is a markdown file
                if self.is_markdown {
                    self.markdown_doc = Some(MarkdownDocument::parse(&self.content));
                }
            }
            Err(e) => {
                // Try reading as binary and check if it's a binary file
                match std::fs::read(path) {
                    Ok(bytes) => {
                        if bytes.iter().take(1024).any(|&b| b == 0) {
                            self.error_message = Some("Cannot display binary file".to_string());
                        } else {
                            self.error_message = Some(format!("Cannot read file: {}", e));
                        }
                    }
                    Err(_) => {
                        self.error_message = Some(format!("Cannot read file: {}", e));
                    }
                }
            }
        }
    }

    /// Apply content that was loaded asynchronously in the background.
    pub(super) fn apply_loaded_content(
        &mut self,
        result: Result<String, String>,
        syntax_set: &SyntaxSet,
        is_dark: bool,
    ) {
        self.loading = false;
        match result {
            Ok(content) => {
                self.content = content;
                self.do_highlight_content(&self.file_path.clone(), syntax_set, is_dark);
                if self.is_markdown {
                    self.markdown_doc = Some(MarkdownDocument::parse(&self.content));
                }
                // Try to get mtime for local files; harmlessly fails for remote files.
                self.modified_at = std::fs::metadata(&self.file_path)
                    .ok()
                    .and_then(|m| m.modified().ok());
            }
            Err(e) => {
                self.error_message = Some(e);
            }
        }
    }

    /// Apply the result of a background freshness reload computed by
    /// `compute_freshness_reload`. All heavy work (stat/read/highlight) already
    /// happened off-thread; this is just field assignment on the UI thread.
    pub(super) fn apply_freshness_reload(&mut self, reload: Result<Option<FreshnessReload>, String>) {
        match reload {
            Ok(Some(reload)) => {
                self.error_message = None;
                self.content = reload.content;
                self.line_count = reload.highlighted_lines.len();
                self.line_num_width = self.line_count.to_string().len().max(3);
                self.highlighted_lines = reload.highlighted_lines;
                self.markdown_doc = reload.markdown_doc;
                self.modified_at = reload.modified_at;
            }
            Ok(None) => {}
            Err(e) => {
                self.error_message = Some(e);
            }
        }
    }

    /// Check if the file was modified externally and reload if so.
    /// Returns true if the file was reloaded.
    pub(super) fn reload_if_changed(
        &mut self,
        syntax_set: &SyntaxSet,
        is_dark: bool,
    ) -> bool {
        let Some(old_mtime) = self.modified_at else {
            return false;
        };
        let Ok(metadata) = std::fs::metadata(&self.file_path) else {
            return false;
        };
        let Ok(new_mtime) = metadata.modified() else {
            return false;
        };
        if new_mtime == old_mtime {
            return false;
        }
        let path = self.file_path.clone();
        self.error_message = None;
        self.load_file(&path, syntax_set, is_dark);
        true
    }

    /// Apply syntax highlighting to the content using shared utilities.
    pub(super) fn do_highlight_content(
        &mut self,
        path: &Path,
        syntax_set: &SyntaxSet,
        is_dark: bool,
    ) {
        self.highlighted_lines =
            highlight_content(&self.content, path, syntax_set, MAX_LINES, is_dark);
        self.line_count = self.highlighted_lines.len();
        self.line_num_width = self.line_count.to_string().len().max(3);
    }
}
