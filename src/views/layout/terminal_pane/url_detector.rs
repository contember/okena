//! URL detection for terminal content.
//!
//! Pure logic component - no UI, no Entity.

use crate::elements::terminal_element::URLMatch;
use crate::terminal::terminal::Terminal;
use std::sync::Arc;

/// URL detector for finding and tracking URLs in terminal content.
pub struct UrlDetector {
    /// Detected URL matches
    matches: Vec<URLMatch>,
    /// Currently hovered URL index
    hovered_index: Option<usize>,
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
            hovered_index: None,
        }
    }

    /// Update URL matches from terminal content.
    pub fn update_matches(&mut self, terminal: &Option<Arc<Terminal>>) {
        if let Some(ref terminal) = terminal {
            let detected = terminal.detect_urls();
            self.matches = detected
                .into_iter()
                .map(|(line, col, len, url)| URLMatch { line, col, len, url })
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
        let new_hovered = self
            .matches
            .iter()
            .position(|url| url.line == row && col >= url.col && col < url.col + url.len);

        if new_hovered != self.hovered_index {
            self.hovered_index = new_hovered;
            true
        } else {
            false
        }
    }

    /// Clear hover state. Returns true if state changed.
    pub fn clear_hover(&mut self) -> bool {
        if self.hovered_index.is_some() {
            self.hovered_index = None;
            true
        } else {
            false
        }
    }

    /// Get the currently hovered URL index.
    pub fn hovered_index(&self) -> Option<usize> {
        self.hovered_index
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
            let _ = std::process::Command::new("xdg-open").arg(url).spawn();
        }
        #[cfg(target_os = "macos")]
        {
            let _ = std::process::Command::new("open").arg(url).spawn();
        }
        #[cfg(target_os = "windows")]
        {
            let _ = std::process::Command::new("cmd")
                .args(["/C", "start", "", url])
                .spawn();
        }
    }
}
