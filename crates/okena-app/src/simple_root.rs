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
            // Not `.cached()`: the root is an ancestor of every view, so any
            // notify marks it dirty and its cache would never hit.
            // `view_cache_cascade` pins how the nested caches behave.
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

/// Pins the GPUI view-retention behaviour the window is built on.
///
/// A `cx.notify()` re-renders the notified view and its ancestors. Every other
/// view, the notified view's own cached descendants included, reuses its last
/// frame until it is notified itself, so a view must be notified whenever
/// state it reads outside an entity changes.
#[cfg(test)]
mod view_cache_cascade {
    use gpui::{
        AnyView, AppContext as _, Context, Entity, IntoElement, ParentElement, Render,
        StyleRefinement, Styled, TestAppContext, Window, div,
    };
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const COLUMNS: usize = 2;
    const PANES_PER_COLUMN: usize = 3;

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

    /// Stands in for the `.cached()` chain above a pane: terminal pane ->
    /// layout container -> project column.
    struct Column {
        renders: Arc<AtomicUsize>,
        contents: Vec<Entity<Content>>,
    }

    impl Render for Column {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            self.renders.fetch_add(1, Ordering::SeqCst);
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

    struct Harness {
        columns: Vec<Entity<Column>>,
        column_renders: Vec<Arc<AtomicUsize>>,
        pane_renders: Vec<Vec<Arc<AtomicUsize>>>,
    }

    impl Harness {
        fn new(cx: &mut TestAppContext) -> Self {
            let column_renders: Vec<_> = (0..COLUMNS)
                .map(|_| Arc::new(AtomicUsize::new(0)))
                .collect();
            let pane_renders: Vec<Vec<_>> = (0..COLUMNS)
                .map(|_| {
                    (0..PANES_PER_COLUMN)
                        .map(|_| Arc::new(AtomicUsize::new(0)))
                        .collect()
                })
                .collect();

            let window = {
                let column_renders = column_renders.clone();
                let pane_renders = pane_renders.clone();
                cx.add_window(move |_, cx| {
                    let columns = column_renders
                        .iter()
                        .zip(&pane_renders)
                        .map(|(renders, panes)| {
                            let contents = panes
                                .iter()
                                .map(|renders| {
                                    cx.new(|_| Content {
                                        renders: renders.clone(),
                                    })
                                })
                                .collect();
                            cx.new(|_| Column {
                                renders: renders.clone(),
                                contents,
                            })
                        })
                        .collect();
                    Root { columns }
                })
            };
            let columns = window
                .read_with(cx, |root, _| root.columns.clone())
                .expect("window is open");

            let harness = Self {
                columns,
                column_renders,
                pane_renders,
            };
            // `add_window` already drew once; count from a settled tree.
            harness.reset();
            harness
        }

        fn pane(&self, cx: &mut TestAppContext, column: usize, pane: usize) -> Entity<Content> {
            self.columns[column].read_with(cx, |column, _| column.contents[pane].clone())
        }

        /// Notify inside `cx.update`; the test context draws the dirty window
        /// when the update ends. No extra `draw`: a frame where nothing is
        /// dirty leaves skipped caches a frame stale, and GPUI then re-renders
        /// them as a safety measure.
        fn notify<V: 'static>(&self, cx: &mut TestAppContext, entity: &Entity<V>) {
            cx.update(|cx| entity.update(cx, |_, cx| cx.notify()));
        }

        fn reset(&self) {
            for renders in self
                .column_renders
                .iter()
                .chain(self.pane_renders.iter().flatten())
            {
                renders.store(0, Ordering::SeqCst);
            }
        }

        fn column_renders(&self) -> Vec<usize> {
            self.column_renders
                .iter()
                .map(|c| c.load(Ordering::SeqCst))
                .collect()
        }

        fn pane_renders(&self) -> Vec<Vec<usize>> {
            self.pane_renders
                .iter()
                .map(|column| column.iter().map(|c| c.load(Ordering::SeqCst)).collect())
                .collect()
        }
    }

    #[gpui::test]
    fn a_pane_notify_repaints_only_that_pane(cx: &mut TestAppContext) {
        let harness = Harness::new(cx);
        let notified = harness.pane(cx, 0, 0);

        for frame in 0..3 {
            harness.notify(cx, &notified);
            assert_eq!(
                harness.pane_renders(),
                vec![vec![1, 0, 0], vec![0, 0, 0]],
                "frame {frame}: only the notified pane repaints"
            );
            assert_eq!(
                harness.column_renders(),
                vec![1, 0],
                "frame {frame}: the dirty ancestor column re-renders, the sibling column stays cached"
            );
            harness.reset();
        }
    }

    #[gpui::test]
    fn a_column_notify_repaints_only_that_column(cx: &mut TestAppContext) {
        let harness = Harness::new(cx);
        let column = harness.columns[1].clone();

        harness.notify(cx, &column);

        assert_eq!(harness.column_renders(), vec![0, 1]);
        assert_eq!(
            harness.pane_renders(),
            vec![vec![0; PANES_PER_COLUMN], vec![0; PANES_PER_COLUMN]],
            "a notified view's cached panes reuse their frame until they are notified themselves"
        );
    }
}
