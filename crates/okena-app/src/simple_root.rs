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
        Self { view: view.into() }
    }
}

impl Render for SimpleRoot {
    fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let decorations = window.window_decorations();

        div()
            .id("simple-root")
            .size_full()
            .when(
                matches!(decorations, Decorations::Client { .. }),
                |div: gpui::Stateful<gpui::Div>| {
                    div.child(
                        canvas(
                            |_bounds, window, _cx| {
                                let size = window.window_bounds().get_bounds().size;
                                let e = RESIZE_EDGE_SIZE;
                                // Create 4 edge-only hitboxes (top, bottom, left, right strips)
                                [
                                    window.insert_hitbox(
                                        Bounds::new(
                                            point(px(0.0), px(0.0)),
                                            Size {
                                                width: size.width,
                                                height: e,
                                            },
                                        ),
                                        HitboxBehavior::Normal,
                                    ),
                                    window.insert_hitbox(
                                        Bounds::new(
                                            point(px(0.0), size.height - e),
                                            Size {
                                                width: size.width,
                                                height: e,
                                            },
                                        ),
                                        HitboxBehavior::Normal,
                                    ),
                                    window.insert_hitbox(
                                        Bounds::new(
                                            point(px(0.0), e),
                                            Size {
                                                width: e,
                                                height: size.height - e * 2.0,
                                            },
                                        ),
                                        HitboxBehavior::Normal,
                                    ),
                                    window.insert_hitbox(
                                        Bounds::new(
                                            point(size.width - e, e),
                                            Size {
                                                width: e,
                                                height: size.height - e * 2.0,
                                            },
                                        ),
                                        HitboxBehavior::Normal,
                                    ),
                                ]
                            },
                            move |_bounds, hitboxes: [Hitbox; 4], window, _cx| {
                                let mouse = window.mouse_position();
                                let size = window.window_bounds().get_bounds().size;
                                if let Some(edge) = detect_resize_edge(mouse, size) {
                                    let cursor = match edge {
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
                                    };
                                    for hitbox in &hitboxes {
                                        window.set_cursor_style(cursor, hitbox);
                                    }
                                }

                                // Handle mouse down on edge hitboxes for resize
                                let hitbox_ids: [_; 4] = std::array::from_fn(|i| hitboxes[i].id);
                                window.on_mouse_event(
                                    move |e: &MouseDownEvent, phase, window, cx| {
                                        if phase != DispatchPhase::Bubble
                                            || e.button != MouseButton::Left
                                        {
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
                                    },
                                );
                            },
                        )
                        .size_full()
                        .absolute(),
                    )
                },
            )
            // NOTE: do NOT wrap the view in `.cached()`. The root is the common
            // ancestor of every view, so any descendant invalidation marks it
            // dirty (GPUI ancestor-marking) — its cache would never hit. Worse,
            // a cached view re-rendering sets `window.refreshing = true` for its
            // entire subtree, which bypasses EVERY nested `.cached()` (project
            // columns, terminal panes). Caching the root therefore defeats all
            // nested caching: render-stats showed every column and pane
            // re-rendering on every window draw. Render the root uncached so
            // nested caches actually work and only dirty views re-render.
            // `tests::view_cache_cascade` pins both halves of this.
            .child(self.view.clone())
    }
}

fn detect_resize_edge(pos: Point<Pixels>, size: Size<Pixels>) -> Option<ResizeEdge> {
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

/// Pins the GPUI view-caching behaviour the terminal grid cache is sized for.
///
/// One `cx.notify()` on a terminal's content view (what
/// `TerminalContent::request_activity_repaint` does on every activity tick)
/// does *not* repaint only that pane: GPUI marks the whole ancestor path dirty
/// (`Window::mark_view_dirty`), and a dirty `.cached()` ancestor re-renders with
/// `window.refreshing = true` for its entire subtree, bypassing every nested
/// `.cached()`. So the notified pane's whole project column repaints — which is
/// exactly why `TerminalRenderCache` exists, and why the cache's value scales
/// with panes-per-column rather than being a fixed percentage.
///
/// Sibling *columns* stay cached. If that ever stops holding, the cost of one
/// terminal's output grows to the whole window and this needs revisiting.
#[cfg(test)]
mod view_cache_cascade {
    use gpui::{
        AnyView, AppContext as _, Context, Entity, IntoElement, ParentElement, Render,
        StyleRefinement, Styled, TestAppContext, Window, div,
    };
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Stands in for `TerminalContent` — the view that gets notified.
    struct Content {
        renders: Arc<AtomicUsize>,
    }

    impl Render for Content {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            self.renders.fetch_add(1, Ordering::SeqCst);
            div().size_full()
        }
    }

    /// Stands in for the `.cached()` chain above it: terminal pane -> layout
    /// container -> project column. One level is enough; `refreshing` propagates.
    struct Column {
        contents: Vec<Entity<Content>>,
    }

    impl Render for Column {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div().size_full().children(
                self.contents.iter().map(|c| {
                    AnyView::from(c.clone()).cached(StyleRefinement::default().size_full())
                }),
            )
        }
    }

    /// Stands in for `SimpleRoot`: uncached, one `.cached()` child per column.
    struct Root {
        columns: Vec<Entity<Column>>,
    }

    impl Render for Root {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div().size_full().children(
                self.columns.iter().map(|c| {
                    AnyView::from(c.clone()).cached(StyleRefinement::default().size_full())
                }),
            )
        }
    }

    #[gpui::test]
    fn one_notify_repaints_its_whole_column_and_no_other(cx: &mut TestAppContext) {
        const COLUMNS: usize = 2;
        const PANES_PER_COLUMN: usize = 3;

        let counters: Vec<Vec<Arc<AtomicUsize>>> = (0..COLUMNS)
            .map(|_| {
                (0..PANES_PER_COLUMN)
                    .map(|_| Arc::new(AtomicUsize::new(0)))
                    .collect()
            })
            .collect();

        let mut notified = None;
        let window = {
            let counters = counters.clone();
            let notified = &mut notified;
            cx.add_window(move |_, cx| {
                let columns = counters
                    .iter()
                    .enumerate()
                    .map(|(col_idx, column)| {
                        let contents: Vec<_> = column
                            .iter()
                            .enumerate()
                            .map(|(pane_idx, renders)| {
                                let content = cx.new(|_| Content {
                                    renders: renders.clone(),
                                });
                                if (col_idx, pane_idx) == (0, 0) {
                                    *notified = Some(content.clone());
                                }
                                content
                            })
                            .collect();
                        cx.new(|_| Column { contents })
                    })
                    .collect();
                Root { columns }
            })
        };
        let notified = notified.expect("first pane registered");

        // `add_window` already drew once; start counting from a settled tree.
        cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();
        for renders in counters.iter().flatten() {
            renders.store(0, Ordering::SeqCst);
        }

        cx.update(|cx| notified.update(cx, |_, cx| cx.notify()));
        cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();

        let repaints: Vec<Vec<usize>> = counters
            .iter()
            .map(|column| column.iter().map(|c| c.load(Ordering::SeqCst)).collect())
            .collect();

        assert_eq!(
            repaints[0],
            vec![1; PANES_PER_COLUMN],
            "notifying one pane repaints every pane in its column"
        );
        assert_eq!(
            repaints[1],
            vec![0; PANES_PER_COLUMN],
            "other columns stay cached"
        );
    }
}
