//! Tab-local actions that never change the geometry of the tab strip.

use crate::popover::popover_panel;
use crate::theme::ThemeColors;
use gpui::prelude::*;
use gpui::*;

#[derive(Default)]
struct HoverState {
    open: bool,
    trigger: Bounds<Pixels>,
    panel: Bounds<Pixels>,
}

/// Put this in a relatively positioned tab. Only its rightmost 20px opens
/// the actions; it reserves no width, and clicks/drags still reach the tab.
/// The caption is supplied by the caller so it can activate the same tab.
#[derive(IntoElement)]
pub struct TabHoverActions {
    id: ElementId,
    caption: AnyElement,
    actions: Box<dyn FnOnce(&mut App) -> AnyElement>,
    theme: ThemeColors,
}

impl TabHoverActions {
    pub fn new(
        id: impl Into<ElementId>,
        caption: impl IntoElement,
        actions: impl FnOnce(&mut App) -> AnyElement + 'static,
        theme: &ThemeColors,
    ) -> Self {
        Self {
            id: id.into(),
            caption: caption.into_any_element(),
            actions: Box::new(actions),
            theme: *theme,
        }
    }
}

impl RenderOnce for TabHoverActions {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = window.use_keyed_state(self.id.clone(), cx, |_, _| HoverState::default());
        let open = state.read(cx).open && !cx.has_active_drag();
        let trigger_bounds = state.read(cx).trigger;
        let t = &self.theme;

