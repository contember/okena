//! Single-line text that shows its full content in a tooltip, but only while
//! the layout has cut it off with an ellipsis.

use gpui::{Div, ElementId, InteractiveText, Pixels, SharedString, Styled, StyledText, div};
use gpui_component::tooltip::Tooltip;

/// Text element whose tooltip shows `text` in full when it does not fit.
///
/// The ellipsis itself comes from the parent: give it `.overflow_hidden()`,
/// `.whitespace_nowrap()` and `.text_ellipsis()` (or just `.truncate()`).
/// Without nowrap a long text wraps and the clipped second line gets neither
/// the "…" nor this tooltip, because wrapping leaves the text intact. Whether
/// the text was cut is read from the laid-out line when the mouse rests on it,
/// so a title that fits gets no tooltip and a resize needs no bookkeeping.
///
/// `note` is appended as a second line of the tooltip. Use it when the parent
/// has its own tooltip: the hovered text's tooltip wins, so it must carry the
/// parent's message too.
pub fn truncated_text(
    id: impl Into<ElementId>,
    text: impl Into<SharedString>,
    note: Option<SharedString>,
) -> InteractiveText {
    let text = text.into();
    let styled = StyledText::new(text.clone());
    let layout = styled.layout().clone();
    InteractiveText::new(id, styled).tooltip(move |_ix, window, cx| {
        if layout.text() == text.as_ref() {
            return None;
        }
        let content = match &note {
            Some(note) => format!("{text}\n{note}"),
            None => text.to_string(),
        };
        Some(Tooltip::new(content).build(window, cx))
    })
}

/// A content-sized label that can shrink with its row. The font-relative cap
/// keeps pathological names bounded without clipping ordinary names at a fixed
/// pixel width. Icons and controls belong outside this container.
pub fn adaptive_label_container(font_size: Pixels) -> Div {
    div()
        .min_w_0()
        .max_w(font_size * 40.0)
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::AppContext as _;
    use gpui::{
        Bounds, Context, InteractiveElement, IntoElement, ParentElement, Render, ScrollHandle,
        StatefulInteractiveElement, TestAppContext, Window, canvas, px, relative, size,
    };
    use std::{cell::RefCell, rc::Rc};

    #[derive(Default, Clone)]
    struct Measurements {
        label: Rc<RefCell<Bounds<Pixels>>>,
        controls: Rc<RefCell<Bounds<Pixels>>>,
        tab: Rc<RefCell<Bounds<Pixels>>>,
        scroll: ScrollHandle,
    }

    fn measure(bounds: Rc<RefCell<Bounds<Pixels>>>) -> impl IntoElement {
        canvas(
            move |value, _, _| *bounds.borrow_mut() = value,
            |_, _, _, _| {},
        )
        .absolute()
        .size_full()
    }

    struct LabelRow {
        text: SharedString,
        font_size: Pixels,
        tabs: bool,
        measurements: Measurements,
    }

    impl Render for LabelRow {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let label = adaptive_label_container(self.font_size)
                .relative()
                .text_size(self.font_size)
                .child(truncated_text("label", self.text.clone(), None))
                .child(measure(self.measurements.label.clone()));
            let content = if self.tabs {
                // The scrolling tab strip reserves a separate area for actions.
                div()
                    .id("tabs")
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .overflow_x_scroll()
                    .track_scroll(&self.measurements.scroll)
                    .child(
                        div()
                            .relative()
                            .flex_shrink_0()
                            .max_w(relative(1.0))
                            .overflow_hidden()
                            .px(px(8.0))
                            .child(div().flex().min_w_0().child(label))
                            .child(measure(self.measurements.tab.clone())),
                    )
                    .child(div().flex_shrink_0().w(px(100.0)).h(px(20.0)))
                    .into_any_element()
            } else {
                label.into_any_element()
            };
            div().flex().w_full().child(content).child(
                div()
                    .relative()
                    .flex_shrink_0()
                    .w(px(80.0))
                    .h(px(20.0))
                    .child(measure(self.measurements.controls.clone())),
            )
        }
    }

    fn layout(
        cx: &mut TestAppContext,
        text: impl Into<SharedString>,
        width: f32,
        font_size: f32,
        tabs: bool,
    ) -> Measurements {
        let measurements = Measurements::default();
        let result = measurements.clone();
        let text = text.into();
        let window = cx.open_window(size(px(width), px(100.0)), move |_, _| LabelRow {
            text,
            font_size: px(font_size),
            tabs,
            measurements,
        });
        cx.update_window(window.into(), |_, window, cx| {
            window.simulate_next_frame(cx)
        })
        .unwrap();
        result
    }

    #[gpui::test]
    fn adaptive_label_uses_content_width_and_available_space(cx: &mut TestAppContext) {
        let short = layout(cx, "main", 800.0, 12.0, false);
        let long = layout(
            cx,
            "feature/a-descriptive-name-for-the-current-project",
            800.0,
            12.0,
            false,
        );
        assert!(short.label.borrow().size.width < px(100.0));
        assert!(long.label.borrow().size.width > px(200.0));
    }

    #[gpui::test]
    fn adaptive_label_yields_to_controls_in_a_narrow_row(cx: &mut TestAppContext) {
        let bounds = layout(cx, "feature/".repeat(40), 180.0, 12.0, false);
        assert!(bounds.label.borrow().size.width <= px(100.0));
        assert_eq!(bounds.controls.borrow().size.width, px(80.0));
        assert!(bounds.controls.borrow().right() <= px(180.0));
    }

    #[gpui::test]
    fn adaptive_label_cap_scales_with_the_font(cx: &mut TestAppContext) {
        let small = layout(cx, "x".repeat(500), 2000.0, 10.0, false);
        let large = layout(cx, "x".repeat(500), 2000.0, 20.0, false);
        assert_eq!(small.label.borrow().size.width, px(400.0));
        assert_eq!(large.label.borrow().size.width, px(800.0));
    }

    #[gpui::test]
    fn adaptive_label_keeps_unicode_on_one_line(cx: &mut TestAppContext) {
        let short = layout(cx, "Příliš 日本語", 800.0, 12.0, false);
        let long = layout(cx, "Příliš 日本語 ".repeat(50), 180.0, 12.0, false);
        assert_eq!(
            short.label.borrow().size.height,
            long.label.borrow().size.height
        );
        assert!(long.label.borrow().size.width <= px(100.0));
    }

    #[gpui::test]
    fn adaptive_label_tab_fits_the_scroll_viewport(cx: &mut TestAppContext) {
        let wide = layout(cx, "x".repeat(500), 800.0, 12.0, true);
        assert!(wide.tab.borrow().size.width > px(200.0));
        let narrow = layout(cx, "x".repeat(500), 240.0, 12.0, true);
        assert!(narrow.tab.borrow().size.width <= px(160.0));
        assert!(narrow.label.borrow().size.width <= px(144.0));
        assert_eq!(narrow.controls.borrow().size.width, px(80.0));
        assert_eq!(narrow.controls.borrow().right(), px(240.0));
        assert!(narrow.scroll.max_offset().x > px(0.0));
    }
}
