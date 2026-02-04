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
}
