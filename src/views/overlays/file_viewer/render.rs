//! Rendering logic for the file viewer overlay.

use crate::keybindings::Cancel;
use crate::theme::{theme, ThemeColors};
use crate::ui::{Selection1DExtension, Selection2DExtension};
use crate::views::components::{
    get_scrollbar_geometry, modal_backdrop, modal_content, segmented_toggle, HighlightedLine,
};
use super::markdown_renderer::RenderedNode;
use super::{DisplayMode, FileViewer};
use gpui::*;
use gpui::prelude::*;
use std::sync::Arc;

impl FileViewer {
    /// Render a single highlighted line with selection support.
    pub(super) fn render_line(&self, line_number: usize, line: &HighlightedLine, t: &ThemeColors, cx: &mut Context<Self>) -> Stateful<Div> {
        // Format line number with right padding
        let line_num_str = format!("{:>width$}", line_number + 1, width = self.line_num_width);
        let has_selection = self.selection.line_has_selection(line_number);
        let line_num_width = self.line_num_width;

        // Selection highlight color
        let selection_bg = Rgba {
            r: 0.2,
            g: 0.4,
            b: 0.7,
            a: 0.4,
        };

        let font_size = self.file_font_size;
        let line_height = font_size * 1.5;

        div()
            .id(ElementId::Name(format!("line-{}", line_number).into()))
            .flex()
            .h(px(line_height))
            .text_size(px(font_size))
            .font_family("monospace")
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                let col = this.x_to_column(f32::from(event.position.x), line_num_width);
                this.selection.start = Some((line_number, col));
                this.selection.end = Some((line_number, col));
                this.selection.is_selecting = true;
                cx.notify();
            }))
            .on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, _window, cx| {
                if this.selection.is_selecting {
                    let col = this.x_to_column(f32::from(event.position.x), line_num_width);
                    this.selection.end = Some((line_number, col));
                    cx.notify();
                }
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _window, cx| {
                this.selection.finish();
                cx.notify();
            }))
            .child(
                // Line number gutter
                div()
                    .w(px((self.line_num_width * 8 + 16) as f32))
                    .pr(px(8.0))
                    .text_color(rgb(t.text_muted))
                    .flex()
                    .justify_end()
                    .flex_shrink_0()
                    .child(line_num_str),
            )
            .child(
                // Line content with syntax highlighting and selection
                if has_selection {
                    // Render with selection highlighting
                    self.render_line_with_selection(line_number, line, t, selection_bg)
                } else {
                    // Simple render without selection
                    div()
                        .flex_1()
                        .flex()
                        .overflow_hidden()
                        .children(
                            line.spans.iter().map(|span| {
                                div()
                                    .text_color(span.color)
                                    .child(span.text.clone())
                            }),
                        )
                },
            )
    }

    /// Render a line with selection highlighting.
    fn render_line_with_selection(
        &self,
        line_number: usize,
        line: &HighlightedLine,
        _t: &ThemeColors,
        selection_bg: Rgba,
    ) -> Div {
        let ((start_line, start_col), (end_line, end_col)) = match self.selection.normalized() {
            Some(range) => range,
            None => {
                return div()
                    .flex_1()
                    .flex()
                    .overflow_hidden()
                    .children(
                        line.spans.iter().map(|span| {
                            div()
                                .text_color(span.color)
                                .child(span.text.clone())
                        }),
                    );
            }
        };

        // Determine selection bounds for this line
        let line_len = line.plain_text.len();
        let sel_start = if line_number == start_line { start_col.min(line_len) } else { 0 };
        let sel_end = if line_number == end_line { end_col.min(line_len) } else { line_len };

        // Build character-level rendering with selection
        let mut elements: Vec<Div> = Vec::new();
        let mut current_col = 0;

        for span in &line.spans {
            let span_len = span.text.len();
            let span_end = current_col + span_len;

            // Check if this span intersects with selection
            let span_sel_start = sel_start.max(current_col);
            let span_sel_end = sel_end.min(span_end);

            if span_sel_start < span_sel_end && span_sel_start < span_end && span_sel_end > current_col {
                // Span has some selection - split into parts
                let rel_sel_start = span_sel_start - current_col;
                let rel_sel_end = span_sel_end - current_col;

                // Before selection
                if rel_sel_start > 0 {
                    elements.push(
                        div()
                            .text_color(span.color)
                            .child(span.text[..rel_sel_start].to_string())
                    );
                }

                // Selected part
                elements.push(
                    div()
                        .bg(selection_bg)
                        .text_color(span.color)
                        .child(span.text[rel_sel_start..rel_sel_end].to_string())
                );

                // After selection
                if rel_sel_end < span_len {
                    elements.push(
                        div()
                            .text_color(span.color)
                            .child(span.text[rel_sel_end..].to_string())
                    );
                }
            } else {
                // No selection in this span
                elements.push(
                    div()
                        .text_color(span.color)
                        .child(span.text.clone())
                );
            }

            current_col = span_end;
        }

        div()
            .flex_1()
            .flex()
            .overflow_hidden()
            .children(elements)
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
        let has_selection = self.selection.normalized().is_some();
        let is_markdown = self.is_markdown;
        let display_mode = self.display_mode;
        let is_preview_mode = display_mode == DisplayMode::Preview;

        let filename = self.file_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "File".to_string());

        let relative_path = self.file_path.to_string_lossy().to_string();

        // Virtualization setup
        let line_count = self.line_count;
        let theme_colors = Arc::new(t.clone());
        let view = cx.entity().clone();
        let scrollbar_geometry = get_scrollbar_geometry(&self.source_scroll_handle);
        let is_dragging_scrollbar = self.scrollbar_drag.is_some();

        // Pre-render markdown preview with selection - using per-node handlers
        let preview_nodes: Vec<RenderedNode> = if !has_error && is_preview_mode && is_markdown {
            self.markdown_doc.as_ref().map(|doc| {
                let selection = self.markdown_selection.normalized_non_empty();
                doc.render_nodes_with_offsets(&t, selection)
            }).unwrap_or_default()
        } else {
            Vec::new()
        };
        let has_markdown_selection = self.markdown_selection.normalized_non_empty().is_some();

        // Focus on first render
        window.focus(&focus_handle, cx);

        modal_backdrop("file-viewer-backdrop", &t)
            .track_focus(&focus_handle)
            .key_context("FileViewer")
            .items_center()
            .on_action(cx.listener(|this, _: &Cancel, _window, cx| {
                let is_preview = this.display_mode == DisplayMode::Preview;
                if is_preview && this.markdown_selection.normalized_non_empty().is_some() {
                    this.markdown_selection.clear();
                    cx.notify();
                } else if this.selection.normalized().is_some() {
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
                                // Left side: filename and path
                                div()
                                    .flex()
                                    .flex_col()
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
                            )
                            .child(
                                // Right side: toggle (for markdown) and close button
                                div()
                                    .flex()
                                    .items_center()
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
                                    // Block-level selection for simple nodes
                                    let node_end = end_offset.saturating_sub(1);
                                    let idx = node_idx;
                                    content_children.push(
                                        div()
                                            .id(ElementId::Name(format!("md-node-{}", idx).into()))
                                            .w_full()
                                            .on_mouse_down(MouseButton::Left, cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                                                if event.click_count == 2 {
                                                    // Double-click: select entire block
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
                                    // Code block with per-line selection
                                    let lang_label = language.as_deref().unwrap_or("");
                                    let idx = node_idx;

                                    // Build lines with handlers
                                    let line_children: Vec<AnyElement> = lines.into_iter().enumerate().map(|(line_idx, (line_div, start_offset, end_offset))| {
                                        let line_end = end_offset.saturating_sub(1); // Exclude newline
                                        div()
                                            .id(ElementId::Name(format!("md-code-{}-line-{}", idx, line_idx).into()))
                                            .on_mouse_down(MouseButton::Left, cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                                                if event.click_count == 2 {
                                                    // Double-click: select entire line
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

                                    // Build code block container
                                    let code_block = div()
                                        .id(ElementId::Name(format!("md-codeblock-{}", idx).into()))
                                        .flex()
                                        .flex_col()
                                        .rounded(px(6.0))
                                        .bg(rgb(t.bg_primary))
                                        .border_1()
                                        .border_color(rgb(t.border))
                                        .overflow_hidden()
                                        .when(!lang_label.is_empty(), |d| {
                                            d.child(
                                                div()
                                                    .px(px(12.0))
                                                    .py(px(4.0))
                                                    .bg(rgb(t.bg_header))
                                                    .border_b_1()
                                                    .border_color(rgb(t.border))
                                                    .text_size(px(10.0))
                                                    .text_color(rgb(t.text_muted))
                                                    .child(lang_label.to_string())
                                            )
                                        })
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
                                    // Table with per-row selection
                                    let idx = node_idx;

                                    let mut table_rows: Vec<AnyElement> = Vec::new();

                                    // Header row with handler
                                    if let Some((header_div, start_offset, end_offset)) = header {
                                        let row_end = end_offset.saturating_sub(1);
                                        table_rows.push(
                                            div()
                                                .id(ElementId::Name(format!("md-table-{}-header", idx).into()))
                                                .on_mouse_down(MouseButton::Left, cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                                                    if event.click_count == 2 {
                                                        // Double-click: select entire row
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

                                    // Data rows with handlers
                                    for (row_idx, (row_div, start_offset, end_offset)) in rows.into_iter().enumerate() {
                                        let row_end = end_offset.saturating_sub(1);
                                        table_rows.push(
                                            div()
                                                .id(ElementId::Name(format!("md-table-{}-row-{}", idx, row_idx).into()))
                                                .on_mouse_down(MouseButton::Left, cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                                                    if event.click_count == 2 {
                                                        // Double-click: select entire row
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

                                    // Build table container
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

                        let content_div = div()
                            .flex()
                            .flex_col()
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
                                // Global mouse up to handle case where mouse up happens outside a node
                                .on_mouse_up(MouseButton::Left, cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                                    this.markdown_selection.finish();
                                    cx.notify();
                                }))
                                .child(content_div)
                        )
                    })
                    .child(
                        // Footer with hints
                        div()
                            .px(px(12.0))
                            .py(px(8.0))
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
                                    // Tab toggle (only for markdown)
                                    .when(is_markdown, |d| {
                                        d.child(
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
                                                        .child("Tab"),
                                                )
                                                .child(
                                                    div()
                                                        .text_size(px(10.0))
                                                        .text_color(rgb(t.text_muted))
                                                        .child("toggle preview"),
                                                ),
                                        )
                                    })
                                    // Copy
                                    .child(
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
                                                    .child(if cfg!(target_os = "macos") { "Cmd+C" } else { "Ctrl+C" }),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(10.0))
                                                    .text_color(rgb(t.text_muted))
                                                    .child("copy"),
                                            ),
                                    )
                                    // Select all
                                    .child(
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
                                                    .child(if cfg!(target_os = "macos") { "Cmd+A" } else { "Ctrl+A" }),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(10.0))
                                                    .text_color(rgb(t.text_muted))
                                                    .child("select all"),
                                            ),
                                    )
                                    .child(
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
                                                    .child("Esc"),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(10.0))
                                                    .text_color(rgb(t.text_muted))
                                                    .child("close"),
                                            ),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(10.0))
                                    .text_color(rgb(t.text_muted))
                                    .when(has_selection && !is_preview_mode, |d| {
                                        d.child("Selection active")
                                    })
                                    .when(!has_selection && !is_preview_mode, |d| {
                                        d.child(format!("{} lines", self.line_count))
                                    })
                                    .when(is_preview_mode && has_markdown_selection, |d| {
                                        d.child("Selection active")
                                    })
                                    .when(is_preview_mode && !has_markdown_selection, |d| {
                                        d.child("Preview mode")
                                    }),
                            ),
                    ),
            )
    }
}
