//! Reusable popover components for anchored floating panels.
//!
//! Provides anchored positioning and styled panel container.
//! Uses GPUI's `deferred(anchored().position().snap_to_window())` pattern.

use crate::theme::ThemeColors;
use gpui::*;

/// Styled popover panel container with standard look: bg, border, rounded corners, shadow.
///
/// Stops mouse-down and scroll-wheel propagation to prevent interaction with elements underneath.
pub fn popover_panel(id: impl Into<SharedString>, t: &ThemeColors) -> Stateful<Div> {
    div()
        .id(ElementId::Name(id.into()))
        .occlude()
        .bg(rgb(t.bg_primary))
        .border_1()
        .border_color(rgb(t.border))
        .rounded(px(6.0))
        .shadow_lg()
        .p(px(8.0))
        .on_mouse_down(MouseButton::Left, |_, _, cx| {
            cx.stop_propagation();
        })
        .on_scroll_wheel(|_, _, cx| {
            cx.stop_propagation();
        })
}

/// Stores a popover trigger's bounds, measured while the trigger is drawn.
///
/// The popover is placed from the bounds of the previous draw, so when the trigger of an
/// open popover moves, this requests one more frame of the view that renders the popover.
/// Call it from that view's elements.
pub fn record_trigger_bounds(
    slot: &mut Bounds<Pixels>,
    bounds: Bounds<Pixels>,
    popover_open: bool,
    window: &Window,
) {
    if popover_open && *slot != bounds {
        window.request_animation_frame();
    }
    *slot = bounds;
}

#[cfg(test)]
mod tests {
    use super::record_trigger_bounds;
    use gpui::{
        AnyView, AppContext as _, Bounds, Context, Entity, IntoElement, ParentElement, Pixels,
        Render, StyleRefinement, Styled, TestAppContext, Window, canvas, div, px,
    };

    struct Trigger {
        bounds: Bounds<Pixels>,
        popover_open: bool,
        offset: Pixels,
        renders: usize,
    }

    impl Render for Trigger {
        fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.renders += 1;
            let entity = cx.entity();
            div().pl(self.offset).child(
                canvas(
                    move |bounds, window, app| {
                        entity.update(app, |this, _| {
                            record_trigger_bounds(
                                &mut this.bounds,
                                bounds,
                                this.popover_open,
                                window,
                            );
                        });
                    },
                    |_, _, _, _| {},
                )
                .size(px(10.)),
            )
        }
    }

    struct Root {
        trigger: Entity<Trigger>,
    }

    impl Render for Root {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(
                AnyView::from(self.trigger.clone()).cached(StyleRefinement::default().size_full()),
            )
        }
    }

    #[gpui::test]
    fn an_open_popover_renders_once_more_after_its_trigger_moves(cx: &mut TestAppContext) {
        let trigger = cx.new(|_| Trigger {
            bounds: Bounds::default(),
            popover_open: true,
            offset: px(0.),
            renders: 0,
        });
        let window = cx.add_window({
            let trigger = trigger.clone();
            move |_, _| Root { trigger }
        });
        let next_frames = |cx: &mut TestAppContext| {
            for _ in 0..3 {
                cx.update_window(window.into(), |_, window, cx| {
                    window.simulate_next_frame(cx);
                })
                .unwrap();
            }
        };
        let renders = |cx: &mut TestAppContext| cx.read(|cx| trigger.read(cx).renders);
        let move_trigger = |cx: &mut TestAppContext, offset: f32| {
            cx.update(|cx| {
                trigger.update(cx, |this, cx| {
                    this.offset = px(offset);
                    cx.notify();
                })
            });
        };
        next_frames(cx);

        let before = renders(cx);
        move_trigger(cx, 20.);
        next_frames(cx);
        assert_eq!(
            renders(cx),
            before + 2,
            "the move, then one frame to re-place"
        );

        cx.update(|cx| trigger.update(cx, |this, _| this.popover_open = false));
        let before = renders(cx);
        move_trigger(cx, 40.);
        next_frames(cx);
        assert_eq!(
            renders(cx),
            before + 1,
            "a closed popover needs no extra frame"
        );
    }
}
