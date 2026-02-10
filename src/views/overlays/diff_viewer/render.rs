//! Render helper methods for the diff viewer.

use super::types::{DiffViewMode, FileTreeNode};
use super::{DiffViewer, SIDEBAR_WIDTH};
use crate::theme::ThemeColors;
use crate::views::components::segmented_toggle;
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
        is_working: bool,
        ignore_whitespace: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
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
                    .gap(px(16.0))
                    .child(
                        div()
                            .text_size(px(15.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(t.text_primary))
                            .child("Changes"),
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
                                        .child("·"),
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
                    // Diff mode toggle
                    .child(
                        div()
                            .id("diff-mode-toggle")
                            .on_click(cx.listener(|this, _, _window, cx| this.toggle_mode(cx)))
                            .child(segmented_toggle(
                                &[("Unstaged", is_working), ("Staged", !is_working)],
                                t,
                            )),
                    )
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
                                    .child("×"),
                            ),
                    ),
            )
    }

    pub(super) fn render_content(
        &mut self,
        t: &ThemeColors,
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
            .when(has_error, |d| {
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
            .when(!has_error && has_files, |d| {
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
                    // Scale mouse movement: track width ≈ viewport, content can be much wider
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

    pub(super) fn render_footer(&self, t: &ThemeColors, has_selection: bool) -> impl IntoElement {
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
                    .child(self.render_hint("Tab", "staged/unstaged", t))
                    .child(self.render_hint("S", "split", t))
                    .child(self.render_hint("↑↓", "navigate", t))
                    .child(self.render_hint(
                        if cfg!(target_os = "macos") {
                            "⌘C"
                        } else {
                            "Ctrl+C"
                        },
                        "copy",
                        t,
                    )),
            )
            .when(has_selection, |d| {
                d.child(
                    div()
                        .px(px(8.0))
                        .py(px(3.0))
                        .rounded(px(4.0))
                        .bg(rgb(t.bg_selection))
                        .text_size(px(11.0))
                        .text_color(rgb(t.text_secondary))
                        .child("Selection active"),
                )
            })
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
        node: &FileTreeNode,
        depth: usize,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut elements: Vec<AnyElement> = Vec::new();

        for (name, child) in &node.children {
            let indent = depth * 14;
            let has_content = !child.files.is_empty() || !child.children.is_empty();

            if has_content {
                elements.push(
                    h_flex()
                        .h(px(26.0))
                        .pl(px(indent as f32 + 12.0))
                        .child(
                            div()
                                .text_size(px(12.0))
                                .text_color(rgb(t.text_muted))
                                .child(format!("{}/", name)),
                        )
                        .into_any_element(),
                );

                elements.extend(self.render_tree_node(child, depth + 1, t, cx));
            }
        }

        for &file_index in &node.files {
            if let Some(file) = self.file_stats.get(file_index) {
                let indent = depth * 14;
                let is_selected = file_index == self.selected_file_index;
                let filename = file.path.rsplit('/').next().unwrap_or(&file.path);
                let added = file.added;
                let removed = file.removed;
                let is_new = file.is_new;
                let is_deleted = file.is_deleted;

                // Status indicator styling
                let (status_char, status_color) = if is_new {
                    ("A", t.diff_added_fg)
                } else if is_deleted {
                    ("D", t.diff_removed_fg)
                } else {
                    ("M", t.text_muted)
                };

                elements.push(
                    div()
                        .id(ElementId::Name(format!("tree-file-{}", file_index).into()))
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .h(px(28.0))
                        .pl(px(indent as f32 + 12.0))
                        .pr(px(12.0))
                        .mx(px(4.0))
                        .rounded(px(4.0))
                        .cursor_pointer()
                        .when(is_selected, |d| d.bg(rgb(t.bg_selection)))
                        .hover(|s| s.bg(rgb(t.bg_hover)))
                        .on_click(cx.listener(move |this, _, _window, cx| {
                            this.select_file(file_index, cx);
                        }))
                        // Status badge
                        .child(
                            div()
                                .text_size(px(10.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(rgb(status_color))
                                .child(status_char),
                        )
                        // Filename
                        .child(
                            div()
                                .flex_1()
                                .text_size(px(13.0))
                                .text_color(rgb(t.text_primary))
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .child(filename.to_string()),
                        )
                        // Line counts - more subtle
                        .when(added > 0 || removed > 0, |d| {
                            d.child(
                                h_flex()
                                    .gap(px(4.0))
                                    .text_size(px(11.0))
                                    .when(added > 0, |d| {
                                        d.child(
                                            div()
                                                .text_color(rgb(t.diff_added_fg))
                                                .child(format!("+{}", added)),
                                        )
                                    })
                                    .when(removed > 0, |d| {
                                        d.child(
                                            div()
                                                .text_color(rgb(t.diff_removed_fg))
                                                .child(format!("-{}", removed)),
                                        )
                                    }),
                            )
                        })
                        .into_any_element(),
                );
            }
        }

        elements
    }
}
