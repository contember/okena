//! Shared header action buttons for terminal panes and tab groups.
//!
//! This module provides reusable button definitions to ensure consistency
//! between regular terminal mode and tab group mode.

use crate::theme::ThemeColors;
use gpui::*;
use gpui_component::tooltip::Tooltip;

/// Header actions that use row space only while their header is hovered.
/// Keep the element in the tree so GPUI can observe group-hover transitions;
/// the zero-width clip also prevents hidden controls from receiving pointer input.
pub fn hover_action_container(group: &'static str) -> Stateful<Div> {
    div()
        .id(format!("{group}-actions"))
        .flex()
        .items_center()
        .flex_shrink_0()
        .min_w_0()
        .max_w(px(0.0))
        .overflow_hidden()
        .opacity(0.0)
        .group_hover(group, |s| s.max_w_auto().opacity(1.0))
}

/// All available header actions for terminal panes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeaderAction {
    SplitVertical,
    SplitHorizontal,
    AddTab,
    More,
    TerminalActions,
    Close,
    ZoomPrev,
    ZoomNext,
    ExitZoom,
}

impl HeaderAction {
    /// Returns the icon path for this action.
    pub fn icon(&self) -> &'static str {
        match self {
            HeaderAction::SplitVertical => "icons/split-vertical.svg",
            HeaderAction::SplitHorizontal => "icons/split-horizontal.svg",
            HeaderAction::AddTab => "icons/tabs.svg",
            HeaderAction::More => "icons/more-horizontal.svg",
            HeaderAction::TerminalActions => "icons/terminal.svg",
            HeaderAction::Close => "icons/close.svg",
            HeaderAction::ZoomPrev => "icons/chevron-left.svg",
            HeaderAction::ZoomNext => "icons/chevron-right.svg",
            HeaderAction::ExitZoom => "icons/close.svg",
        }
    }

    /// Returns the default tooltip text for this action.
    pub fn tooltip(&self) -> &'static str {
        match self {
            HeaderAction::SplitVertical => "Split Vertical",
            HeaderAction::SplitHorizontal => "Split Horizontal",
            HeaderAction::AddTab => "Add Tab",
            HeaderAction::More => "More Terminal Actions",
            HeaderAction::TerminalActions => "Focused Terminal Actions",
            HeaderAction::Close => "Close",
            HeaderAction::ZoomPrev => "Previous Terminal",
            HeaderAction::ZoomNext => "Next Terminal",
            HeaderAction::ExitZoom => "Exit Zoom",
        }
    }

    /// Returns true if this is a close/exit action (for red hover styling).
    pub fn is_close(&self) -> bool {
        matches!(self, HeaderAction::Close | HeaderAction::ExitZoom)
    }

    /// Returns the element ID prefix for this action.
    pub fn id_prefix(&self) -> &'static str {
        match self {
            HeaderAction::SplitVertical => "split-vertical-btn",
            HeaderAction::SplitHorizontal => "split-horizontal-btn",
            HeaderAction::AddTab => "add-tab-btn",
            HeaderAction::More => "more-terminal-actions-btn",
            HeaderAction::TerminalActions => "focused-terminal-actions-btn",
            HeaderAction::Close => "close-btn",
            HeaderAction::ZoomPrev => "zoom-prev-btn",
            HeaderAction::ZoomNext => "zoom-next-btn",
            HeaderAction::ExitZoom => "zoom-exit-btn",
        }
    }
}

/// Button size configuration.
#[derive(Clone, Copy)]
pub struct ButtonSize {
    pub button: f32,
    pub icon: f32,
}

impl ButtonSize {
    /// Compact size for tab group headers (20px button, 12px icon).
    pub const COMPACT: Self = Self {
        button: 20.0,
        icon: 12.0,
    };
}

