//! Rendering logic for the file viewer overlay.

use crate::keybindings::Cancel;
use crate::theme::{theme, ThemeColors};
use crate::ui::{Selection1DExtension, Selection2DNonEmpty};
use crate::views::components::{
    build_styled_text_with_backgrounds, code_block_container, find_word_boundaries,
    get_scrollbar_geometry, modal_backdrop, modal_content, segmented_toggle, selection_bg_ranges,
    HighlightedLine,
};
use super::markdown_renderer::RenderedNode;
use super::{DisplayMode, FileViewer, SIDEBAR_WIDTH};
use crate::views::components::FileTreeNode;
use gpui::*;
use gpui_component::{h_flex, v_flex};
use gpui::prelude::*;
use std::sync::Arc;

/// Helper to create rgba from u32 color and alpha.
fn rgba(color: u32, alpha: f32) -> Rgba {
    let r = ((color >> 16) & 0xFF) as f32 / 255.0;
    let g = ((color >> 8) & 0xFF) as f32 / 255.0;
    let b = (color & 0xFF) as f32 / 255.0;
    Rgba { r, g, b, a: alpha }
}

impl FileViewer {
    /// Render a single highlighted line with selection support.
    pub(super) fn render_line(&self, line_number: usize, line: &HighlightedLine, t: &ThemeColors, cx: &mut Context<Self>) -> Stateful<Div> {
        // Format line number with right padding
        let line_num_str = format!("{:>width$}", line_number + 1, width = self.line_num_width);

        let font_size = self.file_font_size;
        let line_height = font_size * 1.8;
        let char_width = self.measured_char_width;
        let gutter_width = (self.line_num_width as f32) * char_width + 16.0;

        let bg_ranges = selection_bg_ranges(&self.selection, line_number, line.plain_text.len());

        let plain_text = line.plain_text.clone();
        let line_len = line.plain_text.len();

        // Build styled text and capture its layout for accurate position-to-index mapping
        let styled_text = build_styled_text_with_backgrounds(&line.spans, &bg_ranges);
        let text_layout = styled_text.layout().clone();

        div()
            .id(ElementId::Name(format!("line-{}", line_number).into()))
            .flex()
            .h(px(line_height))
            .text_size(px(font_size))
            .font_family("monospace")
            .on_mouse_down(MouseButton::Left, {
                let text_layout = text_layout.clone();
                let plain_text = plain_text.clone();
                cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                    let col = text_layout.index_for_position(event.position)
                        .unwrap_or_else(|ix| ix)
                        .min(line_len);
                    if event.click_count >= 3 {
                        this.selection.start = Some((line_number, 0));
                        this.selection.end = Some((line_number, line_len));
                        this.selection.finish();
                    } else if event.click_count == 2 {
                        let (start, end) = find_word_boundaries(&plain_text, col);
                        this.selection.start = Some((line_number, start));
                        this.selection.end = Some((line_number, end));
                        this.selection.finish();
                    } else {
                        this.selection.start = Some((line_number, col));
                        this.selection.end = Some((line_number, col));
                        this.selection.is_selecting = true;
                    }
                    cx.notify();
                })
            })
            .on_mouse_move({
                let text_layout = text_layout.clone();
                cx.listener(move |this, event: &MouseMoveEvent, _window, cx| {
                    if this.selection.is_selecting {
                        let col = text_layout.index_for_position(event.position)
                            .unwrap_or_else(|ix| ix)
                            .min(line_len);
                        this.selection.end = Some((line_number, col));
                        cx.notify();
                    }
                })
            })
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _window, cx| {
                this.selection.finish();
                cx.notify();
            }))
            .child(
                // Line number gutter
                div()
                    .w(px(gutter_width))
                    .pr(px(10.0))
                    .text_color(rgba(t.text_muted, 0.6))
                    .flex()
                    .items_center()
                    .justify_end()
                    .flex_shrink_0()
                    .child(line_num_str)
                    // Subtle separator
                    .child(
                        div()
                            .ml(px(10.0))
                            .w(px(1.0))
                            .h(px(line_height * 0.6))
                            .bg(rgba(t.border, 0.3))
                            .flex_shrink_0(),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .pl(px(10.0))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .line_height(px(line_height))
                    .child(styled_text),
            )
    }

    /// Render visible lines for the virtualized list.
    pub(super) fn render_visible_lines(
        &self,
        range: std::ops::Range<usize>,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        range
            .filter_map(|i| {
                self.highlighted_lines
                    .get(i)
                    .map(|line| self.render_line(i, line, t, cx).into_any_element())
            })
            .collect()
    }

    /// Render the file tree sidebar.
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
                    .id("file-viewer-tree")
                    .flex_1()
                    .overflow_y_scroll()
                    .track_scroll(&self.tree_scroll_handle)
                    .py(px(6.0))
                    .children(tree_elements),
            )
    }

    /// Recursively render file tree nodes with expand/collapse.
    pub(super) fn render_tree_node(
        &self,
        node: &FileTreeNode,
        depth: usize,
        parent_path: &str,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut elements: Vec<AnyElement> = Vec::new();

        for (name, child) in &node.children {
            let folder_path = if parent_path.is_empty() {
                name.clone()
            } else {
                format!("{}/{}", parent_path, name)
            };
            let is_expanded = self.expanded_folders.contains(&folder_path);
            let indent = depth * 14;

            let folder_path_clone = folder_path.clone();
            elements.push(
                div()
                    .id(ElementId::Name(format!("fv-folder-{}", folder_path).into()))
                    .flex()
                    .items_center()
                    .h(px(26.0))
                    .pl(px(indent as f32 + 8.0))
                    .pr(px(12.0))
                    .mx(px(4.0))
                    .rounded(px(4.0))
                    .cursor_pointer()
                    .hover(|s| s.bg(rgb(t.bg_hover)))
                    .on_click(cx.listener(move |this, _, _window, cx| {
                        this.toggle_folder(&folder_path_clone, cx);
                    }))
                    .child(
                        // Chevron icon
                        svg()
                            .path(if is_expanded { "icons/chevron-down.svg" } else { "icons/chevron-right.svg" })
                            .size(px(14.0))
                            .text_color(rgb(t.text_muted))
                            .mr(px(4.0))
                            .flex_shrink_0(),
                    )
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(rgb(t.text_muted))
                            .child(format!("{}/", name)),
                    )
                    .into_any_element(),
            );

            if is_expanded {
                elements.extend(self.render_tree_node(child, depth + 1, &folder_path, t, cx));
            }
        }

        for &file_index in &node.files {
            if let Some(file) = self.files.get(file_index) {
                let indent = depth * 14;
                let is_selected = self.selected_file_index == Some(file_index);

                elements.push(
                    div()
                        .id(ElementId::Name(format!("fv-file-{}", file_index).into()))
                        .flex()
                        .items_center()
                        .h(px(26.0))
                        .pl(px(indent as f32 + 8.0 + 18.0)) // extra 18px to align past chevron
                        .pr(px(12.0))
                        .mx(px(4.0))
                        .rounded(px(4.0))
                        .cursor_pointer()
                        .when(is_selected, |d| d.bg(rgb(t.bg_selection)))
                        .hover(|s| s.bg(rgb(t.bg_hover)))
                        .on_click(cx.listener(move |this, _, _window, cx| {
                            this.select_file(file_index, cx);
                        }))
                        .child(
                            div()
                                .text_size(px(13.0))
                                .text_color(rgb(t.text_primary))
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .child(file.filename.clone()),
                        )
                        .into_any_element(),
                );
            }
        }

        elements
    }

    /// Render scrollbar thumb.
    pub(super) fn render_scrollbar(
        &self,
        t: &ThemeColors,
        thumb_y: f32,
        thumb_height: f32,
        is_dragging: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .id("file-viewer-scrollbar-track")
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
                    this.start_scrollbar_drag(y, cx);
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
}