        let trigger = div()
            .id(self.id)
            .absolute()
            .right_0()
            .top_0()
            .w(px(20.0))
            .h_full()
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .opacity(0.0)
            .hover(|s| s.opacity(1.0).bg(rgb(t.bg_hover)))
            .child(
                svg()
                    .path("icons/more-horizontal.svg")
                    .size(px(12.0))
                    .text_color(rgb(t.text_muted)),
            )
            .on_mouse_move({
                let state = state.clone();
                move |event, _, cx| {
                    if event.pressed_button.is_none() && !cx.has_active_drag() {
                        state.update(cx, |state, cx| {
                            if !state.open {
                                state.open = true;
                                cx.notify();
                            }
                        });
                    }
                }
            })
            // Capture the clipped trigger, so scrolling a tab out of view cannot
            // leave an invisible hot spot. Paint registers an outside-move handler
            // for both the trigger and its floating panel, with no hover timers.
            .child(
                canvas(
                    {
                        let state = state.clone();
                        move |bounds, window, cx| {
                            let bounds = bounds.intersect(&window.content_mask().bounds);
                            state.update(cx, |state, cx| {
                                // Scrolling/resizing can move or clip a tab without
                                // a mouse event. Do not leave its panel stranded.
                                if state.open && state.trigger != bounds {
                                    state.open = false;
                                    window.request_animation_frame();
                                    cx.notify();
                                }
                                state.trigger = bounds;
                            });
                        }
                    },
                    {
                        let state = state.clone();
                        move |_, _, window, _| {
                            let state = state.clone();
                            window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                                if phase == DispatchPhase::Capture {
                                    let dragging =
                                        event.pressed_button.is_some() || cx.has_active_drag();
                                    state.update(cx, |state, cx| {
                                        if state.open
                                            && (dragging
                                                || (!state.trigger.contains(&event.position)
                                                    && !state.panel.contains(&event.position)))
                                        {
                                            state.open = false;
                                            cx.notify();
                                        }
                                    });
                                }
                            });
                        }
                    },
                )
                .absolute()
                .size_full(),
            );

        if !open {
            return trigger;
        }

        let space_below = window.viewport_size().height - trigger_bounds.bottom();
        let above = space_below < px(80.0) && trigger_bounds.top() > space_below;
        let (anchor, position, available_height) = if above {
            (
                Anchor::BottomRight,
                trigger_bounds.top_right(),
                trigger_bounds.top(),
            )
        } else {
            (Anchor::TopRight, trigger_bounds.bottom_right(), space_below)
        };
        let panel = popover_panel("tab-hover-panel", t)
            .relative()
            .p(px(4.0))
            .w(px(360.0).min(window.viewport_size().width))
            .max_h(available_height.max(px(0.0)))
            .overflow_y_scroll()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(4.0))
            .child(self.caption)
            .child(
                div()
                    .id("tab-hover-action-scroll")
                    .max_w_full()
                    .overflow_x_scroll()
                    .child((self.actions)(cx)),
            )
            .child(
                canvas(
                    move |bounds, _, cx| state.update(cx, |state, _| state.panel = bounds),
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            );

        trigger.child(deferred(
            anchored()
                .anchor(anchor)
                .position(position)
                .snap_to_window()
                .child(panel),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::TabHoverActions;
    use crate::theme::DARK_THEME;
    use gpui::{
        AppContext as _, Bounds, Context, InputEvent, InteractiveElement, IntoElement, MouseButton,
        MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Point, Render,
        ScrollHandle, StatefulInteractiveElement, Styled, TestAppContext, Window, canvas, div,
        point, px, size,
    };
    use std::{cell::RefCell, rc::Rc};

    #[derive(Default)]
    struct Observed {
        tabs: [Bounds<Pixels>; 2],
        captions: [Option<Bounds<Pixels>>; 2],
        actions: [Option<Bounds<Pixels>>; 2],
        active: usize,
        action_target: Option<usize>,
        dragged: Option<usize>,
        action_builds: [usize; 2],
    }

    struct Tabs(Rc<RefCell<Observed>>, Pixels, ScrollHandle);
    struct DragPreview;

    impl Render for DragPreview {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size(px(10.0))
        }
    }

    impl Render for Tabs {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.0.borrow_mut().captions = [None; 2];
            self.0.borrow_mut().actions = [None; 2];
            div().size_full().pt(self.1).child(
                div()
                    .id("strip")
                    .flex()
                    .h(px(28.0))
                    .overflow_x_scroll()
                    .track_scroll(&self.2)
                    .children((0..2).map(|i| {
                        let on_click = self.0.clone();
                        let on_drag = self.0.clone();
                        let bounds = self.0.clone();
                        let caption_bounds = self.0.clone();
                        let action_bounds = self.0.clone();
                        let caption_click = self.0.clone();
                        let action_click = self.0.clone();
                        let actions_view = cx.entity();
                        div()
                            .id(("tab", i))
                            .relative()
                            .flex_shrink_0()
                            .w(px(160.0))
                            .h_full()
                            .overflow_hidden()
                            .on_click(move |_, _, _| on_click.borrow_mut().active = i)
                            .on_drag(i, move |_, _, _, cx| {
                                on_drag.borrow_mut().dragged = Some(i);
                                cx.new(|_| DragPreview)
                            })
                            .child(
                                canvas(
                                    move |b, _, _| bounds.borrow_mut().tabs[i] = b,
                                    |_, _, _, _| {},
                                )
                                .absolute()
                                .size_full(),
                            )
                            .child(TabHoverActions::new(
                                ("actions", i),
                                div()
                                    .id("caption")
                                    .relative()
                                    .flex_1()
                                    .min_w(px(80.0))
                                    .h(px(20.0))
                                    .on_click(move |_, _, cx| {
                                        caption_click.borrow_mut().active = i;
                                        cx.stop_propagation();
                                    })
                                    .child(
                                        canvas(
                                            move |b, _, _| {
                                                caption_bounds.borrow_mut().captions[i] = Some(b)
                                            },
                                            |_, _, _, _| {},
                                        )
                                        .absolute()
                                        .size_full(),
                                    ),
                                move |cx| {
                                    actions_view.update(cx, |this, _| {
                                        this.0.borrow_mut().action_builds[i] += 1;
                                        div()
                                            .id("action")
                                            .relative()
                                            .w(px(116.0))
                                            .h(px(20.0))
                                            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                                cx.stop_propagation()
                                            })
                                            .on_click(move |_, _, cx| {
                                                action_click.borrow_mut().action_target = Some(i);
                                                cx.stop_propagation();
                                            })
                                            .child(
                                                canvas(
                                                    move |b, _, _| {
                                                        action_bounds.borrow_mut().actions[i] =
                                                            Some(b)
                                                    },
                                                    |_, _, _, _| {},
                                                )
                                                .absolute()
                                                .size_full(),
                                            )
                                            .into_any_element()
                                    })
                                },
                                &DARK_THEME,
                            ))
                    })),
            )
        }
    }

    fn click(window: &mut Window, position: Point<Pixels>, cx: &mut gpui::App) {
        window.dispatch_event(
            MouseDownEvent {
                position,
                button: MouseButton::Left,
                click_count: 1,
                ..Default::default()
            }
            .to_platform_input(),
            cx,
        );
        window.dispatch_event(
            MouseUpEvent {
                position,
                button: MouseButton::Left,
                click_count: 1,
                ..Default::default()
            }
            .to_platform_input(),
            cx,
        );
    }

    #[gpui::test]
    fn hovering_an_inactive_tab_keeps_it_selectable_and_targets_its_actions(
        cx: &mut TestAppContext,
    ) {
        let observed = Rc::new(RefCell::new(Observed::default()));
        let window = cx.open_window(size(px(400.0), px(160.0)), {
            let observed = observed.clone();
            move |_, _| Tabs(observed, px(0.0), ScrollHandle::new())
        });
        cx.update_window(window.into(), |_, window, cx| {
            window.simulate_mouse_move(point(px(390.0), px(150.0)), cx);
        })
        .unwrap();
        let initial = observed.borrow().tabs;
        assert_eq!(observed.borrow().action_builds, [0; 2]);
        for _ in 0..2 {
            // The label does not open actions or consume its drag/click surface.
            cx.update_window(window.into(), |_, window, cx| {
                window.simulate_mouse_move(point(px(180.0), px(10.0)), cx);
            })
            .unwrap();
            assert_eq!(observed.borrow().captions, [None; 2]);
            cx.update_window(window.into(), |_, window, cx| {
                window.simulate_mouse_move(point(px(310.0), px(10.0)), cx);
            })
            .unwrap();
            assert_eq!(observed.borrow().tabs, initial);
            assert!(observed.borrow().captions[0].is_none());
            assert_eq!(observed.borrow().action_builds[0], 0);
            assert!(observed.borrow().action_builds[1] > 0);
            let caption = observed.borrow().captions[1].expect("second tab caption");
            assert!(caption.top() >= initial[1].bottom());

            // Clicking the original hot spot must still select the second tab.
            cx.update_window(window.into(), |_, window, cx| {
                click(window, point(px(310.0), px(10.0)), cx);
            })
            .unwrap();
            assert_eq!(observed.borrow().active, 1);
            observed.borrow_mut().active = 0;

            cx.update_window(window.into(), |_, window, cx| {
                window.simulate_mouse_move(caption.center(), cx);
            })
            .unwrap();
            assert!(
                observed.borrow().captions[1].is_some(),
                "panel stays open when entered"
            );
            cx.update_window(window.into(), |_, window, cx| {
                click(window, caption.center(), cx)
            })
            .unwrap();
            assert_eq!(observed.borrow().active, 1);
            observed.borrow_mut().active = 0;
            let action = observed.borrow().actions[1].unwrap();
            cx.update_window(window.into(), |_, window, cx| {
                window.simulate_mouse_move(action.center(), cx);
                click(window, action.center(), cx);
            })
            .unwrap();
            assert_eq!(observed.borrow().action_target, Some(1));
            assert_eq!(
                observed.borrow().active,
                0,
                "action click must not activate a different tab"
            );
            cx.update_window(window.into(), |_, window, cx| {
                window.simulate_mouse_move(point(px(390.0), px(150.0)), cx);
            })
            .unwrap();
            assert_eq!(observed.borrow().captions, [None; 2]);
        }

        cx.update_window(window.into(), |_, window, cx| {
            let position = point(px(180.0), px(10.0));
            window.simulate_mouse_move(position, cx);
            window.dispatch_event(
                MouseDownEvent {
                    position,
                    button: MouseButton::Left,
                    click_count: 1,
                    ..Default::default()
                }
                .to_platform_input(),
                cx,
            );
            window.dispatch_event(
                MouseMoveEvent {
                    position: point(px(310.0), px(10.0)),
                    pressed_button: Some(MouseButton::Left),
                    ..Default::default()
                }
                .to_platform_input(),
                cx,
            );
        })
        .unwrap();
        assert_eq!(observed.borrow().dragged, Some(1));
        assert_eq!(
            observed.borrow().captions,
            [None; 2],
            "dragging across the trigger must not open actions"
        );
    }

    #[gpui::test]
    fn actions_fit_a_narrow_window_and_open_above_a_bottom_tab(cx: &mut TestAppContext) {
        for (width, top) in [(180.0, 0.0), (400.0, 120.0)] {
            let observed = Rc::new(RefCell::new(Observed::default()));
            let window = cx.open_window(size(px(width), px(160.0)), {
                let observed = observed.clone();
                move |_, _| Tabs(observed, px(top), ScrollHandle::new())
            });
            cx.update_window(window.into(), |_, window, cx| {
                window.simulate_mouse_move(point(px(10.0), px(80.0)), cx);
            })
            .unwrap();
            let tabs = observed.borrow().tabs;
            cx.update_window(window.into(), |_, window, cx| {
                window.simulate_mouse_move(point(px(150.0), px(top + 10.0)), cx);
            })
            .unwrap();
            assert_eq!(observed.borrow().tabs, tabs);
            let caption = observed.borrow().captions[0].expect("first tab caption");
            let action = observed.borrow().actions[0].expect("first tab actions");
            for bounds in [caption, action] {
                assert!(bounds.left() >= px(0.0));
                assert!(bounds.right() <= px(width));
                assert!(bounds.bottom() <= tabs[0].top() || bounds.top() >= tabs[0].bottom());
            }
            cx.update_window(window.into(), |_, window, cx| {
                window.simulate_mouse_move(action.center(), cx);
                click(window, action.center(), cx);
            })
            .unwrap();
            assert_eq!(observed.borrow().action_target, Some(0));
        }
    }

    #[gpui::test]
    fn scrolling_a_tab_out_of_view_closes_its_panel_without_a_mouse_move(cx: &mut TestAppContext) {
        let observed = Rc::new(RefCell::new(Observed::default()));
        let scroll = ScrollHandle::new();
        let window = cx.open_window(size(px(180.0), px(160.0)), {
            let observed = observed.clone();
            let scroll = scroll.clone();
            move |_, _| Tabs(observed, px(0.0), scroll)
        });
        cx.update_window(window.into(), |_, window, cx| {
            window.simulate_mouse_move(point(px(10.0), px(120.0)), cx);
            scroll.set_offset(point(px(-140.0), px(0.0)));
            window.refresh();
        })
        .unwrap();
        cx.update_window(window.into(), |_, window, cx| {
            window.simulate_mouse_move(point(px(170.0), px(10.0)), cx);
        })
        .unwrap();
        assert!(observed.borrow().captions[1].is_some());
        cx.update_window(window.into(), |_, window, _| {
            scroll.set_offset(point(px(0.0), px(0.0)));
            window.refresh();
        })
        .unwrap();
        cx.update_window(window.into(), |_, window, cx| {
            window.simulate_next_frame(cx);
        })
        .unwrap();
        assert_eq!(observed.borrow().captions, [None; 2]);
    }
}