/// Renders a header button base element without click handler.
/// The caller should attach `.on_click()` to handle the action.
///
/// # Arguments
/// * `action` - The action this button represents
/// * `id_suffix` - Unique suffix for the element ID
/// * `size` - Button and icon size configuration
/// * `t` - Theme reference
/// * `tooltip_override` - Optional tooltip text override (e.g., "Close Tab" instead of "Close")
/// * `gpui_action` - Optional GPUI action for keybinding display in tooltips
pub fn header_button_base(
    action: HeaderAction,
    id_suffix: &str,
    size: ButtonSize,
    t: &ThemeColors,
    tooltip_override: Option<&'static str>,
    gpui_action: Option<Box<dyn Action>>,
) -> Stateful<Div> {
    let tooltip_text = tooltip_override.unwrap_or_else(|| action.tooltip());
    let bg_hover = t.bg_hover;

    let base = div()
        .id(format!("{}-{}", action.id_prefix(), id_suffix))
        .cursor_pointer()
        .w(px(size.button))
        .h(px(size.button))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.0))
        .on_mouse_down(MouseButton::Left, |_, _, cx| {
            cx.stop_propagation();
        })
        .child(
            svg()
                .path(action.icon())
                .size(px(size.icon))
                .text_color(rgb(t.text_secondary)),
        )
        .tooltip(move |_window, cx| {
            let mut tooltip = Tooltip::new(tooltip_text);
            if let Some(ref action) = gpui_action {
                tooltip = tooltip.action(action.as_ref(), None);
            }
            tooltip.build(_window, cx)
        });

    // Apply hover style - red for close, normal for others
    if action.is_close() {
        base.hover(|s| s.bg(rgba(0xf14c4c99)))
    } else {
        base.hover(move |s| s.bg(rgb(bg_hover)))
    }
}

#[cfg(test)]
mod hover_tests {
    use super::hover_action_container;
    use gpui::{
        AppContext as _, Bounds, Context, InteractiveElement, IntoElement, ParentElement, Pixels,
        Render, Styled, TestAppContext, Window, canvas, div, point, px, size,
    };
    use std::{cell::RefCell, rc::Rc};

    #[derive(Clone, Default)]
    struct Measurements {
        title: Rc<RefCell<Bounds<Pixels>>>,
        actions: Rc<RefCell<Bounds<Pixels>>>,
    }

    fn measure(bounds: Rc<RefCell<Bounds<Pixels>>>) -> impl IntoElement {
        canvas(
            move |value, _, _| *bounds.borrow_mut() = value,
            |_, _, _, _| {},
        )
        .absolute()
        .size_full()
    }

    struct Header {
        measurements: Measurements,
        action_width: f32,
    }

    impl Render for Header {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(
                div()
                    .group("header")
                    .flex()
                    .w_full()
                    .h(px(28.0))
                    .child(
                        div()
                            .relative()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .child(measure(self.measurements.title.clone())),
                    )
                    .child(
                        hover_action_container("header")
                            .relative()
                            .h_full()
                            .child(div().flex_shrink_0().w(px(self.action_width)).h_full())
                            .child(measure(self.measurements.actions.clone())),
                    ),
            )
        }
    }

    #[gpui::test]
    fn header_reclaims_hidden_action_space_on_hover_exit(cx: &mut TestAppContext) {
        // The wider action group also represents the optional shell selector.
        for (width, action_width) in [(400.0_f32, 116.0_f32), (240.0, 180.0), (100.0, 116.0)] {
            let measurements = Measurements::default();
            let window = cx.open_window(size(px(width), px(100.0)), {
                let measurements = measurements.clone();
                move |_, _| Header {
                    measurements,
                    action_width,
                }
            });
            cx.update_window(window.into(), |_, window, cx| {
                window.simulate_mouse_move(point(px(10.0), px(60.0)), cx);
                window.simulate_next_frame(cx);
            })
            .unwrap();
            assert_eq!(measurements.title.borrow().size.width, px(width));

            for _ in 0..2 {
                cx.update_window(window.into(), |_, window, cx| {
                    window.simulate_mouse_move(point(px(10.0), px(10.0)), cx);
                })
                .unwrap();
                assert_eq!(
                    measurements.title.borrow().size.width,
                    px((width - action_width).max(0.0))
                );
                assert_eq!(measurements.actions.borrow().size.width, px(action_width));
                assert_eq!(
                    measurements.actions.borrow().right(),
                    px(width.max(action_width))
                );

                // Moving over the revealed actions must keep them expanded.
                cx.update_window(window.into(), |_, window, cx| {
                    window.simulate_mouse_move(point(px(width - 10.0), px(10.0)), cx);
                })
                .unwrap();
                assert_eq!(
                    measurements.title.borrow().size.width,
                    px((width - action_width).max(0.0))
                );

                cx.update_window(window.into(), |_, window, cx| {
                    window.simulate_mouse_move(point(px(width - 10.0), px(60.0)), cx);
                })
                .unwrap();
                assert_eq!(measurements.title.borrow().size.width, px(width));
            }
        }
    }
}
