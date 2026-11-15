//! Two-stage header hover: reveal reserved controls, then expand missing actions.
use gpui::prelude::*;
use gpui::*;

#[derive(Default)]
struct BarState {
    header_hovered: bool,
    panel_hovered: bool,
    reserved_extra: Pixels,
    full_extra: Pixels,
    first_visible: usize,
    extra_scroll: ScrollHandle,
    was_expanded: bool,
}

// Keep a contiguous suffix so existing icons stay anchored beside More and Close.
fn fitting_suffix(widths: &[Pixels], available: Pixels) -> (usize, Pixels) {
    let mut first = widths.len();
    let mut used = px(0.0);
    for (i, width) in widths.iter().enumerate().rev() {
        let next = used
            + *width
            + if first < widths.len() {
                px(2.0)
            } else {
                px(0.0)
            };
        if next > available {
            break;
        }
        first = i;
        used = next;
    }
    (first, used)
}

#[derive(IntoElement)]
pub struct HeaderActionBar {
    id: ElementId,
    header: AnyElement,
    extra: Vec<AnyElement>,
    always: AnyElement,
    header_width: Box<dyn Fn() -> Pixels>,
    min_header_width: Pixels,
    background: Option<Hsla>,
}

impl HeaderActionBar {
    pub fn new(
        id: impl Into<ElementId>,
        header: impl IntoElement,
        extra: Vec<AnyElement>,
        always: impl IntoElement,
        header_width: impl Fn() -> Pixels + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            header: header.into_any_element(),
            extra,
            always: always.into_any_element(),
            header_width: Box::new(header_width),
            min_header_width: px(28.0),
            background: None,
        }
    }

    /// Paint the original header surface beneath the transparent controls.
    pub fn background(mut self, color: impl Into<Hsla>) -> Self {
        self.background = Some(color.into());
        self
    }

    pub fn min_header_width(mut self, width: Pixels) -> Self {
        self.min_header_width = width;
        self
    }
}

