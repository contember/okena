//! Render helper methods for the diff viewer.

use super::types::{DiffViewMode, FileTreeNode};
use super::{DiffViewer, SIDEBAR_WIDTH};
use okena_core::theme::ThemeColors;
use okena_ui::toggle::segmented_toggle;
use crate::DiffMode;
use gpui::prelude::*;
use gpui::*;
use gpui_component::h_flex;
use std::sync::Arc;

impl DiffViewer {
    pub(super) fn render_header(
        &self,
        t: &ThemeColors,
        has_files: bool,
        file_count: usize,
        total_added: usize,
        total_removed: usize,
        diff_mode: &DiffMode,
        ignore_whitespace: bool,
        commit_message: Option<&str>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let is_working = *diff_mode == DiffMode::WorkingTree;
        let hide_mode_toggle = matches!(diff_mode, DiffMode::Commit(_) | DiffMode::BranchCompare { .. });
        let is_unified = self.view_mode == DiffViewMode::Unified;

        div()
            .px(px(20.0))
            .py(px(14.0))
            .border_b_1()
            .border_color(rgb(t.border))
            .flex()
            .items_center()
            .justify_between()
            .child(
                h_flex()
                    .gap(px(10.0))
                    .min_w_0()
                    // Title
                    .child({
                        let title = match diff_mode {
                            DiffMode::Commit(_) => commit_message.unwrap_or("Commit").to_string(),
                            DiffMode::BranchCompare { base, head } => format!("{base} \u{2192} {head}"),
                            _ => "Changes".to_string(),
                        };
                        div()
                            .text_size(px(15.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(t.text_primary))
                            .text_ellipsis()
                            .overflow_hidden()
                            .min_w_0()
                            .max_w(px(400.0))
                            .child(title)
                    })
                    // Clickable short hash (copy on click) — for commit mode
                    .when_some(
                        if let DiffMode::Commit(hash) = diff_mode {
                            Some(hash.clone())
                        } else {
                            None
                        },
                        |d, hash| {
                            let short = if hash.len() > 7 { hash[..7].to_string() } else { hash.clone() };
                            d.child(
                                div()
                                    .id("commit-hash-copy")
                                    .text_size(px(11.0))
                                    .font_family("monospace")
                                    .text_color(rgb(t.term_yellow))
                                    .cursor_pointer()
                                    .px(px(5.0))
                                    .py(px(2.0))
                                    .rounded(px(4.0))
                                    .hover(|s| s.bg(rgb(t.bg_hover)))
                                    .on_click(move |_, _, cx| {
                                        cx.write_to_clipboard(ClipboardItem::new_string(hash.clone()));
                                    })
                                    .tooltip(|_window, cx| gpui_component::tooltip::Tooltip::new("Copy commit hash").build(_window, cx))
                                    .child(short),
                            )
                        },
                    )
                    .when(has_files, |d| {
                        d.child(
                            h_flex()
                                .gap(px(6.0))
                                .pl(px(8.0))
                                .border_l_1()
                                .border_color(rgb(t.border))
                                .child(
                                    div()
                                        .text_size(px(12.0))
                                        .text_color(rgb(t.text_muted))
                                        .child(format!(
                                            "{} {}",
                                            file_count,
                                            if file_count == 1 { "file" } else { "files" }
                                        )),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.0))
                                        .text_color(rgb(t.text_muted))
                                        .child("\u{00B7}"),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.0))
                                        .text_color(rgb(t.diff_added_fg))
                                        .child(format!("+{}", total_added)),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.0))
                                        .text_color(rgb(t.diff_removed_fg))
                                        .child(format!("-{}", total_removed)),
                                ),
                        )
                    }),
            )
            .child(
                h_flex()
                    .gap(px(8.0))
                    // Whitespace toggle
                    .child(
                        div()
                            .id("ignore-whitespace-toggle")
                            .cursor_pointer()
                            .px(px(10.0))
                            .py(px(5.0))
                            .rounded(px(6.0))
                            .bg(rgb(if ignore_whitespace {
                                t.button_primary_bg
                            } else {
                                t.bg_secondary
                            }))
                            .hover(|s| s.opacity(0.85))
                            .on_click(cx.listener(|this, _, _window, cx| {
                                this.toggle_ignore_whitespace(cx)
                            }))
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .text_color(rgb(if ignore_whitespace {
                                        t.button_primary_fg
                                    } else {
                                        t.text_secondary
                                    }))
                                    .child("Whitespace"),
                            ),
                    )
                    // Separator
                    .child(
                        div()
                            .w(px(1.0))
                            .h(px(20.0))
                            .bg(rgb(t.border))
                            .mx(px(4.0)),
                    )
                    // View mode toggle
                    .child(
                        div()
                            .id("view-mode-toggle")
                            .on_click(cx.listener(|this, _, _window, cx| this.toggle_view_mode(cx)))
                            .child(segmented_toggle(
                                &[("Unified", is_unified), ("Split", !is_unified)],
                                t,
                            )),
                    )
                    // Diff mode toggle (hidden for commit/branch compare diffs)
                    .when(!hide_mode_toggle, |d| {
                        d.child(
                            div()
                                .id("diff-mode-toggle")
                                .on_click(cx.listener(|this, _, _window, cx| this.toggle_mode(cx)))
                                .child(segmented_toggle(
                                    &[("Unstaged", is_working), ("Staged", !is_working)],
                                    t,
                                )),
                        )
                    })
                    // Separator
                    .child(
                        div()
                            .w(px(1.0))
                            .h(px(20.0))
                            .bg(rgb(t.border))
                            .mx(px(4.0)),
                    )
                    // Close button
                    .child(
                        div()
                            .id("close-button")
                            .cursor_pointer()
                            .w(px(28.0))
                            .h(px(28.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(6.0))
                            .hover(|s| s.bg(rgb(t.bg_hover)))
                            .on_click(cx.listener(|this, _, _window, cx| this.close(cx)))
                            .child(
                                div()
                                    .text_size(px(16.0))
                                    .text_color(rgb(t.text_muted))
                                    .child("\u{00D7}"),
                            ),
                    ),
            )
    }

    /// Commit navigation bar: prev/next arrows, author, date, hash, position indicator.
    pub(super) fn render_commit_info_bar(&self, t: &ThemeColors, cx: &mut Context<Self>) -> impl IntoElement {
        use gpui_component::tooltip::Tooltip;

        let commit = self.commits.get(self.commit_index);
        let can_prev = self.can_prev_commit();
        let can_next = self.can_next_commit();
        let position = format!("{}/{}", self.commit_index + 1, self.commits.len());

        h_flex()
            .px(px(20.0))
            .py(px(6.0))
            .gap(px(8.0))
            .items_center()
            .border_b_1()
            .border_color(rgb(t.border))
            .bg(rgb(t.bg_secondary))
            // Prev button
            .child(
                div()
                    .id("commit-nav-prev")
                    .cursor(if can_prev { CursorStyle::PointingHand } else { CursorStyle::default() })
                    .w(px(24.0))
                    .h(px(22.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.0))
                    .when(can_prev, |d| d.hover(|s| s.bg(rgb(t.bg_hover))))
                    .text_size(px(12.0))
                    .text_color(rgb(if can_prev { t.text_secondary } else { t.text_muted }))
                    .when(can_prev, |d| {
                        d.on_click(cx.listener(|this, _, _window, cx| this.prev_commit(cx)))
                    })
                    .child("\u{25C0}")
                    .tooltip(move |_window, cx| Tooltip::new("Previous commit  [").build(_window, cx)),
            )
            // Position
            .child(
                div()
                    .text_size(px(10.0))
                    .text_color(rgb(t.text_muted))
                    .min_w(px(36.0))
                    .text_align(TextAlign::Center)
                    .child(position),
            )
            // Next button
            .child(
                div()
                    .id("commit-nav-next")
                    .cursor(if can_next { CursorStyle::PointingHand } else { CursorStyle::default() })
                    .w(px(24.0))
                    .h(px(22.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.0))
                    .when(can_next, |d| d.hover(|s| s.bg(rgb(t.bg_hover))))
                    .text_size(px(12.0))
                    .text_color(rgb(if can_next { t.text_secondary } else { t.text_muted }))
                    .when(can_next, |d| {
                        d.on_click(cx.listener(|this, _, _window, cx| this.next_commit(cx)))
                    })
                    .child("\u{25B6}")
                    .tooltip(move |_window, cx| Tooltip::new("Next commit  ]").build(_window, cx)),
            )
            // Separator
            .child(div().w(px(1.0)).h(px(16.0)).bg(rgb(t.border)))
            // Commit metadata
            .when_some(commit.cloned(), |d, commit| {
                let hash = commit.hash.clone();
                let short = if hash.len() > 7 { hash[..7].to_string() } else { hash.clone() };
                let time_str = crate::format_relative_time(commit.timestamp);
                d
                    // Hash (clickable, copies to clipboard)
                    .child(
                        div()
                            .id("commit-info-hash")
                            .text_size(px(11.0))
                            .font_family("monospace")
                            .text_color(rgb(t.term_yellow))
                            .cursor_pointer()
                            .px(px(4.0))
                            .py(px(1.0))
                            .rounded(px(3.0))
                            .hover(|s| s.bg(rgb(t.bg_hover)))
                            .on_click(move |_, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(hash.clone()));
                            })
                            .tooltip(|_window, cx| Tooltip::new("Copy hash").build(_window, cx))
                            .child(short),
                    )
                    // Author
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(rgb(t.text_secondary))
                            .child(commit.author.clone()),
                    )
                    // Date
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(rgb(t.text_muted))
                            .child(time_str),
                    )
            })
    }

    pub(super) fn render_content(
        &mut self,
        t: &ThemeColors,
        loading: bool,
        has_error: bool,
        error_message: Option<String>,
        has_files: bool,
        is_binary: bool,
        file_path: String,
        line_count: usize,
        gutter_width: f32,
        tree_elements: Vec<AnyElement>,
        theme_colors: Arc<ThemeColors>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .flex_1()
            .flex()
            .min_h_0()
            .when(loading, |d| {
                d.child(
                    div()
                        .flex_1()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            div()
                                .text_size(px(14.0))
                                .text_color(rgb(t.text_muted))
                                .child("Loading..."),
                        ),
                )
            })
            .when(!loading && has_error, |d| {
                d.child(
                    div()
                        .flex_1()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            div()
                                .text_size(px(14.0))
                                .text_color(rgb(t.text_muted))
                                .child(error_message.unwrap_or_default()),
                        ),
                )
            })
            .when(!loading && !has_error && has_files, |d| {
                d.child(self.render_sidebar(t, tree_elements)).child(
                    self.render_diff_pane(
                        t,
                        is_binary,
                        file_path,
                        line_count,
                        gutter_width,
                        theme_colors,
                        cx,
                    ),
                )
            })
    }

    pub(super) fn render_sidebar(
        &self,
        t: &ThemeColors,
        tree_elements: Vec<AnyElement>,
    ) -> impl IntoElement {
        div()
            .w(px(SIDEBAR_WIDTH))
            .h_full()
            .border_r_1()
            .border_color(rgb(t.border))
            .bg(rgb(t.bg_primary))
            .flex()
            .flex_col()
            .child(
                div()
                    .px(px(16.0))
                    .py(px(10.0))
                    .border_b_1()
                    .border_color(rgb(t.border))
                    .text_size(px(11.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(t.text_muted))
                    .line_height(px(11.0))
                    .child("Files"),
            )
            .child(
                div()
                    .id("file-tree")
                    .flex_1()
                    .overflow_y_scroll()
                    .track_scroll(&self.tree_scroll_handle)
                    .py(px(6.0))
                    .children(tree_elements),
            )
    }

    pub(super) fn render_diff_pane(
        &mut self,
        t: &ThemeColors,
        is_binary: bool,
        file_path: String,
        line_count: usize,
        gutter_width: f32,
        theme_colors: Arc<ThemeColors>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let scrollbar_geometry = self.get_scrollbar_geometry();
        let is_dragging = self.scrollbar_drag.is_some();
        let tc = theme_colors;
        let view = cx.entity().clone();
        let side_by_side_count = self.side_by_side_lines.len();

        // For new/deleted files, always use unified view (no point in split)
        let current_stats = self.file_stats.get(self.selected_file_index);
        let is_new_or_deleted = current_stats
            .map(|f| f.is_new || f.is_deleted)
            .unwrap_or(false);
        let view_mode = if is_new_or_deleted {
            DiffViewMode::Unified
        } else {
            self.view_mode
        };

        // Horizontal scrollbar
        let scroll_x = self.scroll_x;
        self.viewport_width(); // update cached width from scroll handle
        let has_h_scroll = if self.diff_pane_width > 0.0 {
            self.max_scroll_x() > 1.0
        } else {
            // Viewport not yet measured — skip scrollbar, schedule re-render
            cx.notify();
            false
        };
        let max_scroll = self.max_scroll_x();

        div()
            .flex_1()
            .flex()
            .flex_col()
            .min_w_0()
            .min_h_0()
            .child(
                div()
                    .px(px(16.0))
                    .py(px(10.0))
                    .border_b_1()
                    .border_color(rgb(t.border))
                    .bg(rgb(t.bg_header))
                    .text_size(px(12.0))
                    .font_family("monospace")
                    .text_color(rgb(t.text_secondary))
                    .child(file_path),
            )
            .when(is_binary, |d| {
                d.child(
                    div()
                        .flex_1()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            div()
                                .text_size(px(14.0))
                                .text_color(rgb(t.text_muted))
                                .child("Binary file - cannot display diff"),
                        ),
                )
            })
            .when(!is_binary, |d| {
                let item_count = match view_mode {
                    DiffViewMode::Unified => line_count,
                    DiffViewMode::SideBySide => side_by_side_count,
                };

                d.child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .relative()
                        .child(
                            uniform_list("diff-lines", item_count, move |range, _window, cx| {
                                let tc = tc.clone();
                                view.update(cx, |this, cx| {
                                    match view_mode {
                                        DiffViewMode::Unified => {
                                            this.render_visible_lines(range, &tc, gutter_width, cx)
                                        }
                                        DiffViewMode::SideBySide => {
                                            this.render_side_by_side_lines(range, &tc, cx)
                                        }
                                    }
                                })
                            })
                            .size_full()
                            .bg(rgb(t.bg_secondary))
                            .cursor(CursorStyle::IBeam)
                            .map(|mut list| {
                                // Prevent GPUI from redirecting horizontal delta to
                                // vertical scroll when only overflow-y is set (which
                                // causes diagonal scroll on Shift+wheel).
                                list.style().restrict_scroll_to_axis = Some(true);
                                list
                            })
                            .on_scroll_wheel(cx.listener(move |this, event: &ScrollWheelEvent, _window, cx| {
                                this.handle_scroll_x(event, cx);
                            }))
                            .track_scroll(&self.scroll_handle),
                        )
                        .when(scrollbar_geometry.is_some(), |d| {
                            let (_, _, thumb_y, thumb_height) = scrollbar_geometry.unwrap();
                            d.child(self.render_scrollbar_thumb(
                                t,
                                thumb_y,
                                thumb_height,
                                is_dragging,
                                cx,
                            ))
                        }),
                )
                // Horizontal scrollbar
                .when(has_h_scroll, |d| {
                    d.child(self.render_horizontal_scrollbar(t, scroll_x, max_scroll, cx))
                })
            })
    }

    pub(super) fn render_scrollbar_thumb(
        &self,
        t: &ThemeColors,
        thumb_y: f32,
        thumb_height: f32,
        is_dragging: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .id("diff-scrollbar-track")
            .absolute()
            .top_0()
            .bottom_0()
            .right_0()
            .w(px(12.0))
            .cursor(CursorStyle::Arrow)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                    let y = f32::from(event.position.y);
                    if this.get_scrollbar_geometry().is_some() {
                        this.start_scrollbar_drag(y, cx);
                    }
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                if this.scrollbar_drag.is_some() {
                    let y = f32::from(event.position.y);
                    this.update_scrollbar_drag(y, cx);
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _window, cx| this.end_scrollbar_drag(cx)),
            )
            .child(
                div()
                    .absolute()
                    .top(px(thumb_y))
                    .right(px(3.0))
                    .w(px(6.0))
                    .h(px(thumb_height))
                    .rounded(px(3.0))
                    .bg(rgb(if is_dragging {
                        t.scrollbar_hover
                    } else {
                        t.scrollbar
                    }))
                    .hover(|s| s.bg(rgb(t.scrollbar_hover))),
            )
    }

    pub(super) fn render_horizontal_scrollbar(
        &self,
        t: &ThemeColors,
        scroll_x: f32,
        max_scroll: f32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let track_height = 10.0;
        let is_dragging_h = self.h_scrollbar_drag.is_some();

        // Thumb size: visible fraction of total content
        let text_w = self.max_text_width();
        let avail_w = self.available_text_width();
        let visible_ratio = if text_w > 0.0 {
            (avail_w / text_w).clamp(0.05, 0.95)
        } else {
            1.0
        };
        // Scroll position ratio
        let scroll_ratio = if max_scroll > 0.0 {
            (scroll_x / max_scroll).clamp(0.0, 1.0)
        } else {
            0.0
        };

        div()
            .id("diff-h-scrollbar-track")
            .w_full()
            .h(px(track_height))
            .border_t_1()
            .border_color(rgb(t.border))
            .cursor(CursorStyle::Arrow)
            .relative()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                    let x = f32::from(event.position.x);
                    this.h_scrollbar_drag = Some(super::types::HScrollbarDrag {
                        start_x: x,
                        start_scroll_x: this.scroll_x,
                    });
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                if let Some(drag) = this.h_scrollbar_drag {
                    let x = f32::from(event.position.x);
                    let delta_x = x - drag.start_x;
                    let max = this.max_scroll_x();
                    // Scale mouse movement: track width ~ viewport, content can be much wider
                    let text_w = this.max_text_width();
                    let avail_w = this.available_text_width();
                    let scale = if avail_w > 0.0 { text_w / avail_w } else { 1.0 };
                    this.scroll_x = (drag.start_scroll_x + delta_x * scale).clamp(0.0, max);
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _window, cx| {
                    this.h_scrollbar_drag = None;
                    cx.notify();
                }),
            )
            .child(
                div()
                    .absolute()
                    .top(px(2.0))
                    .h(px(track_height - 4.0))
                    .rounded(px(3.0))
                    .bg(rgb(if is_dragging_h {
                        t.scrollbar_hover
                    } else {
                        t.scrollbar
                    }))
                    .hover(|s| s.bg(rgb(t.scrollbar_hover)))
                    // Position and size using percentages of the track
                    .left(relative(scroll_ratio * (1.0 - visible_ratio)))
                    .w(relative(visible_ratio)),
            )
    }

    pub(super) fn render_footer(&self, t: &ThemeColors) -> impl IntoElement {
        let has_commits = self.has_commits();
        div()
            .px(px(16.0))
            .py(px(8.0))
            .border_t_1()
            .border_color(rgb(t.border))
            .flex()
            .items_center()
            .justify_between()
            .child(
                h_flex()
                    .gap(px(20.0))
                    .child(self.render_hint("Esc", "close", t))
                    .when(!has_commits, |d| {
                        d.child(self.render_hint("Tab", "staged/unstaged", t))
                    })
                    .child(self.render_hint("S", "split", t))
                    .child(self.render_hint("\u{2191}\u{2193}", "files", t))
                    .when(has_commits, |d| {
                        d.child(self.render_hint("[ ]", "commits", t))
                    })
                    .child(self.render_hint(
                        if cfg!(target_os = "macos") {
                            "\u{2318}C"
                        } else {
                            "Ctrl+C"
                        },
                        "copy",
                        t,
                    )),
            )
    }

    pub(super) fn render_hint(
        &self,
        key: &str,
        action: &str,
        t: &ThemeColors,
    ) -> impl IntoElement {
        h_flex()
            .gap(px(5.0))
            .child(
                div()
                    .px(px(6.0))
                    .py(px(2.0))
                    .rounded(px(4.0))
                    .bg(rgb(t.bg_secondary))
                    .border_1()
                    .border_color(rgb(t.border))
                    .text_size(px(11.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(t.text_muted))
                    .child(key.to_string()),
            )
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(rgb(t.text_muted))
                    .child(action.to_string()),
            )
    }

    pub(super) fn render_tree_node(
        &self,
        tree: &FileTreeNode,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        use okena_files::file_tree::{flatten_file_tree, render_folder_row, render_file_row, FileTreeItem};

        let mut elements: Vec<AnyElement> = Vec::new();
        for item in flatten_file_tree(tree, 0) {
            match item {
                FileTreeItem::Folder { name, depth } => {
                    elements.push(render_folder_row(name, depth, t));
                }
                FileTreeItem::File { index, depth } => {
                    if let Some(file) = self.file_stats.get(index) {
                        let filename = file.path.rsplit('/').next().unwrap_or(&file.path);
                        let is_selected = index == self.selected_file_index;
                        elements.push(
                            render_file_row(depth, filename, file.added, file.removed, file.is_new, file.is_deleted, is_selected, t)
                                .id(ElementId::Name(format!("tree-file-{}", index).into()))
                                .on_click(cx.listener(move |this, _, _window, cx| {
                                    this.select_file(index, cx);
                                }))
                                .into_any_element(),
                        );
                    }
                }
            }
        }
        elements
    }
}
