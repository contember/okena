//! Async loading and processing for the diff viewer: fetching the diff,
//! syntax-highlighting the selected file, and (re-)building the file tree.

use super::syntax::process_file;
use super::types::{DiffDisplayFile, DisplayItem, FileStats, FileTreeNode};
use super::{BinaryDiffPreview, BinaryPreviewSide, DiffViewer};

use okena_files::file_renderer::{FileRenderer, PreparedFile};
use okena_files::file_tree::build_file_tree;
use okena_git::{DiffMode, DiffResult};

use gpui::*;
use std::collections::HashSet;
use std::path::Path;

struct PreparedBinarySide {
    path: String,
    content: Result<PreparedFile, String>,
}

enum ProcessedFile {
    Text {
        old_content: Option<String>,
        new_content: Option<String>,
        display_file: DiffDisplayFile,
        max_line_num: usize,
    },
    Binary {
        display_file: DiffDisplayFile,
        old: Option<PreparedBinarySide>,
        new: Option<PreparedBinarySide>,
    },
}

impl ProcessedFile {
    fn release(self, cx: &mut App) {
        if let Self::Binary { old, new, .. } = self {
            for side in [old, new].into_iter().flatten() {
                if let Ok(content) = side.content {
                    content.release(cx);
                }
            }
        }
    }
}

