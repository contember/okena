//! Scrollbar handling for the diff viewer.

use super::types::ScrollbarDrag;
use super::DiffViewer;
use gpui::*;

impl DiffViewer {
    /// Get scrollbar geometry if scrollable.
    /// Returns (viewport_height, content_height, thumb_y, thumb_height).
    pub(super) fn get_scrollbar_geometry(&self) -> Option<(f32, f32, f32, f32)> {
        let state = self.scroll_handle.0.borrow();
        let item_size = state.last_item_size?;

        let viewport_height = f32::from(item_size.item.height);
        let content_height = f32::from(item_size.contents.height);

        if content_height <= viewport_height {
            return None;
        }

        let scroll_offset = state.base_handle.offset();
        let scroll_y = -f32::from(scroll_offset.y);

        let thumb_height = (viewport_height / content_height * viewport_height).max(20.0);
        let scrollable_content = content_height - viewport_height;
        let scrollable_track = viewport_height - thumb_height;
        let scroll_ratio = (scroll_y / scrollable_content).clamp(0.0, 1.0);
        let thumb_y = scroll_ratio * scrollable_track;

        Some((viewport_height, content_height, thumb_y, thumb_height))
    }

    /// Start scrollbar drag.
    pub(super) fn start_scrollbar_drag(&mut self, y: f32, cx: &mut Context<Self>) {
        let state = self.scroll_handle.0.borrow();
        let scroll_y = -f32::from(state.base_handle.offset().y);
        drop(state);

        self.scrollbar_drag = Some(ScrollbarDrag {
            start_y: y,
            start_scroll_y: scroll_y,
        });
        cx.notify();
    }

    /// Update scrollbar during drag.
    pub(super) fn update_scrollbar_drag(&mut self, y: f32, cx: &mut Context<Self>) {
        let Some(drag) = self.scrollbar_drag else {
            return;
        };
        let Some((viewport_height, content_height, _, thumb_height)) = self.get_scrollbar_geometry()
        else {
            return;
        };

        let scrollable_content = content_height - viewport_height;
        let scrollable_track = viewport_height - thumb_height;

        if scrollable_track <= 0.0 {
            return;
        }

        let delta_y = y - drag.start_y;
        let delta_scroll = delta_y * scrollable_content / scrollable_track;
        let new_scroll = (drag.start_scroll_y + delta_scroll).clamp(0.0, scrollable_content);

        let state = self.scroll_handle.0.borrow_mut();
        state.base_handle.set_offset(point(px(0.0), px(-new_scroll)));
        drop(state);

        cx.notify();
    }

    /// End scrollbar drag.
    pub(super) fn end_scrollbar_drag(&mut self, cx: &mut Context<Self>) {
        self.scrollbar_drag = None;
        cx.notify();
    }

    /// Width of a single panel's gutter (line number column + accent + padding).
    fn panel_gutter_width(&self) -> f32 {
        let char_width = self.file_font_size * 0.6;
        let num_col_width = (self.line_num_width as f32) * char_width + 12.0;
        // accent(3) + line number column + separator(1) + content padding(10)
        3.0 + num_col_width + 1.0 + 10.0
    }

    /// Maximum text content width in pixels (just the code text, no gutter).
    pub(super) fn max_text_width(&self) -> f32 {
        let char_width = self.file_font_size * 0.6;
        self.max_line_chars as f32 * char_width
    }

    /// Available text width per panel (viewport minus gutter).
    /// In side-by-side mode, each panel gets half the viewport.
    pub(super) fn available_text_width(&self) -> f32 {
        let vw = self.diff_pane_width.max(100.0);
        let panel_width = if self.effective_view_mode() == super::types::DiffViewMode::SideBySide {
            vw / 2.0
        } else {
            vw
        };
        (panel_width - self.panel_gutter_width()).max(0.0)
    }

    /// Max horizontal scroll range.
    pub(super) fn max_scroll_x(&self) -> f32 {
        (self.max_text_width() - self.available_text_width()).max(0.0)
    }

    /// Get the viewport width from the scroll handle, or use cached value.
    pub(super) fn viewport_width(&mut self) -> f32 {
        let state = self.scroll_handle.0.borrow();
        if let Some(item_size) = &state.last_item_size {
            let w = f32::from(item_size.item.width);
            if w > 0.0 {
                self.diff_pane_width = w;
            }
        }
        drop(state);
        self.diff_pane_width
    }

    /// Effective view mode (forced unified for new/deleted files).
    fn effective_view_mode(&self) -> super::types::DiffViewMode {
        let is_new_or_deleted = self
            .file_stats
            .get(self.selected_file_index)
            .map(|f| f.is_new || f.is_deleted)
            .unwrap_or(false);
        if is_new_or_deleted {
            super::types::DiffViewMode::Unified
        } else {
            self.view_mode
        }
    }

    /// Handle horizontal scroll from a scroll wheel event.
    pub(super) fn handle_scroll_x(
        &mut self,
        event: &ScrollWheelEvent,
        cx: &mut Context<Self>,
    ) {
        let delta_x = match event.delta {
            ScrollDelta::Pixels(point) => f32::from(point.x),
            ScrollDelta::Lines(point) => f32::from(point.x) * 20.0,
        };

        if delta_x.abs() < 0.5 {
            return;
        }

        let max_scroll = self.max_scroll_x();
        self.scroll_x = (self.scroll_x - delta_x).clamp(0.0, max_scroll);
        cx.notify();
    }
}
