//! Rendering logic for the file viewer overlay.

use crate::code_view::{
    build_styled_text_with_backgrounds, find_word_boundaries, get_scrollbar_geometry,
    selection_bg_ranges,
};
use crate::file_search::Cancel;
use crate::file_tree::{expandable_file_row, expandable_folder_row};
use crate::selection::{Selection1DExtension, Selection2DNonEmpty};
use crate::syntax::HighlightedLine;
use crate::theme::theme;
use gpui::prelude::*;
use gpui::*;
use gpui_component::{h_flex, v_flex};
use gpui_component::scroll::ScrollableElement;
use okena_core::theme::ThemeColors;
use std::path::PathBuf;
use okena_markdown::RenderedNode;
use okena_ui::code_block::code_block_container;
use okena_ui::modal::{detached_needs_controls, fullscreen_overlay, fullscreen_panel, window_drag_spacer, window_min_max_controls};
use okena_ui::toggle::segmented_toggle;
use okena_ui::file_icon::file_icon;
use okena_ui::tokens::{ui_text, ui_text_md, ui_text_ms, ui_text_sm, ui_text_xl};
use std::sync::Arc;

use super::context_menu::TreeNodeTarget;
use super::{DisplayMode, FileViewer, FontData, PreviewBackground, SIDEBAR_WIDTH};

/// Helper to create rgba from u32 color and alpha.
fn rgba(color: u32, alpha: f32) -> Rgba {
    let r = ((color >> 16) & 0xFF) as f32 / 255.0;
    let g = ((color >> 8) & 0xFF) as f32 / 255.0;
    let b = (color & 0xFF) as f32 / 255.0;
    Rgba { r, g, b, a: alpha }
}

