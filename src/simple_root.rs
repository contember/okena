//! Simple root wrapper with CSD resize support (fixes Linux/Wayland maximize issue
//! from gpui_component's window_border while still enabling window resize).

use gpui::{
    AnyView, Bounds, Context, CursorStyle, Decorations, HitboxBehavior, InteractiveElement,
    IntoElement, MouseButton, ParentElement, Pixels, Point, Render, ResizeEdge, Size, Stateful,
    Styled, Window, canvas, div, point, prelude::FluentBuilder, px,
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
            .when(matches!(decorations, Decorations::Client { .. }), |div: Stateful<gpui::Div>| {
                div.child(
                    canvas(
                        |_bounds, window, _cx| {
                            window.insert_hitbox(
                                Bounds::new(
                                    point(px(0.0), px(0.0)),
                                    window.window_bounds().get_bounds().size,
                                ),
                                HitboxBehavior::Normal,
                            )
                        },
                        move |_bounds, hitbox, window, _cx| {
                            let mouse = window.mouse_position();
                            let size = window.window_bounds().get_bounds().size;
                            if let Some(edge) = detect_resize_edge(mouse, size) {
                                window.set_cursor_style(
                                    match edge {
                                        ResizeEdge::Top | ResizeEdge::Bottom => {
                                            CursorStyle::ResizeUpDown
                                        }
                                        ResizeEdge::Left | ResizeEdge::Right => {
                                            CursorStyle::ResizeLeftRight
                                        }
                                        ResizeEdge::TopLeft | ResizeEdge::BottomRight => {
                                            CursorStyle::ResizeUpLeftDownRight
                                        }
                                        ResizeEdge::TopRight | ResizeEdge::BottomLeft => {
                                            CursorStyle::ResizeUpRightDownLeft
                                        }
                                    },
                                    &hitbox,
                                );
                            }
                        },
                    )
                    .size_full()
                    .absolute(),
                )
                .on_mouse_down(MouseButton::Left, move |_, window: &mut Window, _cx: &mut gpui::App| {
                    let size = window.window_bounds().get_bounds().size;
                    let pos = window.mouse_position();
                    if let Some(edge) = detect_resize_edge(pos, size) {
                        window.start_window_resize(edge);
                    }
                })
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
