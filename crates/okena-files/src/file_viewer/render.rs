//! Rendering logic for the file viewer overlay.

use crate::code_view::{
    build_styled_text_with_backgrounds, find_word_boundaries, get_scrollbar_geometry,
    selection_bg_ranges,
};
use crate::file_search::Cancel;
use crate::file_tree::{expandable_file_row, expandable_folder_row, FileTreeNode};
use crate::selection::{Selection1DExtension, Selection2DNonEmpty};
use crate::syntax::HighlightedLine;
use crate::theme::theme;
use gpui::prelude::*;
use gpui::*;
use gpui_component::{h_flex, v_flex};
use okena_core::theme::ThemeColors;
use okena_markdown::RenderedNode;
use okena_ui::code_block::code_block_container;
use okena_ui::modal::fullscreen_overlay;
use okena_ui::toggle::segmented_toggle;
use okena_ui::file_icon::file_icon;
use okena_ui::tokens::{ui_text, ui_text_md, ui_text_ms, ui_text_sm, ui_text_xl};
use std::sync::Arc;

use super::context_menu::TreeNodeTarget;
use super::{DisplayMode, FileViewer, SIDEBAR_WIDTH};

/// Helper to create rgba from u32 color and alpha.
fn rgba(color: u32, alpha: f32) -> Rgba {
    let r = ((color >> 16) & 0xFF) as f32 / 255.0;
    let g = ((color >> 8) & 0xFF) as f32 / 255.0;
    let b = (color & 0xFF) as f32 / 255.0;
    Rgba { r, g, b, a: alpha }
}