/// Placeholder row shown while a directory's children are being fetched.
fn loading_row(depth: usize, t: &ThemeColors, cx: &App) -> Div {
    let indent = depth as f32 * 14.0;
    div()
        .flex()
        .items_center()
        .h(px(22.0))
        .pl(px(indent + 8.0 + 18.0))
        .pr(px(12.0))
        .text_size(ui_text(12.0, cx))
        .text_color(rgb(t.text_muted))
        .child("Loading…")
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

        let mut bg_ranges = selection_bg_ranges(&tab.selection, line_number, line.plain_text.len());
        bg_ranges.extend(self.search_bg_ranges_for_line(line_number));

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
                cx.listener(|this, _: &MouseUpEvent, _window, cx| {
                    this.active_tab_mut().selection.finish();
                    cx.notify();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                    if this.active_tab().selection.normalized_non_empty().is_some() {
                        this.selection_context_menu = Some(event.position);
                        cx.notify();
                    }
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
            .when_some(
                self.render_blame_cell(line_number, line_height, char_width, t, cx),
                |d, cell| d.child(cell),
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
        let active_count = self.show_ignored as u8;
        let _is_open = self.filter_popover_open;

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


    /// Recursively render file tree nodes with expand/collapse, lazy-loading
    /// directory listings via `loaded_dirs` as folders open.
    pub(super) fn render_tree_node(
        &self,
        parent_relative: &str,
        depth: usize,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut elements: Vec<AnyElement> = Vec::new();

        // Active and open file paths drive highlighting in the tree.
        let active_relative = self.active_tab().relative_path.clone();
        let open_relatives: std::collections::HashSet<String> = self
            .tabs
            .iter()
            .filter(|t| !t.is_empty())
            .map(|t| t.relative_path.clone())
            .collect();

        let entries = match self.loaded_dirs.get(parent_relative) {
            Some(entries) => entries,
            None => {
                if self.loading_dirs.contains(parent_relative) {
                    elements.push(loading_row(depth, t, cx).into_any_element());
                }
                return elements;
            }
        };

        for entry in entries {
            let child_relative = if parent_relative.is_empty() {
                entry.name.clone()
            } else {
                format!("{}/{}", parent_relative, entry.name)
            };

            if entry.is_dir {
                self.render_folder_row(
                    &mut elements,
                    entry,
                    &child_relative,
                    depth,
                    t,
                    cx,
                );
                if self.expanded_folders.contains(&child_relative) {
                    elements.extend(self.render_tree_node(&child_relative, depth + 1, t, cx));
                }
            } else {
                let is_active = active_relative == child_relative;
                let is_open = open_relatives.contains(&child_relative);
                self.render_file_row(
                    &mut elements,
                    entry,
                    &child_relative,
                    depth,
                    is_active,
                    is_open,
                    t,
                    cx,
                );
            }
        }

        elements
    }

    fn render_folder_row(
        &self,
        elements: &mut Vec<AnyElement>,
        entry: &crate::list_directory::DirEntry,
        folder_relative: &str,
        depth: usize,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) {
        let is_expanded = self.expanded_folders.contains(folder_relative);
        let is_renaming = self.is_renaming_folder(folder_relative);
        let is_ctx_target = self.is_context_menu_target_folder(folder_relative);
        let indent = depth as f32 * 14.0;

        if is_renaming {
            let mut row = div()
                .id(ElementId::Name(
                    format!("fv-folder-{}-rename", folder_relative).into(),
                ))
                .flex()
                .items_center()
                .h(px(26.0))
                .pl(px(indent + 8.0))
                .pr(px(12.0))
                .bg(rgb(t.bg_selection))
                .child(
                    svg()
                        .path(if is_expanded {
                            "icons/chevron-down.svg"
                        } else {
                            "icons/chevron-right.svg"
                        })
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
            return;
        }

        let folder_for_click = folder_relative.to_string();
        let folder_for_ctx = folder_relative.to_string();
        let abs_path_for_ctx = match self.project_fs.project_root() {
            Some(root) => root.join(folder_relative),
            None => PathBuf::from(folder_relative),
        };

        elements.push(
            expandable_folder_row(&entry.name, depth, is_expanded, t, cx)
                .id(ElementId::Name(format!("fv-folder-{}", folder_relative).into()))
                .when(is_ctx_target, |d| d.bg(rgb(t.bg_selection)))
                .on_click(cx.listener(move |this, _, _window, cx| {
                    this.toggle_folder(&folder_for_click, cx);
                }))
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener({
                        let folder_path = folder_for_ctx;
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

    // GPUI render helper: many params are render inputs (theme, flags, indices).
    #[allow(clippy::too_many_arguments)]
    fn render_file_row(
        &self,
        elements: &mut Vec<AnyElement>,
        entry: &crate::list_directory::DirEntry,
        file_relative: &str,
        depth: usize,
        is_active: bool,
        is_open: bool,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) {
        let abs_path = match self.project_fs.project_root() {
            Some(root) => root.join(file_relative),
            None => PathBuf::from(file_relative),
        };
        let is_renaming = self.is_renaming_file(&abs_path);
        let is_ctx_target = self.is_context_menu_target_file(&abs_path);
        let highlight = is_active || is_ctx_target;
        let indent = depth as f32 * 14.0;

        if is_renaming {
            let mut row = div()
                .id(ElementId::Name(format!("fv-file-{}-rename", file_relative).into()))
                .flex()
                .items_center()
                .gap(px(6.0))
                .h(px(26.0))
                .pl(px(indent + 8.0 + 18.0))
                .pr(px(12.0))
                .bg(rgb(t.bg_selection))
                .child(file_icon(&entry.name, t, cx).mr(px(4.0)));
            if let Some(input) = self.render_rename_input(t, cx) {
                row = row.child(input);
            }
            row = row.on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key.as_str() == "enter" {
                    this.finish_rename(cx);
                }
            }));
            elements.push(row.into_any_element());
            return;
        }

        let file_relative_for_click = file_relative.to_string();
        elements.push(
            expandable_file_row(&entry.name, depth, None, is_open || is_active, t, cx)
                .id(ElementId::Name(format!("fv-file-{}", file_relative).into()))
                .when(highlight, |d| d.bg(rgba(t.bg_selection, 0.5)))
                .on_click(cx.listener(move |this, _, _window, cx| {
                    this.select_file(file_relative_for_click.clone(), cx);
                }))
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener({
                        let path = abs_path;
                        move |this, event: &MouseDownEvent, _, cx| {
                            this.open_context_menu(
                                event.position,
                                TreeNodeTarget::File { path: path.clone() },
                                cx,
                            );
                            cx.stop_propagation();
                        }
                    }),
                )
                .into_any_element(),
        );
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

impl FileViewer {
    /// Ensure the active tab has a `ListState` matching its markdown document,
    /// returning a clone for the render to drive the virtualized preview.
    ///
    /// The state is rebuilt when the block count changes (a different document
    /// or an external reload) and remeasured when the font size changes, so
    /// cached item heights stay valid.
    fn ensure_markdown_list_state(&mut self, _cx: &mut Context<Self>) -> Option<ListState> {
        let font = self.file_font_size;
        let tab = self.active_tab_mut();
        let count = tab.markdown_doc.as_ref().map(|d| d.node_count())?;

        let needs_new = match &tab.markdown_list_state {
            None => true,
            Some(_) => tab.markdown_list_nodes != count,
        };

        if needs_new {
            tab.markdown_list_state =
                Some(ListState::new(count, ListAlignment::Top, px(400.0)));
            tab.markdown_list_nodes = count;
            tab.markdown_list_font = font;
        } else if tab.markdown_list_font != font {
            if let Some(state) = &tab.markdown_list_state {
                state.remeasure();
            }
            tab.markdown_list_font = font;
        }

        tab.markdown_list_state.clone()
    }

    /// Render the image / SVG-Preview pane with zoom / pan / background
    /// support. Default is `auto_fit` (ObjectFit::Contain); once the user
    /// wheel-zooms or drags, the view switches to natural-size × zoom with
    /// manual pan. Double-click resets to fit.
    pub(super) fn render_image_preview(
        &self,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let tab = self.active_tab();
        let image_data = tab.image_data.clone();
        let view = tab.image_view.clone();
        let muted = t.text_muted;
        let bg_secondary = t.bg_secondary;

        let Some(image) = image_data else {
            return div()
                .id("file-image-empty")
                .flex_1()
                .min_h_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(rgb(bg_secondary))
                .child(
                    div()
                        .text_size(ui_text_sm(cx))
                        .text_color(rgb(muted))
                        .child("Image not available"),
                )
                .into_any_element();
        };

        let (nat_w, nat_h) = image.dimensions();
        let zoom = view.zoom;
        let pan = view.pan;
        let auto_fit = view.auto_fit;

        // Background fill: explicit color for Light/Dark, checkerboard
        // canvas for Checker. The canvas is painted into the pane via
        // paint_quad in tiles small enough to read against both extremes.
        let (bg_solid, show_checker) = match view.background {
            PreviewBackground::Light => (0xFFFFFFu32, false),
            PreviewBackground::Dark => (0x111111u32, false),
            PreviewBackground::Checker => (0x808080u32, true),
        };

        let fallback_muted = muted;
        let img_element = img(image)
            .with_fallback(move || {
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(14.0))
                    .text_color(rgb(fallback_muted))
                    .child("Cannot decode image")
                    .into_any_element()
            });

        let img_styled: AnyElement = if auto_fit {
            img_element
                .object_fit(ObjectFit::Contain)
                .max_w_full()
                .max_h_full()
                .into_any_element()
        } else {
            // flex_shrink_0 is required: without it, the flex container at
            // the next layer ("size_full .flex .items_center .justify_center")
            // squishes one axis once the image's explicit size exceeds the
            // pane, which combined with ObjectFit::Fill produced a stretched
            // preview and made only one axis appear to "zoom past" the
            // viewport. Contain keeps aspect inside the (now-fixed) box.
            img_element
                .object_fit(ObjectFit::Contain)
                .w(px((nat_w as f32) * zoom))
                .h(px((nat_h as f32) * zoom))
                .flex_shrink_0()
                .ml(pan.x)
                .mt(pan.y)
                .into_any_element()
        };

        let cursor = if auto_fit {
            CursorStyle::Arrow
        } else if view.is_panning {
            CursorStyle::ClosedHand
        } else {
            CursorStyle::OpenHand
        };

        let mut container = div()
            .id("file-image")
            .flex_1()
            .min_h_0()
            .relative()
            .overflow_hidden()
            .bg(rgb(bg_solid))
            .cursor(cursor);

        if show_checker {
            container = container.child(
                canvas(
                    |_, _, _| (),
                    |bounds, _, window, _| paint_checkerboard(bounds, window),
                )
                .absolute()
                .inset_0()
                .size_full(),
            );
        }

        container
            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _, cx| {
                let delta = event.delta.pixel_delta(px(17.0));
                let dy = f32::from(delta.y);
                // Reject non-finite deltas (a hostile remote bridge or
                // momentum-extrapolation overflow can hand us NaN/inf;
                // `NaN < EPSILON` is false so the old guard let them
                // through and poisoned zoom into NaN).
                if !dy.is_finite() {
                    return;
                }
                // Filter the macOS kinetic-momentum tail (deltas in the
                // 0.01–0.5 range continuing for ~1 s after the user
                // stops scrolling — they would otherwise keep nudging
                // zoom and respawning the SVG re-raster pipeline).
                if dy.abs() < 0.5 {
                    return;
                }
                // Exponential: positive dy zooms in, negative out. ±100 px ≈ 1.2× / 0.83×.
                let factor = (dy / 250.0).exp();
                this.image_zoom_by(factor, cx);
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                    if event.click_count >= 2 {
                        this.image_fit(cx);
                    } else {
                        this.image_start_pan(event.position, cx);
                    }
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                // If the user released the button outside the container
                // we never saw the on_mouse_up — clear sticky pan state
                // on the next move that arrives without a pressed Left
                // button so the image doesn't follow the cursor with no
                // button held.
                if event.pressed_button != Some(MouseButton::Left) {
                    if this.active_tab().image_view.is_panning {
                        this.image_end_pan(cx);
                    }
                    return;
                }
                this.image_update_pan(event.position, cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.image_end_pan(cx);
                }),
            )
            .child(
                div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(img_styled),
            )
            .into_any_element()
    }
}

/// Header chiclet: `−` / `Fit | 100%` / `+`. Click on the label toggles
/// between Fit and 100%. Buttons step zoom by 1.25× / 0.8×.
fn image_zoom_controls(
    zoom_label: String,
    t: &ThemeColors,
    cx: &mut Context<FileViewer>,
) -> impl IntoElement {
    let button = move |id: &'static str, glyph: &'static str, t: &ThemeColors, cx: &mut Context<FileViewer>, on_click: Box<dyn Fn(&mut FileViewer, &mut Context<FileViewer>)>| {
        let t_text_muted = t.text_muted;
        let t_bg_hover = t.bg_hover;
        div()
            .id(id)
            .cursor_pointer()
            .w(px(24.0))
            .h(px(24.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(4.0))
            .hover(|s| s.bg(rgb(t_bg_hover)))
            .text_size(px(13.0))
            .text_color(rgb(t_text_muted))
            .on_click(cx.listener(move |this, _, _, cx| on_click(this, cx)))
            .child(glyph)
    };
    h_flex()
        .gap(px(2.0))
        .px(px(4.0))
        .py(px(2.0))
        .rounded(px(4.0))
        .bg(rgb(t.bg_secondary))
        .child(button(
            "zoom-out",
            "−",
            t,
            cx,
            Box::new(|this, cx| this.image_zoom_by(1.0 / 1.25, cx)),
        ))
        .child(
            div()
                .id("zoom-label")
                .cursor_pointer()
                .min_w(px(48.0))
                .text_align(TextAlign::Center)
                .text_size(px(12.0))
                .text_color(rgb(t.text_muted))
                .rounded(px(4.0))
                .hover(|s| s.bg(rgb(t.bg_hover)))
                .px(px(4.0))
                .py(px(2.0))
                .on_click(cx.listener(|this, _, _, cx| {
                    if this.active_tab().image_view.auto_fit {
                        this.image_set_zoom(1.0, cx);
                    } else {
                        this.image_fit(cx);
                    }
                }))
                .child(zoom_label),
        )
        .child(button(
            "zoom-in",
            "+",
            t,
            cx,
            Box::new(|this, cx| this.image_zoom_by(1.25, cx)),
        ))
}

/// Header chiclet that cycles the preview background (Checker / Light /
/// Dark). The current option is highlighted; clicking advances to the
/// next. Solves the "black SVG on dark theme" / "white SVG on light
/// theme" invisibility problem.
fn image_background_toggle(
    current: PreviewBackground,
    t: &ThemeColors,
    cx: &mut Context<FileViewer>,
) -> impl IntoElement {
    let labels = [
        (PreviewBackground::Checker, "Checker"),
        (PreviewBackground::Light, "Light"),
        (PreviewBackground::Dark, "Dark"),
    ];
    h_flex()
        .gap(px(2.0))
        .px(px(2.0))
        .py(px(2.0))
        .rounded(px(4.0))
        .bg(rgb(t.bg_secondary))
        .children(labels.into_iter().map(|(bg, label)| {
            let is_active = current == bg;
            let t_active = t.bg_selection;
            let t_hover = t.bg_hover;
            let t_text_primary = t.text_primary;
            let t_text_muted = t.text_muted;
            div()
                .id(ElementId::Name(format!("bg-{}", label).into()))
                .cursor_pointer()
                .px(px(8.0))
                .py(px(2.0))
                .rounded(px(3.0))
                .when(is_active, |d| d.bg(rgb(t_active)))
                .when(!is_active, |d| d.hover(|s| s.bg(rgb(t_hover))))
                .text_size(px(12.0))
                .text_color(rgb(if is_active { t_text_primary } else { t_text_muted }))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.image_set_background(bg, cx);
                }))
                .child(label)
        }))
}

/// Paint a procedural 12 px checkerboard inside `bounds`. Used as the
/// default image-preview background — a black SVG icon stays visible
/// against the light tiles and a white one against the dark tiles, so the
/// user always sees at least half the artwork regardless of fill color.
fn paint_checkerboard(bounds: gpui::Bounds<gpui::Pixels>, window: &mut gpui::Window) {
    const TILE: f32 = 12.0;
    let light: gpui::Rgba = gpui::Rgba {
        r: 0.62,
        g: 0.62,
        b: 0.62,
        a: 1.0,
    };
    let dark: gpui::Rgba = gpui::Rgba {
        r: 0.42,
        g: 0.42,
        b: 0.42,
        a: 1.0,
    };
    // Base fill — every "even" tile inherits this.
    window.paint_quad(fill(bounds, light));
    let cols = (f32::from(bounds.size.width) / TILE).ceil() as usize + 1;
    let rows = (f32::from(bounds.size.height) / TILE).ceil() as usize + 1;
    for row in 0..rows {
        for col in 0..cols {
            if (row + col) % 2 == 0 {
                continue;
            }
            let x = f32::from(bounds.origin.x) + (col as f32) * TILE;
            let y = f32::from(bounds.origin.y) + (row as f32) * TILE;
            let tile_bounds = gpui::Bounds {
                origin: gpui::point(px(x), px(y)),
                size: gpui::size(px(TILE), px(TILE)),
            };
            window.paint_quad(fill(tile_bounds, dark));
        }
    }
}

impl Render for FileViewer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Schedule a background freshness check for externally modified files
        // (throttled to 1/sec). Cheap on the render thread — the stat and any
        // reload/re-highlight run on the background executor.
        self.check_active_tab_freshness(cx);

        let t = theme(cx);
        let focus_handle = self.focus_handle.clone();
        let tab = self.active_tab();
        let tab_loading = tab.loading;
        let has_error = tab.error_message.is_some();
        let error_message = tab.error_message.clone();
        let is_markdown = tab.is_markdown;
        let is_image = tab.is_image;
        let is_svg = tab.is_svg;
        let is_font = tab.is_font;
        let font_data = tab.font_data.clone();
        let display_mode = tab.display_mode;
        let is_preview_mode = display_mode == DisplayMode::Preview;
        // Body view selectors. Each tab renders exactly one of these branches:
        //   * show_image   — raster image, or SVG in Preview mode
        //   * show_font    — font preview (sample text + metadata)
        //   * show_md_preview — markdown rendered preview
        //   * show_source  — text source / markdown source / SVG XML source
        // Markdown / image / font are mutually exclusive (the loader picks
        // one), so these four are also mutually exclusive in practice.
        let show_image = is_image && (!is_svg || is_preview_mode);
        let show_font = is_font;
        let show_md_preview = is_markdown && is_preview_mode;
        let show_source = !show_image && !show_font && !show_md_preview;
        // Whether the header should expose the Preview/Source toggle.
        let supports_view_toggle = is_markdown || is_svg;
        let sidebar_visible = self.sidebar_visible;
        let show_tabs = self.tabs.len() > 1;

        let filename = tab
            .file_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "File".to_string());

        let relative_path = if !tab.relative_path.is_empty() {
            tab.relative_path.clone()
        } else {
            tab.file_path.to_string_lossy().to_string()
        };

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
        let theme_colors = Arc::new(t);
        let view = cx.entity().clone();
        let scrollbar_geometry = get_scrollbar_geometry(&tab.source_scroll_handle);
        let is_dragging_scrollbar = tab.scrollbar_drag.is_some();

        // Pre-render tree elements for sidebar
        let tree_elements = if sidebar_visible {
            self.render_tree_node("", 0, &t, cx)
        } else {
            Vec::new()
        };

        // Markdown preview uses a virtualized list (built below) so only the
        // visible blocks are rendered per frame. Ensure the per-tab ListState
        // exists and matches the current document and font size.
        let markdown_list_state: Option<ListState> =
            if !has_error && is_preview_mode && is_markdown {
                self.ensure_markdown_list_state(cx)
            } else {
                None
            };

        // Render tab bar
        let tab_bar: Option<AnyElement> = if show_tabs {
            Some(self.render_tab_bar(&t, cx).into_any_element())
        } else {
            None
        };

        // Focus on first render, but not when inline rename or search input is active
        if self.rename_state.is_none() && self.search_state.is_none() && !focus_handle.is_focused(window) {
            window.focus(&focus_handle, cx);
        }

        let outer = if self.is_detached {
            fullscreen_panel("file-viewer", &t)
                .when(
                    cfg!(target_os = "macos") && !window.is_fullscreen(),
                    |d| d.pt(px(28.0)),
                )
        } else {
            fullscreen_overlay("file-viewer", &t)
                .when(
                    cfg!(target_os = "macos") && !window.is_fullscreen(),
                    |d| d.top(px(28.0)),
                )
        };
        outer
            .track_focus(&focus_handle)
            .key_context("FileViewer")
            .when(show_source, |d| d.cursor(CursorStyle::IBeam))
            .on_action(cx.listener(|this, _: &Cancel, window, cx| {
                // Dismiss overlays in priority order before default close behavior
                if this.selection_context_menu.is_some() {
                    this.selection_context_menu = None;
                    cx.notify();
                    return;
                }
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
                if this.search_state.is_some() {
                    this.close_search(window, cx);
                    return;
                }

                let tab = this.active_tab();
                let is_preview = tab.display_mode == DisplayMode::Preview;
                if is_preview && tab.markdown_selection.normalized_non_empty().is_some() {
                    this.active_tab_mut().markdown_selection.clear();
                    cx.notify();
                } else if this.active_tab().selection.normalized_non_empty().is_some() {
                    this.clear_source_selection(cx);
                } else {
                    this.close(cx);
                }
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                // Don't intercept keys when search input is focused
                if this.search_state.as_ref().is_some_and(|s| {
                    s.input.read(cx).focus_handle(cx).is_focused(window)
                }) {
                    return;
                }

                let key = event.keystroke.key.as_str();
                let modifiers = &event.keystroke.modifiers;
                let tab = this.active_tab();
                let is_preview = tab.display_mode == DisplayMode::Preview;
                let is_md = tab.is_markdown;
                let is_img = tab.is_image;
                let is_svg_tab = tab.is_svg;
                let is_font_tab = tab.is_font;
                // SVG in preview mode behaves like a raster image (no source).
                // In source mode the highlighted XML is shown, so selection
                // / search / copy / select-all all work normally.
                let is_img_view = is_img && (!is_svg_tab || is_preview);
                // Image-zoom chords only apply when the user is actually
                // looking at the image (SVG in Source mode shows XML; we
                // don't want Cmd+= to silently zoom a hidden preview).
                let img_zoom_chord = is_img_view;

                match key {
                    "f" if modifiers.platform || modifiers.control => {
                        if !is_preview && !is_img_view && !is_font_tab {
                            this.open_search(window, cx);
                        }
                    }
                    "tab" if (is_md || is_svg_tab) && !modifiers.control && !modifiers.shift => {
                        this.toggle_display_mode(cx);
                    }
                    "tab" if modifiers.control && modifiers.shift => {
                        this.prev_tab(cx);
                    }
                    "tab" if modifiers.control => {
                        this.next_tab(cx);
                    }
                    "b" if (modifiers.platform || modifiers.control) && modifiers.alt => {
                        if this.blame_provider.is_some() {
                            this.toggle_blame(cx);
                            let visible = this.blame_visible();
                            cx.emit(super::FileViewerEvent::BlamePreferenceChanged(visible));
                        }
                    }
                    "b" if !modifiers.platform && !modifiers.control => {
                        this.toggle_sidebar(cx);
                    }
                    "c" if modifiers.platform || modifiers.control => {
                        if is_img_view || is_font_tab {
                            // No text selection while previewing an image or font.
                        } else if is_preview {
                            this.copy_markdown_selection(cx);
                        } else {
                            this.copy_selection(cx);
                        }
                    }
                    "a" if modifiers.platform || modifiers.control => {
                        if is_img_view || is_font_tab {
                            // Nothing to select.
                        } else if is_preview {
                            this.select_all_markdown(cx);
                        } else {
                            this.select_all(cx);
                        }
                    }
                    "w" if modifiers.platform || modifiers.control => {
                        this.close_active_tab(cx);
                    }
                    "0" if (modifiers.platform || modifiers.control) && img_zoom_chord => {
                        this.image_fit(cx);
                    }
                    "=" | "+" if (modifiers.platform || modifiers.control) && img_zoom_chord => {
                        this.image_zoom_by(1.25, cx);
                    }
                    "-" if (modifiers.platform || modifiers.control) && img_zoom_chord => {
                        this.image_zoom_by(1.0 / 1.25, cx);
                    }
                    "r" if !modifiers.platform && !modifiers.control => {
                        this.refresh_file_tree_async(cx);
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
            .child({
                let detached = self.is_detached;
                let needs_controls = detached && detached_needs_controls(window);
                let is_maximized = window.is_maximized();
                div()
                    .px(px(16.0))
                    .py(if detached { px(6.0) } else { px(12.0) })
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
                            .child(if detached {
                                // Compact single-line title for detached windows
                                h_flex()
                                    .gap(px(8.0))
                                    .min_w_0()
                                    .child(
                                        div()
                                            .text_size(ui_text(13.0, cx))
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(rgb(t.text_primary))
                                            .text_ellipsis()
                                            .overflow_hidden()
                                            .child(filename),
                                    )
                                    .child(
                                        div()
                                            .text_size(ui_text_sm(cx))
                                            .text_color(rgb(t.text_muted))
                                            .text_ellipsis()
                                            .overflow_hidden()
                                            .min_w_0()
                                            .child(relative_path),
                                    )
                                    .into_any_element()
                            } else {
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
                                    )
                                    .into_any_element()
                            }),
                    )
                    // Drag-to-move spacer (only meaningful when detached)
                    .child(window_drag_spacer(detached))
                    .child(
                        h_flex()
                            .gap(px(12.0))
                            .when(self.blame_provider.is_some() && !is_image && !is_font, |d| {
                                let on = self.blame_visible;
                                d.child(
                                    div()
                                        .id("blame-toggle")
                                        .cursor_pointer()
                                        .px(px(8.0))
                                        .py(px(4.0))
                                        .rounded(px(4.0))
                                        .bg(rgb(if on { t.bg_selection } else { t.bg_secondary }))
                                        .hover(|s| s.bg(rgb(t.bg_hover)))
                                        .tooltip(|window, cx| {
                                            gpui_component::tooltip::Tooltip::new("Toggle git blame").build(window, cx)
                                        })
                                        .on_click(cx.listener(|this, _, _window, cx| {
                                            this.toggle_blame(cx);
                                            let visible = this.blame_visible();
                                            cx.emit(super::FileViewerEvent::BlamePreferenceChanged(visible));
                                        }))
                                        .child(
                                            div()
                                                .text_size(ui_text_sm(cx))
                                                .text_color(rgb(if on {
                                                    t.text_primary
                                                } else {
                                                    t.text_muted
                                                }))
                                                .child("Blame"),
                                        ),
                                )
                            })
                            .when(supports_view_toggle, |d| {
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
                            .when(show_image, |d| {
                                let view = self.active_tab().image_view.clone();
                                let zoom_label = if view.auto_fit {
                                    "Fit".to_string()
                                } else {
                                    format!("{}%", (view.zoom * 100.0).round() as i32)
                                };
                                let bg = view.background;
                                d.child(image_zoom_controls(zoom_label, &t, cx))
                                    .child(image_background_toggle(bg, &t, cx))
                            })
                            .when(!self.is_detached, |d| {
                                d.child(
                                    div()
                                        .id("detach-button")
                                        .cursor_pointer()
                                        .w(px(28.0))
                                        .h(px(28.0))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .rounded(px(4.0))
                                        .hover(|s| s.bg(rgb(t.bg_secondary)))
                                        .tooltip(|window, cx| {
                                            gpui_component::tooltip::Tooltip::new("Open in new window").build(window, cx)
                                        })
                                        .on_click(cx.listener(|this, _, _window, cx| {
                                            this.request_detach(cx);
                                        }))
                                        .child(
                                            svg()
                                                .path("icons/external-link.svg")
                                                .size(px(14.0))
                                                .text_color(rgb(t.text_muted)),
                                        ),
                                )
                            })
                            .when(detached, |d| {
                                d.child(window_min_max_controls(needs_controls, is_maximized, &t, cx))
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
                    )
            })
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
                            // In-file search bar
                            .when(self.search_state.is_some(), |d| {
                                d.child(self.render_search_bar(&t, cx))
                            })
                            .when(tab_loading, |d| {
                                d.child(
                                    div()
                                        .flex_1()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .child(
                                            div()
                                                .text_size(ui_text_sm(cx))
                                                .text_color(rgb(t.text_muted))
                                                .child("Loading…"),
                                        ),
                                )
                            })
                            .when(!tab_loading && has_error, |d| {
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
                            .when(!tab_loading && !has_error && show_image, |d| {
                                d.child(self.render_image_preview(&t, cx))
                            })
                            .when(!tab_loading && !has_error && show_font, |d| {
                                d.child(render_font_preview(font_data.clone(), &t, cx))
                            })
                            .when(!tab_loading && !has_error && show_source, |d| {
                                // SVG in Source mode with no decoded XML (loader
                                // got non-UTF-8 bytes) would render as a blank
                                // pane with zero lines — surface a placeholder
                                // instead of a silent empty box.
                                if is_svg && line_count == 0 {
                                    return d.child(
                                        div()
                                            .flex_1()
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .bg(rgb(t.bg_secondary))
                                            .child(
                                                div()
                                                    .text_size(ui_text_sm(cx))
                                                    .text_color(rgb(t.text_muted))
                                                    .child(
                                                        "SVG source is not valid UTF-8 — switch back to Preview.",
                                                    ),
                                            ),
                                    );
                                }
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
                                        .when_some(
                                            scrollbar_geometry,
                                            |d, (_, _, thumb_y, thumb_height)| {
                                                d.child(self.render_scrollbar(
                                                    &t,
                                                    thumb_y,
                                                    thumb_height,
                                                    is_dragging_scrollbar,
                                                    cx,
                                                ))
                                            },
                                        ),
                                )
                            })
                            .when(!tab_loading && !has_error && show_md_preview, |d| {
                                let Some(list_state) = markdown_list_state.clone() else {
                                    return d;
                                };
                                let view = cx.entity().clone();
                                let md_list = list(list_state.clone(), move |idx, _window, cx| {
                                    view.update(cx, |this, cx| {
                                        let t = theme(cx);
                                        let node_idx = idx;
                                        let selection = this
                                            .active_tab()
                                            .markdown_selection
                                            .normalized_non_empty();
                                        let Some(rendered_node) = this
                                            .active_tab()
                                            .markdown_doc
                                            .as_ref()
                                            .and_then(|doc| doc.render_node(idx, &t, cx, selection))
                                        else {
                                            return div().into_any_element();
                                        };
                                        let element = match rendered_node {
                                        RenderedNode::Simple {
                                            div: node_div,
                                            start_offset,
                                            end_offset,
                                        } => {
                                            let node_end = end_offset.saturating_sub(1);
                                            let idx = node_idx;
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
                                                            if tab.markdown_selection.is_selecting
                                                                && let Some(sel_start) =
                                                                    tab.markdown_selection.start
                                                                {
                                                                    let new_end = if start_offset
                                                                        >= sel_start
                                                                    {
                                                                        Some(node_end)
                                                                    } else {
                                                                        Some(start_offset)
                                                                    };
                                                                    if tab.markdown_selection.end
                                                                        != new_end
                                                                    {
                                                                        tab.markdown_selection
                                                                            .end = new_end;
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
                                                    .into_any_element()
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
                                                            if tab.markdown_selection.is_selecting
                                                                && let Some(sel_start) = tab.markdown_selection.start {
                                                                    let new_end = if start_offset >= sel_start {
                                                                        Some(line_end)
                                                                    } else {
                                                                        Some(start_offset)
                                                                    };
                                                                    if tab.markdown_selection.end != new_end {
                                                                        tab.markdown_selection.end = new_end;
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
                                                    .overflow_x_scroll()
                                                    .child(
                                                        div()
                                                            .p(px(12.0))
                                                            .font_family("monospace")
                                                            .text_size(ui_text(
                                                                this.file_font_size,
                                                                cx,
                                                            ))
                                                            .text_color(rgb(t.text_secondary))
                                                            .flex()
                                                            .flex_col()
                                                            .children(line_children),
                                                    );

                                            code_block.into_any_element()
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
                                                            if tab.markdown_selection.is_selecting
                                                                && let Some(sel_start) = tab.markdown_selection.start {
                                                                    let new_end = if start_offset >= sel_start {
                                                                        Some(row_end)
                                                                    } else {
                                                                        Some(start_offset)
                                                                    };
                                                                    if tab.markdown_selection.end != new_end {
                                                                        tab.markdown_selection.end = new_end;
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
                                                            if tab.markdown_selection.is_selecting
                                                                && let Some(sel_start) = tab.markdown_selection.start {
                                                                    let new_end = if start_offset >= sel_start {
                                                                        Some(row_end)
                                                                    } else {
                                                                        Some(start_offset)
                                                                    };
                                                                    if tab.markdown_selection.end != new_end {
                                                                        tab.markdown_selection.end = new_end;
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
                                                .overflow_x_scroll()
                                                .children(table_rows);

                                            table.into_any_element()
                                        }
                                        };
                                        // Per-block wrapper carries the spacing
                                        // and max width the old container provided.
                                        div()
                                            .w_full()
                                            .max_w(px(900.0))
                                            .pb(px(12.0))
                                            .child(element)
                                            .into_any_element()
                                    })
                                });

                                d.child(
                                    div()
                                        .id("markdown-preview")
                                        .relative()
                                        .flex_1()
                                        .min_h_0()
                                        .p(px(16.0))
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
                                        .child(md_list.w_full().h_full())
                                        .vertical_scrollbar(&list_state),
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
            .when_some(
                self.filter_popover_open
                    .then_some(self.filter_button_bounds)
                    .flatten(),
                |d, bounds| {
                    let entity = cx.entity().downgrade();
                    d.child(crate::list_overlay::file_filter_popover(
                        bounds, self.show_ignored, &t, cx,
                        move |filter, _, cx| {
                            if let Some(e) = entity.upgrade() {
                                e.update(cx, |this, cx| this.toggle_filter(filter, cx));
                            }
                        },
                    ))
                },
            )
            .when_some(self.render_context_menu(&t, cx), |d, menu| d.child(menu))
            .when_some(self.render_tab_context_menu(&t, cx), |d, menu| d.child(menu))
            .when_some(self.render_selection_context_menu(&t, cx), |d, menu| d.child(menu))
            .when_some(self.render_delete_confirm(&t, cx), |d, dialog| d.child(dialog))
    }
}

impl FileViewer {
    /// Right-click context menu over a non-empty text selection. Offers
    /// "Send to Terminal" and "Copy".
    fn render_selection_context_menu(
        &self,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let position = self.selection_context_menu?;
        self.active_tab().selection.normalized_non_empty()?;

        let panel = okena_ui::menu::context_menu_panel("fv-selection-context-menu", t)
            .child(
                okena_ui::menu::menu_item(
                    "fv-sel-ctx-send",
                    "icons/terminal.svg",
                    "Send to Terminal",
                    t,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.selection_context_menu = None;
                    this.send_selection_to_terminal(cx);
                })),
            )
            .child(okena_ui::menu::menu_separator(t))
            .child(
                okena_ui::menu::menu_item("fv-sel-ctx-copy", "icons/copy.svg", "Copy", t)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.selection_context_menu = None;
                        this.copy_selection(cx);
                    })),
            );

        Some(
            div()
                .id("fv-selection-context-menu-backdrop")
                .absolute()
                .inset_0()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.selection_context_menu = None;
                        cx.notify();
                    }),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(|this, _, _, cx| {
                        this.selection_context_menu = None;
                        cx.notify();
                    }),
                )
                .child(deferred(
                    anchored().position(position).snap_to_window().child(panel),
                ))
                .into_any_element(),
        )
    }
}

/// Render the font-preview body: a pangram in descending sizes followed by
/// a metadata table. `data` is None while the loader is still running or
/// if the font failed to parse (caller already gates on error state, so
/// None here means the font Arc was somehow dropped — render a placeholder).
fn render_font_preview(
    data: Option<Arc<FontData>>,
    t: &ThemeColors,
    cx: &mut Context<FileViewer>,
) -> impl IntoElement {
    let Some(data) = data else {
        return div()
            .id("font-empty")
            .flex_1()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgb(t.bg_secondary))
            .child(
                div()
                    .text_size(ui_text_sm(cx))
                    .text_color(rgb(t.text_muted))
                    .child("Font not available"),
            )
            .into_any_element();
    };

    let family: SharedString = data.family_name.clone().into();
    let sample = "The quick brown fox jumps over the lazy dog";
    let sample_sizes_px: [f32; 5] = [48.0, 32.0, 24.0, 18.0, 14.0];

    let sample_block = v_flex()
        .gap(px(20.0))
        .children(sample_sizes_px.iter().map(|&size| {
            div()
                .text_size(px(size))
                .text_color(rgb(t.text_primary))
                .font_family(family.clone())
                .child(sample.to_string())
        }));

    let metadata_rows: Vec<(&str, String)> = vec![
        ("Family", data.family_name.clone()),
        ("Full name", data.full_name.clone()),
        ("Style", data.style.clone()),
        ("Weight class", data.weight_class.to_string()),
        ("Italic", if data.is_italic { "yes" } else { "no" }.to_string()),
        ("Glyphs", data.num_glyphs.to_string()),
        ("Units per em", data.units_per_em.to_string()),
        ("Version", if data.version.is_empty() { "—".to_string() } else { data.version.clone() }),
    ];

    let metadata_block = v_flex()
        .gap(px(6.0))
        .children(metadata_rows.into_iter().map(|(label, value)| {
            h_flex()
                .gap(px(16.0))
                .child(
                    div()
                        .w(px(120.0))
                        .text_size(ui_text_sm(cx))
                        .text_color(rgb(t.text_muted))
                        .child(label),
                )
                .child(
                    div()
                        .text_size(ui_text_sm(cx))
                        .text_color(rgb(t.text_primary))
                        .child(value),
                )
        }));

    div()
        .id("font-preview")
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .bg(rgb(t.bg_secondary))
        .child(
            v_flex()
                .gap(px(32.0))
                .p(px(32.0))
                .child(sample_block)
                .child(
                    div()
                        .h(px(1.0))
                        .bg(rgb(t.border))
                        .w_full(),
                )
                .child(metadata_block),
        )
        .into_any_element()
}
