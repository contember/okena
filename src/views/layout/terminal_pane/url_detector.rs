//! URL detection for terminal content.
//!
//! Pure logic component - no UI, no Entity.

use crate::elements::terminal_element::{LinkKind, URLMatch};
use crate::terminal::terminal::Terminal;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// URL detector for finding and tracking URLs in terminal content.
pub struct UrlDetector {
    /// Detected URL matches
    matches: Vec<URLMatch>,
    /// Currently hovered URL group
    hovered_group: Option<usize>,
    /// Cache of path existence checks to avoid repeated syscalls
    path_exists_cache: HashMap<String, bool>,
}

impl Default for UrlDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl UrlDetector {
    pub fn new() -> Self {
        Self {
            matches: Vec::new(),
            hovered_group: None,
            path_exists_cache: HashMap::new(),
        }
    }

    /// Resolve a detected path string against a working directory.
    /// Handles `~/`, `./`, `../`, and absolute paths.
    fn resolve_path(text: &str, cwd: &str) -> PathBuf {
        // Strip :line:col suffix for existence check
        let clean = strip_line_col_suffix(text);

        if clean.starts_with("~/") {
            if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
                return PathBuf::from(home).join(&clean[2..]);
            }
            PathBuf::from(clean)
        } else if clean.starts_with("./") || clean.starts_with("../") {
            Path::new(cwd).join(clean)
        } else {
            // Absolute path
            PathBuf::from(clean)
        }
    }

    /// Check if a path exists, using the cache.
    fn path_exists_cached(&mut self, text: &str, cwd: &str) -> bool {
        if let Some(&exists) = self.path_exists_cache.get(text) {
            return exists;
        }

        // Evict cache if too large
        if self.path_exists_cache.len() >= 200 {
            self.path_exists_cache.clear();
        }

        let resolved = Self::resolve_path(text, cwd);
        let exists = resolved.exists();
        self.path_exists_cache.insert(text.to_string(), exists);
        exists
    }

    /// Update URL matches from terminal content.
    pub fn update_matches(&mut self, terminal: &Option<Arc<Terminal>>) {
        if let Some(terminal) = terminal {
            let detected = terminal.detect_urls();
            let cwd = terminal.initial_cwd();

            let mut group_id = 0usize;
            let mut last_text: Option<String> = None;

            self.matches = detected
                .into_iter()
                .filter_map(|link| {
                    // Assign group: consecutive links with the same text share a group
                    let same_as_last = last_text.as_ref() == Some(&link.text);
                    if !same_as_last {
                        group_id += 1;
                    }
                    last_text = Some(link.text.clone());
                    let current_group = group_id;

                    if link.is_url {
                        Some(URLMatch {
                            line: link.line,
                            col: link.col,
                            len: link.len,
                            url: link.text,
                            kind: LinkKind::Url,
                            link_group: current_group,
                        })
                    } else {
                        // File path: verify existence before showing
                        if self.path_exists_cached(&link.text, cwd) {
                            Some(URLMatch {
                                line: link.line,
                                col: link.col,
                                len: link.len,
                                url: link.text,
                                kind: LinkKind::FilePath {
                                    line: link.file_line,
                                    col: link.file_col,
                                },
                                link_group: current_group,
                            })
                        } else {
                            None
                        }
                    }
                })
                .collect();
        }
    }

    /// Find URL at the given cell position.
    pub fn find_at(&self, col: usize, row: i32) -> Option<URLMatch> {
        self.matches
            .iter()
            .find(|url| url.line == row && col >= url.col && col < url.col + url.len)
            .cloned()
    }

    /// Update hover state based on mouse position.
    /// Returns true if the hover state changed.
    pub fn update_hover(&mut self, col: usize, row: i32) -> bool {
        let new_group = self
            .matches
            .iter()
            .find(|url| url.line == row && col >= url.col && col < url.col + url.len)
            .map(|url| url.link_group);

        if new_group != self.hovered_group {
            self.hovered_group = new_group;
            true
        } else {
            false
        }
    }

    /// Clear hover state. Returns true if state changed.
    pub fn clear_hover(&mut self) -> bool {
        if self.hovered_group.is_some() {
            self.hovered_group = None;
            true
        } else {
            false
        }
    }

    /// Get the currently hovered URL group.
    pub fn hovered_group(&self) -> Option<usize> {
        self.hovered_group
    }

    /// Get an Arc of the current matches for rendering.
    pub fn matches_arc(&self) -> Arc<Vec<URLMatch>> {
        Arc::new(self.matches.clone())
    }

    /// Open URL in default browser.
    pub fn open_url(url: &str) {
        log::info!("Opening URL: {}", url);
        #[cfg(target_os = "linux")]
        {
            let _ = crate::process::command("xdg-open").arg(url).spawn();
        }
        #[cfg(target_os = "macos")]
        {
            let _ = crate::process::command("open").arg(url).spawn();
        }
        #[cfg(target_os = "windows")]
        {
            let _ = crate::process::command("cmd")
                .args(["/C", "start", "", url])
                .spawn();
        }
    }

    /// Open a file path in the configured editor or system default.
    ///
    /// `path` is the file path (may include :line:col suffix in the display string).
    /// `file_line` and `file_col` are the parsed line/col numbers.
    /// `opener` is the editor command (e.g. "code", "cursor", "zed", "subl", "vim").
    /// If empty, falls back to the system default opener.
    pub fn open_file(path: &str, file_line: Option<u32>, file_col: Option<u32>, opener: &str) {
        // Strip any :line:col suffix from the path for the actual file path
        let clean_path = strip_line_col_suffix(path);

        // Expand ~ to the user's home directory
        let expanded: String;
        let clean_path = if clean_path.starts_with("~/") {
            if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
                expanded = format!("{}{}", home.to_string_lossy(), &clean_path[1..]);
                &expanded
            } else {
                clean_path
            }
        } else {
            clean_path
        };

        log::info!("Opening file: {} (line: {:?}, col: {:?}, opener: {:?})", clean_path, file_line, file_col, opener);

        if opener.is_empty() {
            // Use system default
            #[cfg(target_os = "linux")]
            {
                let _ = crate::process::command("xdg-open").arg(clean_path).spawn();
            }
            #[cfg(target_os = "macos")]
            {
                let _ = crate::process::command("open").arg(clean_path).spawn();
            }
            #[cfg(target_os = "windows")]
            {
                let _ = crate::process::command("cmd")
                    .args(["/C", "start", "", clean_path])
                    .spawn();
            }
            return;
        }

        // Build editor-specific arguments
        match opener {
            "code" | "cursor" => {
                // VS Code / Cursor: --goto file:line:col
                let mut args = vec!["--goto".to_string()];
                let mut loc = clean_path.to_string();
                if let Some(line) = file_line {
                    loc.push_str(&format!(":{}", line));
                    if let Some(col) = file_col {
                        loc.push_str(&format!(":{}", col));
                    }
                }
                args.push(loc);
                let _ = crate::process::command(opener).args(&args).spawn();
            }
            "zed" => {
                // Zed: file:line
                let mut loc = clean_path.to_string();
                if let Some(line) = file_line {
                    loc.push_str(&format!(":{}", line));
                    if let Some(col) = file_col {
                        loc.push_str(&format!(":{}", col));
                    }
                }
                let _ = crate::process::command("zed").arg(&loc).spawn();
            }
            "subl" | "sublime" => {
                // Sublime Text: file:line:col
                let mut loc = clean_path.to_string();
                if let Some(line) = file_line {
                    loc.push_str(&format!(":{}", line));
                    if let Some(col) = file_col {
                        loc.push_str(&format!(":{}", col));
                    }
                }
                let _ = crate::process::command("subl").arg(&loc).spawn();
            }
            "vim" | "nvim" => {
                // vim/nvim: +line file
                let mut args = Vec::new();
                if let Some(line) = file_line {
                    args.push(format!("+{}", line));
                }
                args.push(clean_path.to_string());
                let _ = crate::process::command(opener).args(&args).spawn();
            }
            _ => {
                // Generic: try editor file:line:col pattern
                let mut loc = clean_path.to_string();
                if let Some(line) = file_line {
                    loc.push_str(&format!(":{}", line));
                    if let Some(col) = file_col {
                        loc.push_str(&format!(":{}", col));
                    }
                }
                let _ = crate::process::command(opener).arg(&loc).spawn();
            }
        }
    }
}

/// Strip `:line` or `:line:col` suffix from a path string.
fn strip_line_col_suffix(path: &str) -> &str {
    if let Some(colon_pos) = path.rfind(':') {
        let after = &path[colon_pos + 1..];
        if after.chars().all(|c| c.is_ascii_digit()) && !after.is_empty() {
            let before = &path[..colon_pos];
            if let Some(colon_pos2) = before.rfind(':') {
                let after2 = &before[colon_pos2 + 1..];
                if after2.chars().all(|c| c.is_ascii_digit()) && !after2.is_empty() {
                    return &before[..colon_pos2];
                }
            }
            return before;
        }
    }
    path
}