impl FileViewer {
    /// Render a single highlighted line with selection support.
    pub(super) fn render_line(
        &self,
        line_number: usize,
        line: &HighlightedLine,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let tab = self.active_tab();
        let line_num_str = format!("{:>width$}", line_number + 1, width = tab.line_num_width);

        let font_size = self.file_font_size;
        let line_height = font_size * 1.8;
        let char_width = self.measured_char_width;
        let gutter_width = (tab.line_num_width as f32) * char_width + 16.0;

        let bg_ranges = selection_bg_ranges(&tab.selection, line_number, line.plain_text.len());

        let plain_text = line.plain_text.clone();
        let line_len = line.plain_text.len();

        let styled_text = build_styled_text_with_backgrounds(&line.spans, &bg_ranges);
        let text_layout = styled_text.layout().clone();

        div()
            .id(ElementId::Name(format!("line-{}", line_number).into()))
            .w_full()
            .flex()
            .h(px(line_height))
            .text_size(ui_text(font_size, cx))
            .font_family("monospace")
            .on_mouse_down(MouseButton::Left, {
                let text_layout = text_layout.clone();
                let plain_text = plain_text.clone();
                cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                    let tab = this.active_tab_mut();
                    let col = text_layout
                        .index_for_position(event.position)
                        .unwrap_or_else(|ix| ix)
                        .min(line_len);
                    if event.click_count >= 3 {
                        tab.selection.start = Some((line_number, 0));
                        tab.selection.end = Some((line_number, line_len));
                        tab.selection.finish();
                    } else if event.click_count == 2 {
                        let (start, end) = find_word_boundaries(&plain_text, col);
                        tab.selection.start = Some((line_number, start));
                        tab.selection.end = Some((line_number, end));
                        tab.selection.finish();
                    } else {
                        tab.selection.start = Some((line_number, col));
                        tab.selection.end = Some((line_number, col));
                        tab.selection.is_selecting = true;
                    }
                    cx.notify();
                })
            })
            .on_mouse_move({
                let text_layout = text_layout.clone();
                cx.listener(move |this, event: &MouseMoveEvent, _window, cx| {
                    let tab = this.active_tab_mut();
                    if tab.selection.is_selecting {
                        let col = text_layout
                            .index_for_position(event.position)
                            .unwrap_or_else(|ix| ix)
                            .min(line_len);
                        tab.selection.end = Some((line_number, col));
                        cx.notify();
                    }
                })
            })
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _window, cx| {
                    this.active_tab_mut().selection.finish();
                    cx.notify();
                }),
            )
            .child(
                div()
                    .w(px(gutter_width))
                    .pr(px(10.0))
                    .text_color(rgba(t.text_muted, 0.6))
                    .flex()
                    .items_center()
                    .justify_end()
                    .flex_shrink_0()
                    .child(line_num_str)
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
        let tab = self.active_tab();
        range
            .filter_map(|i| {
                tab.highlighted_lines
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
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let active_count = self.show_ignored as u8 + self.show_hidden as u8;
        let is_open = self.filter_popover_open;

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
                    .py(px(10.0))
                    .border_b_1()
                    .border_color(rgb(t.border))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(ui_text_ms(cx))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(t.text_secondary))
                            .line_height(px(11.0))
                            .child("Files"),
                    )
                    .child({
                        let entity = cx.entity().downgrade();
                        let entity2 = entity.clone();
                        crate::list_overlay::file_filter_button(
                            "fv-filter-btn", active_count, t, cx,
                            move |_, _, cx| {
                                if let Some(e) = entity.upgrade() {
                                    e.update(cx, |this, cx| {
                                        this.filter_popover_open = !this.filter_popover_open;
                                        cx.notify();
                                    });
                                }
                            },
                            move |bounds, _, cx| {
                                if let Some(e) = entity2.upgrade() {
                                    e.update(cx, |this, _| this.filter_button_bounds = Some(bounds));
                                }
                            },
                        )
                    }),
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
        let active_file_index = self.active_tab().selected_file_index;
        // Collect all file indices that have open tabs (for dimmer highlight)
        let open_file_indices: std::collections::HashSet<usize> = self
            .tabs
            .iter()
            .filter_map(|t| t.selected_file_index)
            .collect();

        for (name, child) in &node.children {
            let folder_path = if parent_path.is_empty() {
                name.clone()
            } else {
                format!("{}/{}", parent_path, name)
            };
            let is_expanded = self.expanded_folders.contains(&folder_path);
            let is_renaming = self.is_renaming_folder(&folder_path);
            let is_ctx_target = self.is_context_menu_target_folder(&folder_path);

            let indent = depth as f32 * 14.0;

            if is_renaming {
                // Build folder row with inline rename input instead of name label
                let mut row = div()
                    .id(ElementId::Name(
                        format!("fv-folder-{}-rename", folder_path).into(),
                    ))
                    .flex()
                    .items_center()
                    .h(px(26.0))
                    .pl(px(indent + 8.0))
                    .pr(px(12.0))
                    .mx(px(4.0))
                    .rounded(px(4.0))
                    .bg(rgb(t.bg_selection))
                    .child(
                        svg()
                            .path(if is_expanded { "icons/chevron-down.svg" } else { "icons/chevron-right.svg" })
                            .size(px(14.0))
                            .text_color(rgb(t.text_muted))
                            .mr(px(4.0))
                            .flex_shrink_0(),
                    )
                    .child(
                        svg()
                            .path("icons/folder.svg")
                            .size(px(14.0))
                            .text_color(rgb(t.text_secondary))
                            .mr(px(4.0))
                            .flex_shrink_0(),
                    );
                if let Some(input) = self.render_rename_input(t, cx) {
                    row = row.child(input);
                }
                row = row.on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                    if event.keystroke.key.as_str() == "enter" {
                        this.finish_rename(cx);
                    }
                }));
                elements.push(row.into_any_element());
            } else {
                let folder_path_clone = folder_path.clone();
                let folder_path_for_ctx = folder_path.clone();
                let abs_path_for_ctx = self.project_path.join(&folder_path);

                elements.push(
                    expandable_folder_row(name, depth, is_expanded, t, cx)
                        .id(ElementId::Name(
                            format!("fv-folder-{}", folder_path).into(),
                        ))
                        .when(is_ctx_target, |d| d.bg(rgb(t.bg_selection)))
                        .on_click(cx.listener(move |this, _, _window, cx| {
                            this.toggle_folder(&folder_path_clone, cx);
                        }))
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener({
                                let folder_path = folder_path_for_ctx;
                                let abs_path = abs_path_for_ctx;
                                move |this, event: &MouseDownEvent, _, cx| {
                                    this.open_context_menu(
                                        event.position,
                                        TreeNodeTarget::Folder {
                                            folder_path: folder_path.clone(),
                                            abs_path: abs_path.clone(),
                                        },
                                        cx,
                                    );
                                    cx.stop_propagation();
                                }
                            }),
                        )
                        .into_any_element(),
                );
            }

            if is_expanded {
                elements.extend(self.render_tree_node(child, depth + 1, &folder_path, t, cx));
            }
        }

        for &file_index in &node.files {
            if let Some(file) = self.files.get(file_index) {
                let is_active = active_file_index == Some(file_index);
                let is_open = open_file_indices.contains(&file_index);
                let is_renaming = self.is_renaming_file(&file.path);
                let is_ctx_target = self.is_context_menu_target_file(&file.path);

                let highlight = is_active || is_ctx_target;
                let indent = depth as f32 * 14.0;

                if is_renaming {
                    // Build file row with inline rename input instead of name label
                    let mut row = div()
                        .id(ElementId::Name(format!("fv-file-{}-rename", file_index).into()))
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .h(px(26.0))
                        .pl(px(indent + 8.0 + 18.0))
                        .pr(px(12.0))
                        .mx(px(4.0))
                        .rounded(px(4.0))
                        .bg(rgb(t.bg_selection))
                        .child(file_icon(&file.filename, t, cx).mr(px(4.0)));
                    if let Some(input) = self.render_rename_input(t, cx) {
                        row = row.child(input);
                    }
                    row = row.on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                        if event.keystroke.key.as_str() == "enter" {
                            this.finish_rename(cx);
                        }
                    }));
                    elements.push(row.into_any_element());
                } else {
                    let file_path_for_ctx = file.path.clone();
                    elements.push(
                        expandable_file_row(&file.filename, depth, None, t, cx)
                            .id(ElementId::Name(format!("fv-file-{}", file_index).into()))
                            .when(highlight, |d| d.bg(rgb(t.bg_selection)))
                            .when(!highlight && is_open, |d| {
                                d.bg(rgba(t.bg_selection, 0.4))
                            })
                            .on_click(cx.listener(move |this, _, _window, cx| {
                                this.select_file(file_index, cx);
                            }))
                            .on_mouse_down(
                                MouseButton::Right,
                                cx.listener({
                                    let path = file_path_for_ctx;
                                    move |this, event: &MouseDownEvent, _, cx| {
                                        this.open_context_menu(
                                            event.position,
                                            TreeNodeTarget::File {
                                                path: path.clone(),
                                            },
                                            cx,
                                        );
                                        cx.stop_propagation();
                                    }
                                }),
                            )
                            .into_any_element(),
                    );
                }
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
                if this.active_tab().scrollbar_drag.is_some() {
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

    /// Render the tab bar (styled like terminal tabs).
    fn render_tab_bar(&self, t: &ThemeColors, cx: &mut Context<Self>) -> impl IntoElement {
        let mut tab_elements: Vec<AnyElement> = Vec::new();

        for (i, tab) in self.tabs.iter().enumerate() {
            let is_active = i == self.active_tab;
            let label = tab.filename();

            tab_elements.push(
                div()
                    .id(ElementId::Name(format!("fv-tab-{}", i).into()))
                    .h(px(28.0))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .px(px(8.0))
                    .border_r_1()
                    .border_color(rgb(t.border))
                    .cursor_pointer()
                    .when(is_active, |d| {
                        d.bg(rgb(t.bg_secondary))
                            .text_color(rgb(t.text_primary))
                    })
                    .when(!is_active, |d| {
                        d.bg(rgb(t.bg_header))
                            .text_color(rgb(t.text_secondary))
                            .hover(|s| s.bg(rgb(t.bg_hover)))
                    })
                    .on_click(cx.listener(move |this, _, _window, cx| {
                        this.set_active_tab(i, cx);
                    }))
                    .on_mouse_down(
                        MouseButton::Middle,
                        cx.listener(move |this, _, _window, cx| {
                            this.close_tab(i, cx);
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                            this.tab_context_menu =
                                Some(super::context_menu::TabContextMenu {
                                    position: event.position,
                                    tab_index: i,
                                });
                            cx.notify();
                        }),
                    )
                    .child(
                        h_flex()
                            .gap(px(6.0))
                            .items_center()
                            // File type icon
                            .child(file_icon(&label, t, cx))
                            // Filename
                            .child(
                                div()
                                    .text_size(ui_text_md(cx))
                                    .max_w(px(160.0))
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .child(label),
                            ),
                    )
                    // Close button
                    .child(
                        div()
                            .id(ElementId::Name(format!("fv-tab-close-{}", i).into()))
                            .cursor_pointer()
                            .ml(px(4.0))
                            .w(px(16.0))
                            .h(px(16.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(3.0))
                            .hover(|s| s.bg(rgb(t.bg_hover)))
                            .on_click(cx.listener(move |this, _, _window, cx| {
                                this.close_tab(i, cx);
                            }))
                            .child(
                                svg()
                                    .path("icons/close.svg")
                                    .size(px(12.0))
                                    .text_color(rgb(t.text_muted)),
                            ),
                    )
                    .into_any_element(),
            );
        }

        h_flex()
            .id("fv-tabs-scroll")
            .h(px(28.0))
            .flex_shrink_0()
            .min_w_0()
            .overflow_x_scroll()
            .bg(rgb(t.bg_header))
            .border_b_1()
            .border_color(rgb(t.border))
            .children(tab_elements)
    }

    /// Render the back/forward navigation buttons.
    fn render_nav_buttons(&self, t: &ThemeColors, cx: &mut Context<Self>) -> impl IntoElement {
        let can_back = self.history.can_go_back();
        let can_forward = self.history.can_go_forward();

        h_flex()
            .gap(px(2.0))
            .child(
                div()
                    .id("fv-back")
                    .cursor(if can_back {
                        CursorStyle::PointingHand
                    } else {
                        CursorStyle::Arrow
                    })
                    .w(px(28.0))
                    .h(px(28.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(6.0))
                    .when(can_back, |d| d.hover(|s| s.bg(rgb(t.bg_hover))))
                    .on_click(cx.listener(|this, _, _window, cx| {
                        this.go_back(cx);
                    }))
                    .child(
                        svg()
                            .path("icons/chevron-left.svg")
                            .size(px(14.0))
                            .text_color(rgb(if can_back {
                                t.text_secondary
                            } else {
                                t.text_muted
                            }))
                            .opacity(if can_back { 1.0 } else { 0.4 }),
                    ),
            )
            .child(
                div()
                    .id("fv-forward")
                    .cursor(if can_forward {
                        CursorStyle::PointingHand
                    } else {
                        CursorStyle::Arrow
                    })
                    .w(px(28.0))
                    .h(px(28.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(6.0))
                    .when(can_forward, |d| d.hover(|s| s.bg(rgb(t.bg_hover))))
                    .on_click(cx.listener(|this, _, _window, cx| {
                        this.go_forward(cx);
                    }))
                    .child(
                        svg()
                            .path("icons/chevron-right.svg")
                            .size(px(14.0))
                            .text_color(rgb(if can_forward {
                                t.text_secondary
                            } else {
                                t.text_muted
                            }))
                            .opacity(if can_forward { 1.0 } else { 0.4 }),
                    ),
            )
    }

    fn render_hint(
        &self,
        key: &str,
        action: &str,
        t: &ThemeColors,
        cx: &App,
    ) -> impl IntoElement {
        h_flex()
            .gap(px(4.0))
            .child(
                div()
                    .px(px(4.0))
                    .py(px(1.0))
                    .rounded(px(3.0))
                    .bg(rgb(t.bg_secondary))
                    .text_size(ui_text_sm(cx))
                    .text_color(rgb(t.text_muted))
                    .child(key.to_string()),
            )
            .child(
                div()
                    .text_size(ui_text_sm(cx))
                    .text_color(rgb(t.text_muted))
                    .child(action.to_string()),
            )
    }
}

impl Render for FileViewer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Check for externally modified files (throttled to 1/sec)
        self.check_active_tab_freshness();

        let t = theme(cx);
        let focus_handle = self.focus_handle.clone();
        let tab = self.active_tab();
        let has_error = tab.error_message.is_some();
        let error_message = tab.error_message.clone();
        let is_markdown = tab.is_markdown;
        let display_mode = tab.display_mode;
        let is_preview_mode = display_mode == DisplayMode::Preview;
        let sidebar_visible = self.sidebar_visible;
        let show_tabs = self.tabs.len() > 1;

        let filename = tab
            .file_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "File".to_string());

        let relative_path = tab
            .file_path
            .strip_prefix(&self.project_path)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| tab.file_path.to_string_lossy().to_string());

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
        let tab = self.active_tab();
        let line_count = tab.line_count;
        let theme_colors = Arc::new(t.clone());
        let view = cx.entity().clone();
        let scrollbar_geometry = get_scrollbar_geometry(&tab.source_scroll_handle);
        let is_dragging_scrollbar = tab.scrollbar_drag.is_some();

        // Pre-render tree elements for sidebar
        let tree_elements = if sidebar_visible {
            self.render_tree_node(&self.file_tree.clone(), 0, "", &t, cx)
        } else {
            Vec::new()
        };

        // Pre-render markdown preview with selection
        let tab = self.active_tab();
        let preview_nodes: Vec<RenderedNode> = if !has_error && is_preview_mode && is_markdown {
            tab.markdown_doc
                .as_ref()
                .map(|doc| {
                    let selection = tab.markdown_selection.normalized_non_empty();
                    doc.render_nodes_with_offsets(&t, cx, selection)
                })
                .unwrap_or_default()
        } else {
            Vec::new()
        };

        // Render tab bar
        let tab_bar: Option<AnyElement> = if show_tabs {
            Some(self.render_tab_bar(&t, cx).into_any_element())
        } else {
            None
        };

        // Focus on first render, but not when inline rename input is active
        if self.rename_state.is_none() && !focus_handle.is_focused(window) {
            window.focus(&focus_handle, cx);
        }

        fullscreen_overlay("file-viewer", &t)
            .when(
                cfg!(target_os = "macos") && !window.is_fullscreen(),
                |d| d.top(px(28.0)),
            )
            .track_focus(&focus_handle)
            .key_context("FileViewer")
            .when(!is_preview_mode, |d| d.cursor(CursorStyle::IBeam))
            .on_action(cx.listener(|this, _: &Cancel, _window, cx| {
                // Dismiss overlays in priority order before default close behavior
                if this.tab_context_menu.is_some() {
                    this.tab_context_menu = None;
                    cx.notify();
                    return;
                }
                if this.context_menu.is_some() {
                    this.close_context_menu(cx);
                    return;
                }
                if this.rename_state.is_some() {
                    this.cancel_rename(cx);
                    return;
                }
                if this.delete_confirm.is_some() {
                    this.cancel_delete(cx);
                    return;
                }

                let tab = this.active_tab();
                let is_preview = tab.display_mode == DisplayMode::Preview;
                if is_preview && tab.markdown_selection.normalized_non_empty().is_some() {
                    this.active_tab_mut().markdown_selection.clear();
                    cx.notify();
                } else if this.active_tab().selection.normalized_non_empty().is_some() {
                    this.active_tab_mut().selection.clear();
                    cx.notify();
                } else {
                    this.close(cx);
                }
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                let key = event.keystroke.key.as_str();
                let modifiers = &event.keystroke.modifiers;
                let tab = this.active_tab();
                let is_preview = tab.display_mode == DisplayMode::Preview;
                let is_md = tab.is_markdown;

                match key {
                    "tab" if is_md && !modifiers.control && !modifiers.shift => {
                        this.toggle_display_mode(cx);
                    }
                    "tab" if modifiers.control && modifiers.shift => {
                        this.prev_tab(cx);
                    }
                    "tab" if modifiers.control => {
                        this.next_tab(cx);
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
                    "w" if modifiers.platform || modifiers.control => {
                        this.close_active_tab(cx);
                    }
                    "r" if !modifiers.platform && !modifiers.control => {
                        this.refresh_file_tree();
                        cx.notify();
                    }
                    "left" if modifiers.alt => {
                        this.go_back(cx);
                    }
                    "right" if modifiers.alt => {
                        this.go_forward(cx);
                    }
                    _ => {}
                }
            }))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                if this.active_tab().scrollbar_drag.is_some() {
                    let y = f32::from(event.position.y);
                    this.update_scrollbar_drag(y, cx);
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _window, cx| {
                    if this.active_tab().scrollbar_drag.is_some() {
                        this.end_scrollbar_drag(cx);
                    }
                }),
            )
            // Header
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
                                    .bg(rgb(if sidebar_visible {
                                        t.bg_selection
                                    } else {
                                        t.bg_secondary
                                    }))
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
                            .child(self.render_nav_buttons(&t, cx))
                            .child(
                                v_flex()
                                    .gap(px(2.0))
                                    .child(
                                        div()
                                            .text_size(ui_text_xl(cx))
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(rgb(t.text_primary))
                                            .child(filename),
                                    )
                                    .child(
                                        div()
                                            .text_size(ui_text_ms(cx))
                                            .text_color(rgb(t.text_muted))
                                            .child(relative_path),
                                    ),
                            ),
                    )
                    .child(
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
                                            cx,
                                        )),
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
                                            .text_size(ui_text(18.0, cx))
                                            .text_color(rgb(t.text_muted))
                                            .child("\u{00d7}"),
                                    ),
                            ),
                    ),
            )
            // Main content area: sidebar + (tab bar + content)
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .when(sidebar_visible, |d| {
                        d.child(self.render_sidebar(&t, tree_elements, cx))
                    })
                    .child(
                        v_flex()
                            .flex_1()
                            .h_full()
                            .min_h_0()
                            .min_w_0()
                            // Tab bar (above editor, not above sidebar)
                            .when_some(tab_bar, |d, tab_bar| d.child(tab_bar))
                            .when(has_error, |d| {
                                d.child(
                                    div()
                                        .flex_1()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .child(
                                            div()
                                                .text_size(ui_text_xl(cx))
                                                .text_color(rgb(t.text_muted))
                                                .child(error_message.unwrap_or_default()),
                                        ),
                                )
                            })
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
                                            uniform_list(
                                                "file-lines",
                                                line_count,
                                                move |range, _window, cx| {
                                                    let tc = tc.clone();
                                                    view_clone.update(cx, |this, cx| {
                                                        this.render_visible_lines(range, &tc, cx)
                                                    })
                                                },
                                            )
                                            .size_full()
                                            .bg(rgb(t.bg_secondary))
                                            .cursor(CursorStyle::IBeam)
                                            .track_scroll(
                                                &self.active_tab().source_scroll_handle,
                                            ),
                                        )
                                        .when(scrollbar_geometry.is_some(), |d| {
                                            let (_, _, thumb_y, thumb_height) =
                                                scrollbar_geometry.unwrap();
                                            d.child(self.render_scrollbar(
                                                &t,
                                                thumb_y,
                                                thumb_height,
                                                is_dragging_scrollbar,
                                                cx,
                                            ))
                                        }),
                                )
                            })
                            .when(!has_error && is_preview_mode, |d| {
                                let mut content_children: Vec<AnyElement> = Vec::new();
                                let mut node_idx = 0usize;

                                for rendered_node in preview_nodes {
                                    match rendered_node {
                                        RenderedNode::Simple {
                                            div: node_div,
                                            start_offset,
                                            end_offset,
                                        } => {
                                            let node_end = end_offset.saturating_sub(1);
                                            let idx = node_idx;
                                            content_children.push(
                                                div()
                                                    .id(ElementId::Name(
                                                        format!("md-node-{}", idx).into(),
                                                    ))
                                                    .w_full()
                                                    .on_mouse_down(
                                                        MouseButton::Left,
                                                        cx.listener(
                                                            move |this,
                                                                  event: &MouseDownEvent,
                                                                  _window,
                                                                  cx| {
                                                                let tab = this.active_tab_mut();
                                                                if event.click_count == 2 {
                                                                    tab.markdown_selection.start =
                                                                        Some(start_offset);
                                                                    tab.markdown_selection.end =
                                                                        Some(node_end);
                                                                    tab.markdown_selection
                                                                        .finish();
                                                                } else {
                                                                    tab.markdown_selection.start =
                                                                        Some(start_offset);
                                                                    tab.markdown_selection.end =
                                                                        Some(start_offset);
                                                                    tab.markdown_selection
                                                                        .is_selecting = true;
                                                                }
                                                                cx.notify();
                                                            },
                                                        ),
                                                    )
                                                    .on_mouse_move(cx.listener(
                                                        move |this,
                                                              _event: &MouseMoveEvent,
                                                              _window,
                                                              cx| {
                                                            let tab = this.active_tab_mut();
                                                            if tab.markdown_selection.is_selecting {
                                                                if let Some(sel_start) =
                                                                    tab.markdown_selection.start
                                                                {
                                                                    if start_offset >= sel_start {
                                                                        tab.markdown_selection
                                                                            .end = Some(node_end);
                                                                    } else {
                                                                        tab.markdown_selection
                                                                            .end =
                                                                            Some(start_offset);
                                                                    }
                                                                    cx.notify();
                                                                }
                                                            }
                                                        },
                                                    ))
                                                    .on_mouse_up(
                                                        MouseButton::Left,
                                                        cx.listener(
                                                            |this,
                                                             _event: &MouseUpEvent,
                                                             _window,
                                                             cx| {
                                                                this.active_tab_mut()
                                                                    .markdown_selection
                                                                    .finish();
                                                                cx.notify();
                                                            },
                                                        ),
                                                    )
                                                    .child(node_div)
                                                    .into_any_element(),
                                            );
                                            node_idx += 1;
                                        }
                                        RenderedNode::CodeBlock {
                                            language, lines, ..
                                        } => {
                                            let idx = node_idx;
                                            let line_children: Vec<AnyElement> = lines
                                                .into_iter()
                                                .enumerate()
                                                .map(
                                                    |(line_idx, (line_div, start_offset, end_offset))| {
                                                        let line_end =
                                                            end_offset.saturating_sub(1);
                                                        div()
                                                        .id(ElementId::Name(format!("md-code-{}-line-{}", idx, line_idx).into()))
                                                        .on_mouse_down(MouseButton::Left, cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                                                            let tab = this.active_tab_mut();
                                                            if event.click_count == 2 {
                                                                tab.markdown_selection.start = Some(start_offset);
                                                                tab.markdown_selection.end = Some(line_end);
                                                                tab.markdown_selection.finish();
                                                            } else {
                                                                tab.markdown_selection.start = Some(start_offset);
                                                                tab.markdown_selection.end = Some(start_offset);
                                                                tab.markdown_selection.is_selecting = true;
                                                            }
                                                            cx.notify();
                                                        }))
                                                        .on_mouse_move(cx.listener(move |this, _event: &MouseMoveEvent, _window, cx| {
                                                            let tab = this.active_tab_mut();
                                                            if tab.markdown_selection.is_selecting {
                                                                if let Some(sel_start) = tab.markdown_selection.start {
                                                                    if start_offset >= sel_start {
                                                                        tab.markdown_selection.end = Some(line_end);
                                                                    } else {
                                                                        tab.markdown_selection.end = Some(start_offset);
                                                                    }
                                                                    cx.notify();
                                                                }
                                                            }
                                                        }))
                                                        .on_mouse_up(MouseButton::Left, cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                                                            this.active_tab_mut().markdown_selection.finish();
                                                            cx.notify();
                                                        }))
                                                        .child(line_div)
                                                        .into_any_element()
                                                    },
                                                )
                                                .collect();

                                            let code_block =
                                                code_block_container(language.as_deref(), &t, cx)
                                                    .id(ElementId::Name(
                                                        format!("md-codeblock-{}", idx).into(),
                                                    ))
                                                    .child(
                                                        div()
                                                            .p(px(12.0))
                                                            .font_family("monospace")
                                                            .text_size(ui_text(
                                                                self.file_font_size,
                                                                cx,
                                                            ))
                                                            .text_color(rgb(t.text_secondary))
                                                            .flex()
                                                            .flex_col()
                                                            .children(line_children),
                                                    );

                                            content_children.push(code_block.into_any_element());
                                            node_idx += 1;
                                        }
                                        RenderedNode::Table { header, rows } => {
                                            let idx = node_idx;
                                            let mut table_rows: Vec<AnyElement> = Vec::new();

                                            if let Some((header_div, start_offset, end_offset)) =
                                                header
                                            {
                                                let row_end = end_offset.saturating_sub(1);
                                                table_rows.push(
                                                    div()
                                                        .id(ElementId::Name(format!("md-table-{}-header", idx).into()))
                                                        .on_mouse_down(MouseButton::Left, cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                                                            let tab = this.active_tab_mut();
                                                            if event.click_count == 2 {
                                                                tab.markdown_selection.start = Some(start_offset);
                                                                tab.markdown_selection.end = Some(row_end);
                                                                tab.markdown_selection.finish();
                                                            } else {
                                                                tab.markdown_selection.start = Some(start_offset);
                                                                tab.markdown_selection.end = Some(start_offset);
                                                                tab.markdown_selection.is_selecting = true;
                                                            }
                                                            cx.notify();
                                                        }))
                                                        .on_mouse_move(cx.listener(move |this, _event: &MouseMoveEvent, _window, cx| {
                                                            let tab = this.active_tab_mut();
                                                            if tab.markdown_selection.is_selecting {
                                                                if let Some(sel_start) = tab.markdown_selection.start {
                                                                    if start_offset >= sel_start {
                                                                        tab.markdown_selection.end = Some(row_end);
                                                                    } else {
                                                                        tab.markdown_selection.end = Some(start_offset);
                                                                    }
                                                                    cx.notify();
                                                                }
                                                            }
                                                        }))
                                                        .on_mouse_up(MouseButton::Left, cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                                                            this.active_tab_mut().markdown_selection.finish();
                                                            cx.notify();
                                                        }))
                                                        .child(header_div)
                                                        .into_any_element()
                                                );
                                            }

                                            for (row_idx, (row_div, start_offset, end_offset)) in
                                                rows.into_iter().enumerate()
                                            {
                                                let row_end = end_offset.saturating_sub(1);
                                                table_rows.push(
                                                    div()
                                                        .id(ElementId::Name(format!("md-table-{}-row-{}", idx, row_idx).into()))
                                                        .on_mouse_down(MouseButton::Left, cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                                                            let tab = this.active_tab_mut();
                                                            if event.click_count == 2 {
                                                                tab.markdown_selection.start = Some(start_offset);
                                                                tab.markdown_selection.end = Some(row_end);
                                                                tab.markdown_selection.finish();
                                                            } else {
                                                                tab.markdown_selection.start = Some(start_offset);
                                                                tab.markdown_selection.end = Some(start_offset);
                                                                tab.markdown_selection.is_selecting = true;
                                                            }
                                                            cx.notify();
                                                        }))
                                                        .on_mouse_move(cx.listener(move |this, _event: &MouseMoveEvent, _window, cx| {
                                                            let tab = this.active_tab_mut();
                                                            if tab.markdown_selection.is_selecting {
                                                                if let Some(sel_start) = tab.markdown_selection.start {
                                                                    if start_offset >= sel_start {
                                                                        tab.markdown_selection.end = Some(row_end);
                                                                    } else {
                                                                        tab.markdown_selection.end = Some(start_offset);
                                                                    }
                                                                    cx.notify();
                                                                }
                                                            }
                                                        }))
                                                        .on_mouse_up(MouseButton::Left, cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                                                            this.active_tab_mut().markdown_selection.finish();
                                                            cx.notify();
                                                        }))
                                                        .child(row_div)
                                                        .into_any_element()
                                                );
                                            }

                                            let table = div()
                                                .id(ElementId::Name(
                                                    format!("md-table-{}", idx).into(),
                                                ))
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
                                        .track_scroll(
                                            &self.active_tab().markdown_scroll_handle,
                                        )
                                        .bg(rgb(t.bg_secondary))
                                        .cursor(CursorStyle::IBeam)
                                        .on_mouse_up(
                                            MouseButton::Left,
                                            cx.listener(
                                                |this, _event: &MouseUpEvent, _window, cx| {
                                                    this.active_tab_mut()
                                                        .markdown_selection
                                                        .finish();
                                                    cx.notify();
                                                },
                                            ),
                                        )
                                        .child(content_div),
                                )
                            })
                            // Footer
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
                                            .child(self.render_hint("B", "files", &t, cx))
                                            .when(is_markdown, |d| {
                                                d.child(self.render_hint(
                                                    "Tab",
                                                    "toggle preview",
                                                    &t,
                                                    cx,
                                                ))
                                            })
                                            .child(self.render_hint(
                                                if cfg!(target_os = "macos") {
                                                    "Cmd+C"
                                                } else {
                                                    "Ctrl+C"
                                                },
                                                "copy",
                                                &t,
                                                cx,
                                            ))
                                            .child(self.render_hint(
                                                if cfg!(target_os = "macos") {
                                                    "Cmd+A"
                                                } else {
                                                    "Ctrl+A"
                                                },
                                                "select all",
                                                &t,
                                                cx,
                                            ))
                                            .child(self.render_hint(
                                                if cfg!(target_os = "macos") {
                                                    "Cmd+W"
                                                } else {
                                                    "Ctrl+W"
                                                },
                                                "close tab",
                                                &t,
                                                cx,
                                            ))
                                            .child(self.render_hint(
                                                "Alt+\u{2190}/\u{2192}",
                                                "back/fwd",
                                                &t,
                                                cx,
                                            ))
                                            .child(self.render_hint("Esc", "close", &t, cx)),
                                    )
                                    .child(
                                        div()
                                            .text_size(ui_text_sm(cx))
                                            .text_color(rgb(t.text_muted))
                                            .when(!is_preview_mode, |d| {
                                                d.child(format!(
                                                    "{} lines",
                                                    self.active_tab().line_count
                                                ))
                                            })
                                            .when(is_preview_mode, |d| {
                                                d.child("Preview mode")
                                            }),
                                    ),
                            ),
                    ),
            )
            // Filter popover backdrop + overlay (at fullscreen overlay level)
            .when(self.filter_popover_open, |d| {
                d.child(
                    div()
                        .id("fv-filter-popover-backdrop")
                        .absolute()
                        .inset_0()
                        .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                            this.filter_popover_open = false;
                            cx.notify();
                        }))
                )
            })
            .when(self.filter_popover_open && self.filter_button_bounds.is_some(), |d| {
                let bounds = self.filter_button_bounds.unwrap();
                let entity = cx.entity().downgrade();
                d.child(crate::list_overlay::file_filter_popover(
                    bounds, self.show_ignored, self.show_hidden, &t, cx,
                    move |filter, _, cx| {
                        if let Some(e) = entity.upgrade() {
                            e.update(cx, |this, cx| this.toggle_filter(filter, cx));
                        }
                    },
                ))
            })
            .when_some(self.render_context_menu(&t, cx), |d, menu| d.child(menu))
            .when_some(self.render_tab_context_menu(&t, cx), |d, menu| d.child(menu))
            .when_some(self.render_delete_confirm(&t, cx), |d, dialog| d.child(dialog))
    }
}