impl RenderOnce for HeaderActionBar {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = window.use_keyed_state(self.id.clone(), cx, |_, _| BarState::default());
        let visible = state.read(cx).header_hovered && !cx.has_active_drag();
        let expanded = visible && state.read(cx).panel_hovered;
        let extra_width = if expanded {
            state.read(cx).full_extra
        } else {
            state.read(cx).reserved_extra
        };
        let first_visible = state.read(cx).first_visible;
        let extra_count = self.extra.len();
        let extra_measure = state.read(cx).extra_scroll.clone();
        // In compact mode show the suffix. On entry to expanded mode keep its
        // right edge fixed; further horizontal scrolling is retained until exit.
        if !expanded || !state.read(cx).was_expanded {
            extra_measure.set_offset(point(-state.read(cx).full_extra, px(0.0)));
        }
        state.update(cx, |state, _| state.was_expanded = expanded);
        let always_measure = ScrollHandle::new();
        let actions = div()
            .id("terminal-header-actions")
            .debug_selector(|| "terminal-header-actions".into())
            .h_full()
            .flex()
            .items_center()
            .gap(px(if extra_width > px(0.0) { 4.0 } else { 0.0 }))
            .px(px(2.0))
            .min_w(px(46.0))
            .flex_shrink_1()
            .max_w_full()
            // The wrapper stays transparent and interactive even when its
            // children are hidden. Hidden controls have no click/tooltip handlers.
            .on_hover({
                let state = state.clone();
                move |hovered, _, cx| {
                    if !hovered {
                        state.update(cx, |state, cx| {
                            if state.panel_hovered {
                                state.panel_hovered = false;
                                cx.notify();
                            }
                        });
                    }
                }
            })
            .on_mouse_move({
                let state = state.clone();
                move |event, _, cx| {
                    // Preserve the pressed control until release; a tiny pointer
                    // movement is not a tab drag and must not cancel its click.
                    if event.pressed_button.is_some() || cx.has_active_drag() {
                        return;
                    }
                    let next = true;
                    state.update(cx, |state, cx| {
                        if state.panel_hovered != next {
                            state.panel_hovered = next;
                            cx.notify();
                        }
                    });
                }
            })
            .child(
                div()
                    .id("terminal-header-extra")
                    .debug_selector(|| "terminal-header-extra".into())
                    .flex()
                    .items_center()
                    .gap(px(2.0))
                    .w(extra_width)
                    .min_w_0()
                    .flex_shrink_1()
                    .overflow_x_scroll()
                    .track_scroll(&extra_measure)
                    .children(self.extra.into_iter().enumerate().map(|(i, item)| {
                        div()
                            .debug_selector(move || format!("terminal-header-item-{i}"))
                            .flex_shrink_0()
                            .when(!visible || (!expanded && i < first_visible), |el| {
                                el.invisible()
                            })
                            .child(item)
                    })),
            )
            .child(
                div()
                    .id("terminal-header-always")
                    .flex()
                    .flex_shrink_0()
                    .track_scroll(&always_measure)
                    .child(
                        div()
                            .flex()
                            .flex_shrink_0()
                            .when(!visible, |el| el.invisible())
                            .child(self.always),
                    ),
            );
        div()
            .id(self.id)
            .relative()
            .w_full()
            .h(px(28.0))
            .flex()
            .items_center()
            .flex_shrink_0()
            .debug_selector(|| "terminal-action-header".into())
            .when_some(self.background, |header, color| header.bg(color))
            .on_hover({
                let state = state.clone();
                move |hovered, _, cx| {
                    if !hovered {
                        state.update(cx, |state, cx| {
                            if state.header_hovered || state.panel_hovered {
                                state.header_hovered = false;
                                state.panel_hovered = false;
                                cx.notify();
                            }
                        });
                    }
                }
            })
            .on_mouse_move({
                let state = state.clone();
                move |event, _, cx| {
                    // Preserve the pressed control until release; a tiny pointer
                    // movement is not a tab drag and must not cancel its click.
                    if event.pressed_button.is_some() || cx.has_active_drag() {
                        return;
                    }
                    let next = true;
                    state.update(cx, |state, cx| {
                        if state.header_hovered != next {
                            state.header_hovered = next;
                            cx.notify();
                        }
                    });
                }
            })
            .child(
                div()
                    .flex_1()
                    .min_w(self.min_header_width)
                    .overflow_hidden()
                    .child(self.header),
            )
            .child(actions)
            .child(
                canvas(
                    move |bounds, window, cx| {
                        // Invisible children still participate in layout, so measuring
                        // does not require duplicate controls or invisible click targets.
                        let widths: Vec<_> = (0..extra_count)
                            .map(|i| {
                                extra_measure
                                    .bounds_for_item(i)
                                    .map(|b| b.size.width)
                                    .unwrap_or_default()
                            })
                            .collect();
                        let full = widths.iter().copied().sum::<Pixels>()
                            + px(2.0 * extra_count.saturating_sub(1) as f32);
                        let always = always_measure
                            .bounds_for_item(0)
                            .map(|b| b.size.width)
                            .unwrap_or_default();
                        // Any nonempty suffix needs both 4px padding and the
                        // 4px separator. No separator is reserved for an empty suffix.
                        let available =
                            bounds.size.width - (self.header_width)() - always - px(8.0);
                        let (first, reserved) = fitting_suffix(&widths, available);
                        state.update(cx, |state, _cx| {
                            let mut changed = state.full_extra != full;
                            state.full_extra = full;
                            // Expanded actions temporarily squeeze the tabs. Recompute
                            // the baseline only in its own layout, never from squeezed tabs.
                            if !expanded
                                && (state.first_visible != first
                                    || state.reserved_extra != reserved)
                            {
                                state.first_visible = first;
                                state.reserved_extra = reserved;
                                changed = true;
                            }
                            if changed {
                                // Measurement happens during prepaint: schedule one
                                // follow-up frame rather than notifying during this frame.
                                window.request_animation_frame();
                            }
                        });
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::fitting_suffix;
    use gpui::px;

    #[test]
    fn reserved_actions_are_whole_and_right_aligned() {
        let widths = [px(75.0), px(20.0), px(20.0), px(20.0)];
        for (space, first, width) in [
            (-1.0, 4, 0.0),
            (19.0, 4, 0.0),
            (20.0, 3, 20.0),
            (41.0, 3, 20.0),
            (42.0, 2, 42.0),
            (64.0, 1, 64.0),
            (140.0, 1, 64.0),
            (141.0, 0, 141.0),
        ] {
            assert_eq!(fitting_suffix(&widths, px(space)), (first, px(width)));
        }
    }
}
