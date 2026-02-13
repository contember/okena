//! File loading and syntax highlighting for the file viewer.

use super::{FileViewer, MAX_FILE_SIZE, MAX_LINES};
use crate::views::components::highlight_content;
use super::MarkdownDocument;
use std::path::PathBuf;

impl FileViewer {
    /// Check if a file is a markdown file based on extension.
    pub(super) fn is_markdown_file(path: &PathBuf) -> bool {
        path.extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| {
                let ext_lower = ext.to_lowercase();
                ext_lower == "md" || ext_lower == "markdown"
            })
            .unwrap_or(false)
    }

    /// Load file content and apply syntax highlighting.
    pub(super) fn load_file(&mut self, path: &PathBuf) {
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
            }
            Err(e) => {
                self.error_message = Some(format!("Cannot read file: {}", e));
                return;
            }
        }

        // Read file content
        match std::fs::read_to_string(path) {
            Ok(content) => {
                self.content = content.clone();
                self.do_highlight_content(path);
                // Parse markdown if this is a markdown file
                if self.is_markdown {
                    self.markdown_doc = Some(MarkdownDocument::parse(&content));
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

    /// Apply syntax highlighting to the content using shared utilities.
    pub(super) fn do_highlight_content(&mut self, path: &PathBuf) {
        self.highlighted_lines = highlight_content(
            &self.content,
            path,
            &self.syntax_set,
            MAX_LINES,
            self.is_dark,
        );
        self.line_count = self.highlighted_lines.len();
        self.line_num_width = self.line_count.to_string().len().max(3);
    }
}
