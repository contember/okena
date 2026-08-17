//! Async loading and processing for the diff viewer: fetching the diff,
//! syntax-highlighting the selected file, and (re-)building the file tree.

use super::DiffViewer;
use super::review::{
    LoadState, ReviewEpoch, ReviewFileKey, adjacent_inventory_file, derived_requests, is_smart_mode,
    theme_requires_rehighlight,
};
use super::review_ui::ContentView;
use super::syntax::process_file;
use super::types::{DiffDisplayFile, DisplayItem, FileStats, FileTreeNode};

use okena_core::review::{ImmutableResolvedComparison, ReviewSourceRequest};
use okena_files::file_tree::build_file_tree;
use okena_git::{DiffMode, DiffResult};

use gpui::*;
use std::collections::HashSet;

impl DiffViewer {
    pub(super) fn load_diff_async(
        &mut self,
        mode: DiffMode,
        select_file: Option<String>,
        cx: &mut Context<Self>,
    ) {
        self.review_navigation.invalidate();
        if is_smart_mode(&mode) {
            self.load_smart_review_async(mode, select_file, cx);
        } else {
            self.load_legacy_diff_async(mode, select_file, cx);
        }
    }

    fn reset_diff_display(&mut self, mode: DiffMode) {
        self.diff_mode = mode.clone();
        self.loading = true;
        self.error_message = None;
        self.file_error_active = false;
        self.raw_files.clear();
        self.file_stats.clear();
        self.current_file = None;
        self.current_file_old_content = None;
        self.current_file_new_content = None;
        self.file_tree = FileTreeNode::default();
        self.selected_file_index = 0;
        self.selection.clear();
        self.selection_side = None;
        self.side_by_side_lines.clear();
        self.scroll_x = 0.0;
        self.max_line_chars = 0;
    }

