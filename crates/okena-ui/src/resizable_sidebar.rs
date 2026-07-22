use crate::resize_handle::ResizeHandle;
use gpui::*;

pub const DEFAULT_SIDEBAR_WIDTH: f32 = 240.0;
pub const MIN_SIDEBAR_WIDTH: f32 = 150.0;
pub const MAX_SIDEBAR_WIDTH: f32 = 500.0;

#[derive(Clone, Copy, Debug, PartialEq)]
struct ResizeDrag {
    start_x: f32,
    start_width: f32,
}

/// Width and transient drag state shared by resizable sidebars.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResizableSidebarState {
    width: f32,
    drag: Option<ResizeDrag>,
}

impl Default for ResizableSidebarState {
    fn default() -> Self {
        Self {
            width: DEFAULT_SIDEBAR_WIDTH,
            drag: None,
        }
    }
}

impl ResizableSidebarState {
    pub fn width(&self) -> f32 {
        self.width
    }

    pub fn start_resize(&mut self, mouse_x: f32) {
        self.drag = Some(ResizeDrag {
            start_x: mouse_x,
            start_width: self.width,
        });
    }

    /// Update the width from the active drag. Returns whether it changed.
    pub fn update_resize(&mut self, mouse_x: f32) -> bool {
        let Some(drag) = self.drag else {
            return false;
        };
        let width =
            (drag.start_width + mouse_x - drag.start_x).clamp(MIN_SIDEBAR_WIDTH, MAX_SIDEBAR_WIDTH);
        if width == self.width {
            return false;
        }
        self.width = width;
        true
    }

    pub fn end_resize(&mut self) {
        self.drag = None;
    }
}

/// Render a fixed-width sidebar and its vertical resize divider.
pub fn resizable_sidebar(
    width: f32,
    background_color: u32,
    border_color: u32,
    border_active_color: u32,
    children: Vec<AnyElement>,
    on_drag_start: impl FnOnce(Point<Pixels>, &mut App) + 'static,
    on_drag_end: impl FnOnce(&mut App) + 'static,
) -> Div {
    div()
        .relative()
        .h_full()
        .flex()
        .flex_shrink_0()
        .child(
            div()
                .w(px(width))
                .h_full()
                .bg(rgb(background_color))
                .flex()
                .flex_col()
                .children(children),
        )
        .child(ResizeHandle::new(
            false,
            border_color,
            border_active_color,
            on_drag_start,
        ))
        // Window-level mouse-up survives the divider's blocking hitbox.
        .child(
            canvas(
                |_bounds, _window, _cx| {},
                move |_bounds, _state, window, _cx| {
                    let mut on_drag_end = Some(on_drag_end);
                    window.on_mouse_event(move |event: &MouseUpEvent, phase, _window, cx| {
                        if phase == DispatchPhase::Bubble
                            && event.button == MouseButton::Left
                            && let Some(callback) = on_drag_end.take()
                        {
                            callback(cx);
                        }
                    });
                },
            )
            .absolute()
            .size_full(),
        )
}

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_SIDEBAR_WIDTH, MAX_SIDEBAR_WIDTH, MIN_SIDEBAR_WIDTH, ResizableSidebarState,
    };

    #[test]
    fn resize_tracks_mouse_from_initial_width() {
        let mut state = ResizableSidebarState::default();
        state.start_resize(100.0);

        assert!(state.update_resize(145.0));
        assert_eq!(state.width(), DEFAULT_SIDEBAR_WIDTH + 45.0);
    }

    #[test]
    fn resize_clamps_to_bounds() {
        let mut state = ResizableSidebarState::default();
        state.start_resize(100.0);

        assert!(state.update_resize(-1_000.0));
        assert_eq!(state.width(), MIN_SIDEBAR_WIDTH);
        assert!(state.update_resize(1_000.0));
        assert_eq!(state.width(), MAX_SIDEBAR_WIDTH);
    }

    #[test]
    fn resize_stops_after_mouse_up() {
        let mut state = ResizableSidebarState::default();
        state.start_resize(100.0);
        state.end_resize();

        assert!(!state.update_resize(200.0));
        assert_eq!(state.width(), DEFAULT_SIDEBAR_WIDTH);
    }
}
