//! Search execution, result aggregation, and highlight-cache management.

use super::{ContentSearchDialog, ResultRow};
use crate::content_search::{ContentSearchConfig, FileSearchResult, SearchHandle, SearchMode};
use crate::syntax::{HighlightedLine, highlight_content};
use gpui::*;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

impl ContentSearchDialog {
    /// Trigger a debounced search.
    pub(super) fn trigger_search(&mut self, cx: &mut Context<Self>) {
        // Cancel any running search
        if let Some(handle) = self.search_handle.take() {
            handle.cancel();
        }

        let query = self.search_input.read(cx).value().to_string();
        if query.is_empty() {
            self.rows.clear();
            self.total_matches = 0;
            self.searching = false;
            self.selected_index = 0;
            cx.notify();
            return;
        }

        // Debounce: wait 200ms before starting search
        self.debounce_task = Some(cx.spawn(async move |this: WeakEntity<ContentSearchDialog>, cx| {
            cx.background_executor().timer(std::time::Duration::from_millis(200)).await;
            this.update(cx, |this, cx| {
                this.run_search(cx);
            }).ok();
        }));
    }

    /// Actually run the search on a background thread.
    fn run_search(&mut self, cx: &mut Context<Self>) {
        let query = self.search_input.read(cx).value().to_string();
        if query.is_empty() {
            return;
        }

        let handle = SearchHandle::new();
        self.search_handle = Some(handle.clone());
        self.searching = true;
        self.highlight_cache.clear();
        cx.notify();

        let mode = if self.fuzzy_mode {
            SearchMode::Fuzzy
        } else if self.regex_mode {
            SearchMode::Regex
        } else {
            SearchMode::Literal
        };

        let config = ContentSearchConfig {
            case_sensitive: self.case_sensitive,
            mode,
            max_results: 1000,
            file_glob: self.file_glob.clone(),
            context_lines: 0,
            show_ignored: self.show_ignored,
        };

        let project_fs = self.project_fs.clone();
        let cancelled = handle.flag();

        cx.spawn(async move |entity: WeakEntity<ContentSearchDialog>, cx| {
            let results = cx
                .background_executor()
                .spawn(async move {
                    let mut results: Vec<FileSearchResult> = Vec::new();
                    project_fs.search_content(
                        &query,
                        &config,
                        &cancelled,
                        &mut |result| {
                            results.push(result);
                        },
                    );
                    // Sort files by best match score (highest first) for fuzzy mode,
                    // breaking ties by relative_path for stable order across runs
                    // (parallel walker emits in non-deterministic order).
                    results.sort_by(|a, b| {
                        b.best_score
                            .cmp(&a.best_score)
                            .then_with(|| a.relative_path.cmp(&b.relative_path))
                    });
                    results
                })
                .await;

            entity
                .update(cx, |this, cx| {
                    if this
                        .search_handle
                        .as_ref()
                        .is_some_and(|h| !h.is_cancelled())
                    {
                        this.apply_results(results);
                        this.searching = false;
                        // Preload highlighting for files in search results
                        this.preload_result_files(cx);
                        cx.notify();
                    }
                })
                .ok();
        })
        .detach();
    }

    /// Convert search results into flattened display rows.
    fn apply_results(&mut self, results: Vec<FileSearchResult>) {
        self.rows.clear();
        self.total_matches = 0;

        for file_result in &results {
            self.rows.push(ResultRow::FileHeader {
                file_path: file_result.file_path.clone(),
                relative_path: file_result.relative_path.clone(),
                match_count: file_result.matches.len(),
            });

            for m in &file_result.matches {
                self.total_matches += 1;
                self.rows.push(ResultRow::Match {
                    file_path: file_result.file_path.clone(),
                    relative_path: file_result.relative_path.clone(),
                    line_number: m.line_number,
                    line_content: m.line_content.clone(),
                    match_ranges: m.match_ranges.clone(),
                    _context_before: m.context_before.clone(),
                    _context_after: m.context_after.clone(),
                });
            }
        }

        self.selected_index = if self.rows.is_empty() { 0 } else { 1.min(self.rows.len() - 1) };
    }

    /// Get syntax-highlighted line for a file. Returns None if not yet cached
    /// (caller falls back to plain text rendering).
    pub(super) fn get_highlighted_line(
        &self,
        file_path: &Path,
        line_number: usize,
    ) -> Option<HighlightedLine> {
        let lines = self.highlight_cache.get(file_path)?;
        // line_number is 1-based
        lines.get(line_number.saturating_sub(1)).cloned()
    }

    /// Preload highlighting for the first few unique files in search results.
    fn preload_result_files(&mut self, cx: &mut Context<Self>) {
        let mut seen = HashSet::new();
        let paths: Vec<PathBuf> = self
            .rows
            .iter()
            .filter_map(|row| match row {
                ResultRow::FileHeader { file_path, .. } if seen.insert(file_path.clone()) => {
                    Some(file_path.clone())
                }
                _ => None,
            })
            .take(5)
            .collect();
        for fp in paths {
            self.ensure_file_in_cache(&fp, cx);
        }
    }

    /// Kick off an async load of a file into the highlight cache (if not already loading).
    pub(super) fn ensure_file_in_cache(&mut self, file_path: &Path, cx: &mut Context<Self>) {
        let key = file_path.to_path_buf();
        if self.highlight_cache.contains_key(&key) || self.loading_files.contains(&key) {
            return;
        }
        self.loading_files.insert(key.clone());
        let fs = self.project_fs.clone();
        let fp = key;
        let fp_str = file_path.to_string_lossy().to_string();
        cx.spawn(async move |entity: WeakEntity<Self>, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { fs.read_file(&fp_str) })
                .await;
            let _ = entity.update(cx, |this, cx| {
                this.loading_files.remove(&fp);
                if let Ok(content) = result {
                    let lines = highlight_content(
                        &content,
                        &fp,
                        &this.syntax_set,
                        5000,
                        this.is_dark,
                    );
                    this.highlight_cache.insert(fp, lines);
                }
                cx.notify();
            });
        })
        .detach();
    }
}