    fn load_legacy_diff_async(
        &mut self,
        mode: DiffMode,
        select_file: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let epoch = self.smart_review.disable();
        self.reset_diff_display(mode.clone());
        cx.notify();

        let provider = self.provider.clone();
        let ignore_whitespace = self.ignore_whitespace;

        cx.spawn(async move |this, cx| {
            let mode_for_fallback = mode.clone();
            let result = smol::unblock(move || provider.get_diff(mode, ignore_whitespace)).await;

            let _ = this.update(cx, |this, cx| {
                if !this.smart_review.is_current(epoch) || this.diff_mode != mode_for_fallback {
                    return;
                }
                this.loading = false;
                match result {
                    Ok(diff_result) => {
                        if diff_result.is_empty() {
                            // Auto-fallback: if WorkingTree is empty, try Staged
                            if mode_for_fallback == DiffMode::WorkingTree {
                                this.load_diff_async(DiffMode::Staged, select_file, cx);
                                return;
                            }
                            this.error_message = Some(format!(
                                "No {} changes",
                                mode_for_fallback.display_name().to_lowercase()
                            ));
                        } else {
                            this.store_diff_result(diff_result);
                            this.build_file_tree();

                            // Select specific file if requested
                            if let Some(ref file_path) = select_file
                                && let Some(index) =
                                    this.file_stats.iter().position(|f| f.path == *file_path)
                            {
                                this.selected_file_index = index;
                            }

                            this.process_current_file_async(cx);
                        }
                    }
                    Err(e) => {
                        this.error_message = Some(e);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn load_smart_review_async(
        &mut self,
        mode: DiffMode,
        select_file: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let explicit_selection = select_file.is_some();
        let selected = select_file.or_else(|| {
            self.file_stats
                .get(self.selected_file_index)
                .map(|file| file.path.clone())
        });
        if let Some((epoch, comparison)) = self.smart_review.begin_derived_reload(&mode) {
            self.reset_diff_display(mode.clone());
            self.start_smart_derived(epoch, mode, comparison, selected, cx);
            cx.notify();
            return;
        }

        let epoch = self.smart_review.begin(mode.clone());
        self.review_reset_for_comparison();
        if explicit_selection {
            // The caller already chose the file, so the small-change auto-open must not.
            self.review_ui.content = ContentView::File;
            self.review_ui.small_change_applied = true;
        }
        self.reset_diff_display(mode.clone());
        cx.notify();
        let provider = self.provider.clone();
        cx.spawn(async move |this, cx| {
            let result = provider.get_review_inventory(mode.clone()).await;
            let _ = this.update(cx, |this, cx| {
                if !this.smart_review.accepts(epoch, &mode) {
                    return;
                }
                match result {
                    Ok(inventory) => {
                        if inventory.comparison.requested() != &mode {
                            this.smart_review.inventory = LoadState::Failed(
                                "Review inventory returned a different requested comparison"
                                    .to_string(),
                            );
                            this.loading = false;
                            this.smart_review.changed();
                            this.review_rebuild_model(cx);
                            this.error_message = Some(
                                "Review inventory returned a different comparison".to_string(),
                            );
                            cx.notify();
                            return;
                        }
                        let comparison = match ImmutableResolvedComparison::try_from(
                            inventory.comparison.clone(),
                        ) {
                            Ok(comparison) => comparison,
                            Err(error) => {
                                let message = format!(
                                    "Review inventory comparison is not immutable: {error}"
                                );
                                this.smart_review.inventory = LoadState::Failed(message.clone());
                                this.loading = false;
                                this.smart_review.changed();
                                this.review_rebuild_model(cx);
                                this.error_message = Some(message);
                                cx.notify();
                                return;
                            }
                        };
                        this.smart_review
                            .set_inventory(inventory, selected.as_deref());
                        this.smart_review.diff = LoadState::Loading;
                        this.smart_review.structure = LoadState::Loading;
                        this.smart_review.changed();
                        this.review_rebuild_model(cx);
                        this.start_smart_derived(epoch, mode, comparison, selected, cx);
                    }
                    Err(error) => {
                        this.smart_review.inventory = LoadState::Failed(error.clone());
                        this.loading = false;
                        this.smart_review.changed();
                        this.review_rebuild_model(cx);
                        this.error_message = Some(error);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn start_smart_derived(
        &mut self,
        epoch: ReviewEpoch,
        mode: DiffMode,
        comparison: ImmutableResolvedComparison,
        _select_file: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let (diff_request, structure_request) =
            derived_requests(&comparison, self.ignore_whitespace);

        let diff_provider = self.provider.clone();
        let diff_mode = mode.clone();
        let expected_diff_comparison = comparison.clone();
        cx.spawn(async move |this, cx| {
            let result = diff_provider.get_review_diff(diff_request).await;
            let _ = this.update(cx, |this, cx| {
                match this.smart_review.accept_diff(
                    epoch,
                    &diff_mode,
                    &expected_diff_comparison,
                    result,
                ) {
                    Some(Ok(files)) => {
                        let diff = DiffResult { files };
                        this.loading = false;
                        if !diff.is_empty() {
                            this.store_diff_result(diff);
                            this.build_file_tree();
                            if !this.review_navigation.has_pending() {
                                this.reconcile_smart_selection(cx);
                            }
                        } else if !this.review_navigation.has_pending() {
                            this.smart_review.file.clear();
                        }
                    }
                    Some(Err(error)) => {
                        this.loading = false;
                        debug_assert_eq!(this.smart_review.diff.error(), Some(error.as_str()));
                    }
                    None => return,
                }
                this.review_rebuild_model(cx);
                this.resume_review_navigation(cx);
                cx.notify();
            });
        })
        .detach();

        let structure_provider = self.provider.clone();
        let structure_mode = mode;
        let expected_structure_comparison = comparison;
        cx.spawn(async move |this, cx| {
            let result = structure_provider
                .get_review_structure(structure_request)
                .await;
            let _ = this.update(cx, |this, cx| {
                if this
                    .smart_review
                    .accept_structure(
                        epoch,
                        &structure_mode,
                        &expected_structure_comparison,
                        result,
                    )
                    .is_some()
                {
                    this.review_rebuild_model(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn select_smart_file(&mut self, key: ReviewFileKey, cx: &mut Context<Self>) {
        // Choosing a file always lands in the file view — spec §2.
        self.review_ui.content = ContentView::File;
        self.review_navigation.invalidate();
        self.smart_review.set_selected_file(key);
        self.reconcile_smart_selection(cx);
        cx.notify();
    }

    pub(super) fn select_adjacent_smart_file(&mut self, forward: bool, cx: &mut Context<Self>) {
        let Some(inventory) = self.smart_review.inventory.ready() else {
            return;
        };
        let Some(key) =
            adjacent_inventory_file(inventory, self.smart_review.selected_file.as_ref(), forward)
        else {
            return;
        };
        self.select_smart_file(key, cx);
    }

    pub(super) fn reconcile_smart_selection(&mut self, cx: &mut Context<Self>) {
        let Some(key) = self.smart_review.selected_file.clone() else {
            self.smart_review.file.clear();
            return;
        };
        let Some(index) = self
            .file_stats
            .iter()
            .position(|file| file.old_path == key.old_path && file.new_path == key.new_path)
        else {
            self.smart_review.file.clear();
            return;
        };
        self.selected_file_index = index;
        self.process_current_file_async(cx);
    }

    /// Store raw diff data and extract lightweight stats (no syntax highlighting).
    fn store_diff_result(&mut self, result: DiffResult) {
        let mut files = result.files;
        files.sort_by(|a, b| a.display_name().cmp(b.display_name()));
        for file in files {
            self.file_stats.push(FileStats::from(&file));
            self.raw_files.push(file);
        }
    }

    /// Process the currently selected file with syntax highlighting (async).
    pub(super) fn process_current_file_async(&mut self, cx: &mut Context<Self>) {
        self.process_current_file_for_generation(None, cx);
    }

    pub(super) fn process_current_file_for_generation(
        &mut self,
        requested: Option<(super::review::FileGeneration, ReviewFileKey)>,
        cx: &mut Context<Self>,
    ) {
        self.current_file = None;
        self.current_file_old_content = None;
        self.current_file_new_content = None;
        if self.file_error_active {
            self.error_message = None;
            self.file_error_active = false;
        }
        let Some(raw_file) = self.raw_files.get(self.selected_file_index).cloned() else {
            self.smart_review.file.clear();
            return;
        };

        let key = ReviewFileKey::from_diff(&raw_file);
        let generation = match requested {
            Some((generation, requested_key))
                if requested_key == key
                    && self.smart_review.file.accepts(generation, &requested_key) =>
            {
                generation
            }
            Some(_) | None => self.smart_review.file.begin(key.clone()),
        };
        let provider = self.provider.clone();
        let syntax_set = self.syntax_set.clone();
        let is_dark = self.is_dark;

        if is_smart_mode(&self.diff_mode) {
            let Some(comparison) = self.smart_review.comparison() else {
                self.smart_review.file.source =
                    LoadState::Failed("Exact review comparison is not ready".to_string());
                self.resume_review_navigation(cx);
                return;
            };
            let request = match ReviewSourceRequest::new(
                comparison.as_resolved().clone(),
                key.old_path.clone(),
                key.new_path.clone(),
            ) {
                Ok(request) => request,
                Err(error) => {
                    self.smart_review.file.source = LoadState::Failed(error.to_string());
                    self.resume_review_navigation(cx);
                    return;
                }
            };
            cx.spawn(async move |this, cx| {
                let result = provider.get_review_source(request).await;
                let result = match result {
                    Ok(source)
                        if source.comparison() == &comparison && key.matches_source(&source) =>
                    {
                        let old_content = source.old_content().map(str::to_owned);
                        let new_content = source.new_content().map(str::to_owned);
                        let processing_old = old_content.clone();
                        let processing_new = new_content.clone();
                        let processed = smol::unblock(move || {
                            let mut max_line_num = 0usize;
                            let display_file = process_file(
                                &raw_file,
                                &mut max_line_num,
                                &syntax_set,
                                processing_old,
                                processing_new,
                                is_dark,
                            );
                            (display_file, max_line_num)
                        })
                        .await;
                        Ok((source, old_content, new_content, processed.0, processed.1))
                    }
                    Ok(_) => Err("Exact review source did not match the selected file".to_string()),
                    Err(error) => Err(error),
                };

                let _ = this.update(cx, |this, cx| {
                    if !this.smart_review.file.accepts(generation, &key) {
                        return;
                    }
                    match result {
                        Ok((source, old_content, new_content, display_file, max_line_num)) => {
                            this.smart_review.file.source = LoadState::Ready(source);
                            this.smart_review.file.mark_cache_ready(generation, &key);
                            if this.file_error_active {
                                this.error_message = None;
                                this.file_error_active = false;
                            }
                            if theme_requires_rehighlight(is_dark, this.is_dark) {
                                this.current_file_old_content = old_content;
                                this.current_file_new_content = new_content;
                                this.rehighlight_current_file();
                                this.update_side_by_side_cache();
                            } else {
                                this.install_processed_file(
                                    old_content,
                                    new_content,
                                    display_file,
                                    max_line_num,
                                );
                            }
                        }
                        Err(error) => {
                            this.smart_review.file.source = LoadState::Failed(error);
                            this.current_file = None;
                        }
                    }
                    this.resume_review_navigation(cx);
                    cx.notify();
                });
            })
            .detach();
            return;
        }

        let file_path = raw_file.display_name().to_string();
        let diff_mode = self.diff_mode.clone();

        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || {
                let (old_content, new_content) =
                    provider.get_file_contents(&file_path, diff_mode)?;
                let mut max_line_num = 0usize;
                let display_file = process_file(
                    &raw_file,
                    &mut max_line_num,
                    &syntax_set,
                    old_content.clone(),
                    new_content.clone(),
                    is_dark,
                );
                Ok::<_, String>((old_content, new_content, display_file, max_line_num))
            })
            .await;

            let _ = this.update(cx, |this, cx| {
                if !this.smart_review.file.accepts(generation, &key) {
                    return;
                }
                match result {
                    Ok((old_content, new_content, display_file, max_line_num)) => {
                        this.smart_review.file.source = LoadState::Idle;
                        this.smart_review.file.mark_cache_ready(generation, &key);
                        if theme_requires_rehighlight(is_dark, this.is_dark) {
                            this.current_file_old_content = old_content;
                            this.current_file_new_content = new_content;
                            this.rehighlight_current_file();
                            this.update_side_by_side_cache();
                        } else {
                            this.install_processed_file(
                                old_content,
                                new_content,
                                display_file,
                                max_line_num,
                            );
                        }
                    }
                    Err(error) => {
                        this.current_file = None;
                        this.error_message = Some(error);
                        this.file_error_active = true;
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn install_processed_file(
        &mut self,
        old_content: Option<String>,
        new_content: Option<String>,
        display_file: DiffDisplayFile,
        max_line_num: usize,
    ) {
        self.current_file_old_content = old_content;
        self.current_file_new_content = new_content;
        self.line_num_width = max_line_num.to_string().len().max(3);
        self.max_line_chars = Self::calc_max_line_chars(&display_file);
        self.current_file = Some(display_file);
        self.update_side_by_side_cache();
    }

    /// Re-highlight current file using cached content (for theme changes).
    pub(super) fn rehighlight_current_file(&mut self) {
        let Some(raw_file) = self.raw_files.get(self.selected_file_index) else {
            return;
        };
        let key = ReviewFileKey::from_diff(raw_file);
        if !self
            .smart_review
            .file
            .has_ready_cache(&key, is_smart_mode(&self.diff_mode))
        {
            return;
        }

        let mut max_line_num = 0usize;
        let display_file = process_file(
            raw_file,
            &mut max_line_num,
            &self.syntax_set,
            self.current_file_old_content.clone(),
            self.current_file_new_content.clone(),
            self.is_dark,
        );

        self.line_num_width = max_line_num.to_string().len().max(3);
        self.max_line_chars = Self::calc_max_line_chars(&display_file);
        self.current_file = Some(display_file);
    }

    pub(super) fn build_file_tree(&mut self) {
        self.file_tree = build_file_tree(
            self.file_stats
                .iter()
                .enumerate()
                .map(|(i, f)| (i, &f.path)),
        );
        // Auto-expand all folders in diff view
        self.expanded_folders.clear();
        Self::collect_folder_paths(&self.file_tree, "", &mut self.expanded_folders);
    }

    fn collect_folder_paths(node: &FileTreeNode, parent: &str, out: &mut HashSet<String>) {
        for (name, child) in &node.children {
            let path = if parent.is_empty() {
                name.clone()
            } else {
                format!("{parent}/{name}")
            };
            out.insert(path.clone());
            Self::collect_folder_paths(child, &path, out);
        }
    }

    pub(super) fn calc_max_line_chars(file: &DiffDisplayFile) -> usize {
        file.items
            .iter()
            .filter_map(|item| match item {
                DisplayItem::Line(l) => Some(l.plain_text.chars().count()),
                DisplayItem::Expander(_) => None,
            })
            .max()
            .unwrap_or(0)
    }
}
