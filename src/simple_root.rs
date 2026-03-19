//! Simple root wrapper with CSD resize support (fixes Linux/Wayland maximize issue
//! from gpui_component's window_border while still enabling window resize).

use gpui::{
    AnyView, Bounds, Context, CursorStyle, Decorations, DispatchPhase, Hitbox, HitboxBehavior,
    InteractiveElement, IntoElement, MouseButton, MouseDownEvent, ParentElement, Pixels, Point,
    Render, ResizeEdge, Size, Styled, Window, canvas, div, point, prelude::FluentBuilder, px,
};

/// Edge detection zone size (pixels) for CSD resize handles.
const RESIZE_EDGE_SIZE: Pixels = px(8.0);

/// Simple root view wrapper that provides CSD resize edges without the buggy
/// shadow/maximize behavior of gpui_component's window_border.
pub struct SimpleRoot {
    view: AnyView,
}

impl SimpleRoot {
    pub fn new(view: impl Into<AnyView>, _window: &mut Window, _cx: &mut Context<Self>) -> Self {
        Self {
            view: view.into(),
        }
    }
}

impl Render for SimpleRoot {
    fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let decorations = window.window_decorations();

        div()
            .id("simple-root")
            .size_full()
            .when(matches!(decorations, Decorations::Client { .. }), |div: gpui::Stateful<gpui::Div>| {
                div.child(
                    canvas(
                        |_bounds, window, _cx| {
                            let size = window.window_bounds().get_bounds().size;
                            let e = RESIZE_EDGE_SIZE;
                            // Create 4 edge-only hitboxes (top, bottom, left, right strips)
                            [
                                window.insert_hitbox(
                                    Bounds::new(point(px(0.0), px(0.0)), Size { width: size.width, height: e }),
                                    HitboxBehavior::Normal,
                                ),
                                window.insert_hitbox(
                                    Bounds::new(point(px(0.0), size.height - e), Size { width: size.width, height: e }),
                                    HitboxBehavior::Normal,
                                ),
                                window.insert_hitbox(
                                    Bounds::new(point(px(0.0), e), Size { width: e, height: size.height - e * 2.0 }),
                                    HitboxBehavior::Normal,
                                ),
                                window.insert_hitbox(
                                    Bounds::new(point(size.width - e, e), Size { width: e, height: size.height - e * 2.0 }),
                                    HitboxBehavior::Normal,
                                ),
                            ]
                        },
                        move |_bounds, hitboxes: [Hitbox; 4], window, _cx| {
                            let mouse = window.mouse_position();
                            let size = window.window_bounds().get_bounds().size;
                            if let Some(edge) = detect_resize_edge(mouse, size) {
                                let cursor = match edge {
                                    ResizeEdge::Top | ResizeEdge::Bottom => CursorStyle::ResizeUpDown,
                                    ResizeEdge::Left | ResizeEdge::Right => CursorStyle::ResizeLeftRight,
                                    ResizeEdge::TopLeft | ResizeEdge::BottomRight => CursorStyle::ResizeUpLeftDownRight,
                                    ResizeEdge::TopRight | ResizeEdge::BottomLeft => CursorStyle::ResizeUpRightDownLeft,
                                };
                                for hitbox in &hitboxes {
                                    window.set_cursor_style(cursor, hitbox);
                                }
                            }

                            // Handle mouse down on edge hitboxes for resize
                            let hitbox_ids: [_; 4] = std::array::from_fn(|i| hitboxes[i].id);
                            window.on_mouse_event(move |e: &MouseDownEvent, phase, window, cx| {
                                if phase != DispatchPhase::Bubble || e.button != MouseButton::Left {
                                    return;
                                }
                                // Only act if mouse is over one of the edge hitboxes
                                if !hitbox_ids.iter().any(|id| id.is_hovered(window)) {
                                    return;
                                }
                                let size = window.window_bounds().get_bounds().size;
                                if let Some(edge) = detect_resize_edge(e.position, size) {
                                    window.start_window_resize(edge);
                                    cx.stop_propagation();
                                }
                            });
                        },
                    )
                    .size_full()
                    .absolute(),
                )
            })
            .child(self.view.clone())
    }
}

fn detect_resize_edge(
    pos: Point<Pixels>,
    size: Size<Pixels>,
) -> Option<ResizeEdge> {
    let edge_size = RESIZE_EDGE_SIZE;
    let edge = if pos.y < edge_size && pos.x < edge_size {
        ResizeEdge::TopLeft
    } else if pos.y < edge_size && pos.x > size.width - edge_size {
        ResizeEdge::TopRight
    } else if pos.y < edge_size {
        ResizeEdge::Top
    } else if pos.y > size.height - edge_size && pos.x < edge_size {
        ResizeEdge::BottomLeft
    } else if pos.y > size.height - edge_size && pos.x > size.width - edge_size {
        ResizeEdge::BottomRight
    } else if pos.y > size.height - edge_size {
        ResizeEdge::Bottom
    } else if pos.x < edge_size {
        ResizeEdge::Left
    } else if pos.x > size.width - edge_size {
        ResizeEdge::Right
    } else {
        return None;
    };
    Some(edge)
}