impl Render for FileViewer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let focus_handle = self.focus_handle.clone();
        let has_error = self.error_message.is_some();
        let error_message = self.error_message.clone();
        let is_markdown = self.is_markdown;
        let display_mode = self.display_mode;
        let is_preview_mode = display_mode == DisplayMode::Preview;
        let sidebar_visible = self.sidebar_visible;

        let filename = self.file_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "File".to_string());

        let relative_path = self.file_path
            .strip_prefix(&self.project_path)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| self.file_path.to_string_lossy().to_string());

        // Measure actual monospace character width from font metrics
        let font = Font {
            family: "monospace".into(),
            weight: FontWeight::NORMAL,
            style: FontStyle::Normal,
            ..Default::default()
        };
        let font_size = self.file_font_size;
        let text_system = window.text_system();
        let font_id = text_system.resolve_font(&font);
        self.measured_char_width = text_system
            .advance(font_id, px(font_size), 'm')
            .map(|size| f32::from(size.width))
            .unwrap_or(font_size * 0.6);

        // Virtualization setup
        let line_count = self.line_count;
        let theme_colors = Arc::new(t.clone());
        let view = cx.entity().clone();
        let scrollbar_geometry = get_scrollbar_geometry(&self.source_scroll_handle);
        let is_dragging_scrollbar = self.scrollbar_drag.is_some();

        // Pre-render tree elements for sidebar
        let tree_elements = if sidebar_visible {
            self.render_tree_node(&self.file_tree.clone(), 0, "", &t, cx)
        } else {
            Vec::new()
        };

        // Pre-render markdown preview with selection - using per-node handlers
        let preview_nodes: Vec<RenderedNode> = if !has_error && is_preview_mode && is_markdown {
            self.markdown_doc.as_ref().map(|doc| {
                let selection = self.markdown_selection.normalized_non_empty();
                doc.render_nodes_with_offsets(&t, selection)
            }).unwrap_or_default()
        } else {
            Vec::new()
        };
        // Focus on first render
        if !focus_handle.is_focused(window) {
            window.focus(&focus_handle, cx);
        }

        modal_backdrop("file-viewer-backdrop", &t)
            .track_focus(&focus_handle)
            .key_context("FileViewer")
            .items_center()
            .on_action(cx.listener(|this, _: &Cancel, _window, cx| {
                let is_preview = this.display_mode == DisplayMode::Preview;
                if is_preview && this.markdown_selection.normalized_non_empty().is_some() {
                    this.markdown_selection.clear();
                    cx.notify();
                } else if this.selection.normalized_non_empty().is_some() {
                    this.selection.clear();
                    cx.notify();
                } else {
                    this.close(cx);
                }
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                let key = event.keystroke.key.as_str();
                let modifiers = &event.keystroke.modifiers;
                let is_preview = this.display_mode == DisplayMode::Preview;

                match key {
                    "tab" if this.is_markdown => {
                        this.toggle_display_mode(cx);
                    }
                    "b" if !modifiers.platform && !modifiers.control => {
                        this.toggle_sidebar(cx);
                    }
                    "c" if modifiers.platform || modifiers.control => {
                        if is_preview {
                            this.copy_markdown_selection(cx);
                        } else {
                            this.copy_selection(cx);
                        }
                    }
                    "a" if modifiers.platform || modifiers.control => {
                        if is_preview {
                            this.select_all_markdown(cx);
                        } else {
                            this.select_all(cx);
                        }
                    }
                    _ => {}
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _window, cx| {
                    // Don't close if scrollbar is being dragged
                    if this.scrollbar_drag.is_none() {
                        this.close(cx);
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
                cx.listener(|this, _, _window, cx| {
                    if this.scrollbar_drag.is_some() {
                        this.end_scrollbar_drag(cx);
                    }
                }),
            )
            .child(
                modal_content("file-viewer-modal", &t)
                    // Larger modal - 90% width, 85% height with max bounds
                    .w(relative(0.9))
                    .max_w(px(1200.0))
                    .h(relative(0.85))
                    .max_h(px(900.0))
                    .when(!is_preview_mode, |d| d.cursor(CursorStyle::IBeam))
                    // Custom header with toggle for markdown files
                    .child(
                        div()
                            .px(px(16.0))
                            .py(px(12.0))
                            .border_b_1()
                            .border_color(rgb(t.border))
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                // Left side: sidebar toggle + filename and path
                                h_flex()
                                    .gap(px(10.0))
                                    .child(
                                        div()
                                            .id("sidebar-toggle")
                                            .cursor_pointer()
                                            .w(px(28.0))
                                            .h(px(28.0))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .rounded(px(6.0))
                                            .bg(rgb(if sidebar_visible { t.bg_selection } else { t.bg_secondary }))
                                            .hover(|s| s.bg(rgb(t.bg_hover)))
                                            .on_click(cx.listener(|this, _, _window, cx| {
                                                this.toggle_sidebar(cx);
                                            }))
                                            .child(
                                                svg()
                                                    .path("icons/chevron-right.svg")
                                                    .size(px(14.0))
                                                    .text_color(rgb(t.text_muted)),
                                            ),
                                    )
                                    .child(
                                        v_flex()
                                            .gap(px(2.0))
                                            .child(
                                                div()
                                                    .text_size(px(14.0))
                                                    .font_weight(FontWeight::MEDIUM)
                                                    .text_color(rgb(t.text_primary))
                                                    .child(filename),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(11.0))
                                                    .text_color(rgb(t.text_muted))
                                                    .child(relative_path),
                                            ),
                                    ),
                            )
                            .child(
                                // Right side: toggle (for markdown) and close button
                                h_flex()
                                    .gap(px(12.0))
                                    .when(is_markdown, |d| {
                                        d.child(
                                            div()
                                                .id("display-mode-toggle")
                                                .on_click(cx.listener(|this, _, _window, cx| {
                                                    this.toggle_display_mode(cx);
                                                }))
                                                .child(segmented_toggle(
                                                    &[
                                                        ("Preview", is_preview_mode),
                                                        ("Source", !is_preview_mode),
                                                    ],
                                                    &t,
                                                ))
                                        )
                                    })
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
                                                    .child("\u{00d7}"),
                                            ),
                                    ),
                            ),
                    )
                    // Main content area: sidebar + content + footer
                    .child(
                        h_flex()
                            .flex_1()
                            .min_h_0()
                            // Sidebar (when visible)
                            .when(sidebar_visible, |d| {
                                d.child(self.render_sidebar(&t, tree_elements))
                            })
                            // Content + footer column
                            .child(
                                v_flex()
                                    .flex_1()
                                    .h_full()
                                    .min_h_0()
                                    .min_w_0()
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
                                    // Source view (virtualized, syntax highlighted)
                                    .when(!has_error && !is_preview_mode, |d| {
                                        let tc = theme_colors.clone();
                                        let view_clone = view.clone();
                                        d.child(
                                            div()
                                                .id("file-content")
                                                .flex_1()
                                                .min_h_0()
                                                .relative()
                                                .child(
                                                    uniform_list("file-lines", line_count, move |range, _window, cx| {
                                                        let tc = tc.clone();
                                                        view_clone.update(cx, |this, cx| {
                                                            this.render_visible_lines(range, &tc, cx)
                                                        })
                                                    })
                                                    .size_full()
                                                    .bg(rgb(t.bg_secondary))
                                                    .cursor(CursorStyle::IBeam)
                                                    .track_scroll(&self.source_scroll_handle),
                                                )
                                                .when(scrollbar_geometry.is_some(), |d| {
                                                    let (_, _, thumb_y, thumb_height) = scrollbar_geometry.unwrap();
                                                    d.child(self.render_scrollbar(&t, thumb_y, thumb_height, is_dragging_scrollbar, cx))
                                                }),
                                        )
                                    })
                                    // Preview view (rendered markdown) - with per-node selection handlers
                                    .when(!has_error && is_preview_mode, |d| {
                                        // Build content with per-node/line handlers
                                        let mut content_children: Vec<AnyElement> = Vec::new();
                                        let mut node_idx = 0usize;

                                        for rendered_node in preview_nodes {
                                            match rendered_node {
                                                RenderedNode::Simple { div: node_div, start_offset, end_offset } => {
                                                    let node_end = end_offset.saturating_sub(1);
                                                    let idx = node_idx;
                                                    content_children.push(
                                                        div()
                                                            .id(ElementId::Name(format!("md-node-{}", idx).into()))
                                                            .w_full()
                                                            .on_mouse_down(MouseButton::Left, cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                                                                if event.click_count == 2 {
                                                                    this.markdown_selection.start = Some(start_offset);
                                                                    this.markdown_selection.end = Some(node_end);
                                                                    this.markdown_selection.finish();
                                                                } else {
                                                                    this.markdown_selection.start = Some(start_offset);
                                                                    this.markdown_selection.end = Some(start_offset);
                                                                    this.markdown_selection.is_selecting = true;
                                                                }
                                                                cx.notify();
                                                            }))
                                                            .on_mouse_move(cx.listener(move |this, _event: &MouseMoveEvent, _window, cx| {
                                                                if this.markdown_selection.is_selecting {
                                                                    if let Some(sel_start) = this.markdown_selection.start {
                                                                        if start_offset >= sel_start {
                                                                            this.markdown_selection.end = Some(node_end);
                                                                        } else {
                                                                            this.markdown_selection.end = Some(start_offset);
                                                                        }
                                                                        cx.notify();
                                                                    }
                                                                }
                                                            }))
                                                            .on_mouse_up(MouseButton::Left, cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                                                                this.markdown_selection.finish();
                                                                cx.notify();
                                                            }))
                                                            .child(node_div)
                                                            .into_any_element()
                                                    );
                                                    node_idx += 1;
                                                }
                                                RenderedNode::CodeBlock { language, lines, .. } => {
                                                    let idx = node_idx;
                                                    let line_children: Vec<AnyElement> = lines.into_iter().enumerate().map(|(line_idx, (line_div, start_offset, end_offset))| {
                                                        let line_end = end_offset.saturating_sub(1);
                                                        div()
                                                            .id(ElementId::Name(format!("md-code-{}-line-{}", idx, line_idx).into()))
                                                            .on_mouse_down(MouseButton::Left, cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                                                                if event.click_count == 2 {
                                                                    this.markdown_selection.start = Some(start_offset);
                                                                    this.markdown_selection.end = Some(line_end);
                                                                    this.markdown_selection.finish();
                                                                } else {
                                                                    this.markdown_selection.start = Some(start_offset);
                                                                    this.markdown_selection.end = Some(start_offset);
                                                                    this.markdown_selection.is_selecting = true;
                                                                }
                                                                cx.notify();
                                                            }))
                                                            .on_mouse_move(cx.listener(move |this, _event: &MouseMoveEvent, _window, cx| {
                                                                if this.markdown_selection.is_selecting {
                                                                    if let Some(sel_start) = this.markdown_selection.start {
                                                                        if start_offset >= sel_start {
                                                                            this.markdown_selection.end = Some(line_end);
                                                                        } else {
                                                                            this.markdown_selection.end = Some(start_offset);
                                                                        }
                                                                        cx.notify();
                                                                    }
                                                                }
                                                            }))
                                                            .on_mouse_up(MouseButton::Left, cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                                                                this.markdown_selection.finish();
                                                                cx.notify();
                                                            }))
                                                            .child(line_div)
                                                            .into_any_element()
                                                    }).collect();

                                                    let code_block = code_block_container(language.as_deref(), &t)
                                                        .id(ElementId::Name(format!("md-codeblock-{}", idx).into()))
                                                        .child(
                                                            div()
                                                                .p(px(12.0))
                                                                .font_family("monospace")
                                                                .text_size(px(self.file_font_size))
                                                                .text_color(rgb(t.text_secondary))
                                                                .flex()
                                                                .flex_col()
                                                                .children(line_children)
                                                        );

                                                    content_children.push(code_block.into_any_element());
                                                    node_idx += 1;
                                                }
                                                RenderedNode::Table { header, rows } => {
                                                    let idx = node_idx;
                                                    let mut table_rows: Vec<AnyElement> = Vec::new();

                                                    if let Some((header_div, start_offset, end_offset)) = header {
                                                        let row_end = end_offset.saturating_sub(1);
                                                        table_rows.push(
                                                            div()
                                                                .id(ElementId::Name(format!("md-table-{}-header", idx).into()))
                                                                .on_mouse_down(MouseButton::Left, cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                                                                    if event.click_count == 2 {
                                                                        this.markdown_selection.start = Some(start_offset);
                                                                        this.markdown_selection.end = Some(row_end);
                                                                        this.markdown_selection.finish();
                                                                    } else {
                                                                        this.markdown_selection.start = Some(start_offset);
                                                                        this.markdown_selection.end = Some(start_offset);
                                                                        this.markdown_selection.is_selecting = true;
                                                                    }
                                                                    cx.notify();
                                                                }))
                                                                .on_mouse_move(cx.listener(move |this, _event: &MouseMoveEvent, _window, cx| {
                                                                    if this.markdown_selection.is_selecting {
                                                                        if let Some(sel_start) = this.markdown_selection.start {
                                                                            if start_offset >= sel_start {
                                                                                this.markdown_selection.end = Some(row_end);
                                                                            } else {
                                                                                this.markdown_selection.end = Some(start_offset);
                                                                            }
                                                                            cx.notify();
                                                                        }
                                                                    }
                                                                }))
                                                                .on_mouse_up(MouseButton::Left, cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                                                                    this.markdown_selection.finish();
                                                                    cx.notify();
                                                                }))
                                                                .child(header_div)
                                                                .into_any_element()
                                                        );
                                                    }

                                                    for (row_idx, (row_div, start_offset, end_offset)) in rows.into_iter().enumerate() {
                                                        let row_end = end_offset.saturating_sub(1);
                                                        table_rows.push(
                                                            div()
                                                                .id(ElementId::Name(format!("md-table-{}-row-{}", idx, row_idx).into()))
                                                                .on_mouse_down(MouseButton::Left, cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                                                                    if event.click_count == 2 {
                                                                        this.markdown_selection.start = Some(start_offset);
                                                                        this.markdown_selection.end = Some(row_end);
                                                                        this.markdown_selection.finish();
                                                                    } else {
                                                                        this.markdown_selection.start = Some(start_offset);
                                                                        this.markdown_selection.end = Some(start_offset);
                                                                        this.markdown_selection.is_selecting = true;
                                                                    }
                                                                    cx.notify();
                                                                }))
                                                                .on_mouse_move(cx.listener(move |this, _event: &MouseMoveEvent, _window, cx| {
                                                                    if this.markdown_selection.is_selecting {
                                                                        if let Some(sel_start) = this.markdown_selection.start {
                                                                            if start_offset >= sel_start {
                                                                                this.markdown_selection.end = Some(row_end);
                                                                            } else {
                                                                                this.markdown_selection.end = Some(start_offset);
                                                                            }
                                                                            cx.notify();
                                                                        }
                                                                    }
                                                                }))
                                                                .on_mouse_up(MouseButton::Left, cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                                                                    this.markdown_selection.finish();
                                                                    cx.notify();
                                                                }))
                                                                .child(row_div)
                                                                .into_any_element()
                                                        );
                                                    }

                                                    let table = div()
                                                        .id(ElementId::Name(format!("md-table-{}", idx).into()))
                                                        .flex()
                                                        .flex_col()
                                                        .rounded(px(4.0))
                                                        .border_1()
                                                        .border_color(rgb(t.border))
                                                        .overflow_hidden()
                                                        .children(table_rows);

                                                    content_children.push(table.into_any_element());
                                                    node_idx += 1;
                                                }
                                            }
                                        }

                                        let content_div = v_flex()
                                            .gap(px(12.0))
                                            .p(px(16.0))
                                            .max_w(px(900.0))
                                            .children(content_children);

                                        d.child(
                                            div()
                                                .id("markdown-preview")
                                                .flex_1()
                                                .overflow_y_scroll()
                                                .overflow_x_scroll()
                                                .track_scroll(&self.markdown_scroll_handle)
                                                .bg(rgb(t.bg_secondary))
                                                .cursor(CursorStyle::IBeam)
                                                .on_mouse_up(MouseButton::Left, cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                                                    this.markdown_selection.finish();
                                                    cx.notify();
                                                }))
                                                .child(content_div)
                                        )
                                    })
                                    // Footer with hints
                                    .child(
                                        div()
                                            .px(px(12.0))
                                            .py(px(8.0))
                                            .border_t_1()
                                            .border_color(rgb(t.border))
                                            .flex()
                                            .items_center()
                                            .justify_between()
                                            .child(
                                                h_flex()
                                                    .gap(px(16.0))
                                                    .child(self.render_hint("B", "files", &t))
                                                    .when(is_markdown, |d| {
                                                        d.child(self.render_hint("Tab", "toggle preview", &t))
                                                    })
                                                    .child(self.render_hint(
                                                        if cfg!(target_os = "macos") { "Cmd+C" } else { "Ctrl+C" },
                                                        "copy",
                                                        &t,
                                                    ))
                                                    .child(self.render_hint(
                                                        if cfg!(target_os = "macos") { "Cmd+A" } else { "Ctrl+A" },
                                                        "select all",
                                                        &t,
                                                    ))
                                                    .child(self.render_hint("Esc", "close", &t)),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(10.0))
                                                    .text_color(rgb(t.text_muted))
                                                    .when(!is_preview_mode, |d| {
                                                        d.child(format!("{} lines", self.line_count))
                                                    })
                                                    .when(is_preview_mode, |d| {
                                                        d.child("Preview mode")
                                                    }),
                                            ),
                                    ),
                            ),
                    ),
            )
    }
}

impl FileViewer {
    fn render_hint(
        &self,
        key: &str,
        action: &str,
        t: &ThemeColors,
    ) -> impl IntoElement {
        h_flex()
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
}