impl DiffViewer {
    pub(super) fn load_diff_async(
        &mut self,
        mode: DiffMode,
        select_file: Option<String>,
        cx: &mut Context<Self>,
    ) {
        self.request_generation = self.request_generation.wrapping_add(1);
        let request_generation = self.request_generation;
        if matches!(mode, DiffMode::WorkingTree | DiffMode::Staged) {
            self.uncommitted_mode = mode.clone();
            self.commit_message = None;
        }
        self.diff_mode = mode.clone();
        self.loading = true;
        self.error_message = None;
        self.raw_files.clear();
        self.file_stats.clear();
        self.current_file = None;
        self.release_binary_preview(cx);
        self.binary_preview_loading = false;
        self.current_file_old_content = None;
        self.current_file_new_content = None;
        self.file_tree = FileTreeNode::default();
        self.selected_file_index = 0;
        self.selection.clear();
        self.selection_side = None;
        self.side_by_side_lines.clear();
        self.scroll_x = 0.0;
        self.max_line_chars = 0;
        self.composition.clear();
        cx.notify();

        let provider = self.provider.clone();
        let ignore_whitespace = self.ignore_whitespace;

        cx.spawn(async move |this, cx| {
            let mode_for_fallback = mode.clone();
            let result = smol::unblock(move || provider.get_diff(mode, ignore_whitespace)).await;

            let _ = this.update(cx, |this, cx| {
                if this.request_generation != request_generation {
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
                            this.load_composition_async(cx);
                        }
                    }
                    Err(e) => {
                        this.error_message = Some(e);
                        this.composition.clear();
                        this.composition.loading = false;
                    }
                }
                cx.notify();
            });
        })
        .detach();
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
        self.request_generation = self.request_generation.wrapping_add(1);
        let request_generation = self.request_generation;
        let Some(raw_file) = self.raw_files.get(self.selected_file_index).cloned() else {
            self.current_file = None;
            self.release_binary_preview(cx);
            self.binary_preview_loading = false;
            self.current_file_old_content = None;
            self.current_file_new_content = None;
            return;
        };

        let provider = self.provider.clone();
        let file_path = raw_file.display_name().to_string();
        let diff_mode = self.diff_mode.clone();
        let syntax_set = self.syntax_set.clone();
        let is_dark = self.is_dark;
        let svg_renderer = cx.svg_renderer();
        self.release_binary_preview(cx);
        self.binary_preview_loading = raw_file.is_binary;
        self.error_message = None;

        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || {
                if raw_file.is_binary {
                    let supports = |path: &String| FileRenderer::supports_path(Path::new(path));
                    let old_request = raw_file.old_path.as_ref().filter(|path| supports(path));
                    let new_request = raw_file.new_path.as_ref().filter(|path| supports(path));
                    let contents = provider.get_binary_file_contents(
                        old_request.map(String::as_str),
                        new_request.map(String::as_str),
                        diff_mode,
                    )?;
                    let prepare_side = |path: Option<String>, bytes: Option<Vec<u8>>| {
                        path.map(|path| PreparedBinarySide {
                            content: if FileRenderer::supports_path(Path::new(&path)) {
                                bytes
                                    .ok_or_else(|| {
                                        "File does not exist at this revision".to_string()
                                    })
                                    .and_then(|bytes| {
                                        FileRenderer::prepare(
                                            Path::new(&path),
                                            bytes,
                                            &svg_renderer,
                                        )
                                    })
                            } else {
                                Err("No preview is available for this binary file".to_string())
                            },
                            path,
                        })
                    };
                    let mut max_line_num = 0;
                    let display_file = process_file(
                        &raw_file,
                        &mut max_line_num,
                        &syntax_set,
                        None,
                        None,
                        is_dark,
                    );
                    return Ok::<_, String>(ProcessedFile::Binary {
                        display_file,
                        old: prepare_side(raw_file.old_path, contents.old),
                        new: prepare_side(raw_file.new_path, contents.new),
                    });
                }
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
                Ok::<_, String>(ProcessedFile::Text {
                    old_content,
                    new_content,
                    display_file,
                    max_line_num,
                })
            })
            .await;

            let Some(this) = this.upgrade() else {
                if let Ok(processed) = result {
                    cx.update(|cx| processed.release(cx));
                }
                return;
            };
            this.update(cx, |this, cx| {
                if this.request_generation != request_generation {
                    if let Ok(processed) = result {
                        processed.release(cx);
                    }
                    return;
                }
                this.binary_preview_loading = false;
                match result {
                    Ok(ProcessedFile::Text {
                        old_content,
                        new_content,
                        display_file,
                        max_line_num,
                    }) => {
                        this.current_file_old_content = old_content;
                        this.current_file_new_content = new_content;
                        this.line_num_width = max_line_num.to_string().len().max(3);
                        this.max_line_chars = Self::calc_max_line_chars(&display_file);
                        this.current_file = Some(display_file);
                        this.update_side_by_side_cache();
                    }
                    Ok(ProcessedFile::Binary {
                        display_file,
                        old,
                        new,
                    }) => {
                        let install =
                            |side: PreparedBinarySide,
                             id: &'static str,
                             cx: &mut Context<DiffViewer>| {
                                let (renderer, error) = match side.content {
                                    Ok(content) => (
                                        Some(cx.new(|cx| FileRenderer::new(id, content, cx))),
                                        None,
                                    ),
                                    Err(error) => (None, Some(error)),
                                };
                                BinaryPreviewSide {
                                    path: side.path,
                                    renderer,
                                    error,
                                }
                            };
                        this.current_file_old_content = None;
                        this.current_file_new_content = None;
                        this.line_num_width = 3;
                        this.max_line_chars = 0;
                        this.current_file = Some(display_file);
                        this.binary_preview = Some(BinaryDiffPreview {
                            old: old.map(|side| install(side, "binary-diff-old", cx)),
                            new: new.map(|side| install(side, "binary-diff-new", cx)),
                        });
                        this.side_by_side_lines.clear();
                    }
                    Err(error) => {
                        this.current_file = None;
                        this.error_message = Some(error);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Re-highlight current file using cached content (for theme changes).
    pub(super) fn rehighlight_current_file(&mut self) {
        let Some(raw_file) = self.raw_files.get(self.selected_file_index) else {
            return;
        };

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
        let tree = build_file_tree(
            self.file_stats
                .iter()
                .enumerate()
                .filter(|(_, file)| self.composition.accepts(&file.path))
                .map(|(index, file)| (index, &file.path)),
        );
        self.file_tree = tree;
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

impl DiffViewer {
    /// Load the role composition for the comparison already on screen.
    ///
    /// Separate from the diff on purpose: it parses source, so it must never
    /// delay the diff the reviewer actually asked for.
    pub(super) fn load_composition_async(&mut self, cx: &mut Context<Self>) {
        let generation = self.request_generation;
        let provider = self.provider.clone();
        let mode = self.diff_mode.clone();
        let ignore_whitespace = self.ignore_whitespace;
        self.composition.loading = true;

        cx.spawn(async move |this, cx| {
            let result =
                smol::unblock(move || provider.get_review_composition(mode, ignore_whitespace))
                    .await;
            let _ = this.update(cx, |this, cx| {
                if this.request_generation != generation {
                    return;
                }
                match result {
                    Ok(composition) => this.composition.set(composition),
                    Err(error) => this.composition.fail(error),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Rebuild the tree for the current role filter, keeping a visible file
    /// selected so the diff pane never goes blank behind a filter.
    pub(super) fn apply_role_filter(&mut self, cx: &mut Context<Self>) {
        self.build_file_tree();
        let selected_visible = self
            .file_stats
            .get(self.selected_file_index)
            .is_some_and(|file| self.composition.accepts(&file.path));
        if !selected_visible
            && let Some(index) = self
                .file_stats
                .iter()
                .position(|file| self.composition.accepts(&file.path))
        {
            self.selected_file_index = index;
            self.process_current_file_async(cx);
        }
        cx.notify();
    }

    /// Files the role filter lets through.
    pub(super) fn visible_file_count(&self) -> usize {
        self.file_stats
            .iter()
            .filter(|file| self.composition.accepts(&file.path))
            .count()
    }
}
