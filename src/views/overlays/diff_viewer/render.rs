//! Render helper methods for the diff viewer.

use super::types::FileTreeNode;
use super::{DiffViewer, SIDEBAR_WIDTH};
use crate::theme::ThemeColors;
use crate::views::components::segmented_toggle;
use gpui::prelude::*;
use gpui::*;
use std::sync::Arc;

impl DiffViewer {
    pub(super) fn render_header(
        &self,
        t: &ThemeColors,
        has_files: bool,
        total_added: usize,
        total_removed: usize,
        is_working: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .px(px(16.0))
            .py(px(10.0))
            .border_b_1()
            .border_color(rgb(t.border))
            .flex()
            .items_center()
            .justify_between()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .child(
                        div()
                            .text_size(px(14.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(t.text_primary))
                            .child("Git Diff"),
                    )
                    .when(has_files, |d| {
                        d.child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .child(
                                    div()
                                        .text_size(px(11.0))
                                        .text_color(rgb(t.text_muted))
                                        .child(format!("{} files", self.files.len())),
                                )
                                .child(
                                    div()
                                        .text_size(px(11.0))
                                        .text_color(rgb(t.diff_added_fg))
                                        .child(format!("+{}", total_added)),
                                )
                                .child(
                                    div()
                                        .text_size(px(11.0))
                                        .text_color(rgb(t.diff_removed_fg))
                                        .child(format!("-{}", total_removed)),
                                ),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .child(
                        div()
                            .id("diff-mode-toggle")
                            .on_click(cx.listener(|this, _, _window, cx| this.toggle_mode(cx)))
                            .child(segmented_toggle(
                                &[("Unstaged", is_working), ("Staged", !is_working)],
                                t,
                            )),
                    )
                    .child(
                        div()
                            .id("close-button")
                            .cursor_pointer()
                            .px(px(8.0))
                            .py(px(4.0))
                            .rounded(px(4.0))
                            .hover(|s| s.bg(rgb(t.bg_secondary)))
                            .on_click(cx.listener(|this, _, _window, cx| this.close(cx)))
                            .child(
                                div()
                                    .text_size(px(18.0))
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
                    .px(px(12.0))
                    .py(px(8.0))
                    .border_b_1()
                    .border_color(rgb(t.border))
                    .text_size(px(11.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(t.text_secondary))
                    .child("CHANGED FILES"),
            )
            .child(
                div()
                    .id("file-tree")
                    .flex_1()
                    .overflow_y_scroll()
                    .track_scroll(&self.tree_scroll_handle)
                    .py(px(4.0))
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

        div()
            .flex_1()
            .flex()
            .flex_col()
            .min_w_0()
            .min_h_0()
            .child(
                div()
                    .px(px(12.0))
                    .py(px(8.0))
                    .border_b_1()
                    .border_color(rgb(t.border))
                    .bg(rgb(t.bg_header))
                    .text_size(px(self.file_font_size))
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
                d.child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .relative()
                        .child(
                            uniform_list("diff-lines", line_count, move |range, _window, cx| {
                                let tc = tc.clone();
                                view.update(cx, |this, cx| {
                                    this.render_visible_lines(range, &tc, gutter_width, cx)
                                })
                            })
                            .size_full()
                            .bg(rgb(t.bg_secondary))
                            .cursor(CursorStyle::IBeam)
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

    pub(super) fn render_footer(&self, t: &ThemeColors, has_selection: bool) -> impl IntoElement {
        div()
            .px(px(12.0))
            .py(px(6.0))
            .border_t_1()
            .border_color(rgb(t.border))
            .flex()
            .items_center()
            .justify_between()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(16.0))
                    .child(self.render_hint("Esc", "close", t))
                    .child(self.render_hint("Tab", "toggle mode", t))
                    .child(self.render_hint("↑↓", "files", t))
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
            .child(
                div()
                    .text_size(px(10.0))
                    .text_color(rgb(t.text_muted))
                    .when(has_selection, |d| d.child("Selection active")),
            )
    }

    pub(super) fn render_hint(
        &self,
        key: &str,
        action: &str,
        t: &ThemeColors,
    ) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .gap(px(4.0))
            .child(
                div()
                    .px(px(4.0))
                    .py(px(1.0))
                    .rounded(px(3.0))
                    .bg(rgb(t.bg_secondary))
                    .text_size(px(10.0))
                    .text_color(rgb(t.text_muted))
                    .child(key.to_string()),
            )
            .child(
                div()
                    .text_size(px(10.0))
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
            let indent = depth * 12;
            let has_content = !child.files.is_empty() || !child.children.is_empty();

            if has_content {
                elements.push(
                    div()
                        .flex()
                        .items_center()
                        .py(px(2.0))
                        .pl(px(indent as f32 + 8.0))
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(rgb(t.text_secondary))
                                .font_weight(FontWeight::MEDIUM)
                                .child(format!("{}/", name)),
                        )
                        .into_any_element(),
                );

                elements.extend(self.render_tree_node(child, depth + 1, t, cx));
            }
        }

        for &file_index in &node.files {
            if let Some(file) = self.files.get(file_index) {
                let indent = depth * 12;
                let is_selected = file_index == self.selected_file_index;
                let filename = file.path.rsplit('/').next().unwrap_or(&file.path);
                let added = file.added;
                let removed = file.removed;
                let is_new = file.is_new;
                let is_deleted = file.is_deleted;

                elements.push(
                    div()
                        .id(ElementId::Name(format!("tree-file-{}", file_index).into()))
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .py(px(3.0))
                        .pl(px(indent as f32 + 8.0))
                        .pr(px(8.0))
                        .cursor_pointer()
                        .when(is_selected, |d| d.bg(rgb(t.bg_selection)))
                        .hover(|s| s.bg(rgb(t.bg_hover)))
                        .on_click(cx.listener(move |this, _, _window, cx| {
                            this.select_file(file_index, cx);
                        }))
                        .child(
                            div()
                                .text_size(px(10.0))
                                .w(px(12.0))
                                .text_color(if is_new {
                                    rgb(t.diff_added_fg)
                                } else if is_deleted {
                                    rgb(t.diff_removed_fg)
                                } else {
                                    rgb(t.text_muted)
                                })
                                .child(if is_new {
                                    "A"
                                } else if is_deleted {
                                    "D"
                                } else {
                                    "M"
                                }),
                        )
                        .child(
                            div()
                                .flex_1()
                                .text_size(px(12.0))
                                .text_color(rgb(t.text_primary))
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .child(filename.to_string()),
                        )
                        .when(added > 0 || removed > 0, |d| {
                            d.child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(2.0))
                                    .when(added > 0, |d| {
                                        d.child(
                                            div()
                                                .text_size(px(10.0))
                                                .text_color(rgb(t.diff_added_fg))
                                                .child(format!("+{}", added)),
                                        )
                                    })
                                    .when(removed > 0, |d| {
                                        d.child(
                                            div()
                                                .text_size(px(10.0))
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
