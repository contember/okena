//! Tab context menu
//!
//! This module contains the right-click context menu for tabs:
//! - Close tab
//! - Close other tabs
//! - Close tabs to the right

use crate::theme::theme;
use crate::views::components::{menu_item, menu_item_disabled};
use crate::views::layout::layout_container::LayoutContainer;
use gpui::*;

impl LayoutContainer {
    pub(super) fn show_tab_context_menu(&mut self, tab_index: usize, position: Point<Pixels>, num_tabs: usize, cx: &mut Context<Self>) {
        self.tab_context_menu = Some((tab_index, position, num_tabs));
        cx.notify();
    }

    pub(super) fn hide_tab_context_menu(&mut self, cx: &mut Context<Self>) {
        self.tab_context_menu = None;
        cx.notify();
    }

    pub(super) fn render_tab_context_menu(&self, tab_index: usize, position: Point<Pixels>, num_tabs: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let workspace = self.workspace.clone();
        let project_id = self.project_id.clone();
        let layout_path = self.layout_path.clone();
        let has_tabs_to_right = tab_index < num_tabs.saturating_sub(1);
        let has_other_tabs = num_tabs > 1;

        // Get container bounds origin to calculate relative position
        let bounds = self.container_bounds_ref.borrow();
        let relative_x = position.x - bounds.origin.x;
        let relative_y = position.y - bounds.origin.y;
        drop(bounds);

        div()
            .id("tab-context-menu-backdrop")
            .absolute()
            .inset_0()
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _window, cx| {
                this.hide_tab_context_menu(cx);
            }))
            .on_mouse_down(MouseButton::Right, cx.listener(|this, _, _window, cx| {
                this.hide_tab_context_menu(cx);
            }))
            .child(
                div()
                    .id("tab-context-menu")
                    .absolute()
                    .left(relative_x)
                    .top(relative_y)
                    .bg(rgb(t.bg_secondary))
                    .border_1()
                    .border_color(rgb(t.border))
                    .rounded(px(4.0))
                    .shadow_lg()
                    .py(px(4.0))
                    .min_w(px(140.0))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .on_mouse_down(MouseButton::Right, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .on_scroll_wheel(|_, _, cx| {
                        cx.stop_propagation();
                    })
                    // Close tab
                    .child(
                        menu_item("tab-menu-close", "icons/close.svg", "Close", &t)
                            .on_click({
                                let workspace = workspace.clone();
                                let project_id = project_id.clone();
                                let layout_path = layout_path.clone();
                                cx.listener(move |this, _, _window, cx| {
                                    workspace.update(cx, |ws, cx| {
                                        ws.close_tab(&project_id, &layout_path, tab_index, cx);
                                    });
                                    this.hide_tab_context_menu(cx);
                                })
                            }),
                    )
                    // Close Others
                    .child(if has_other_tabs {
                        menu_item("tab-menu-close-others", "icons/close.svg", "Close Others", &t)
                            .on_click({
                                let workspace = workspace.clone();
                                let project_id = project_id.clone();
                                let layout_path = layout_path.clone();
                                cx.listener(move |this, _, _window, cx| {
                                    workspace.update(cx, |ws, cx| {
                                        ws.close_other_tabs(&project_id, &layout_path, tab_index, cx);
                                    });
                                    this.hide_tab_context_menu(cx);
                                })
                            })
                    } else {
                        menu_item_disabled("tab-menu-close-others", "icons/close.svg", "Close Others", &t)
                    })
                    // Close to Right
                    .child(if has_tabs_to_right {
                        menu_item("tab-menu-close-to-right", "icons/chevron-right.svg", "Close to Right", &t)
                            .on_click({
                                let workspace = workspace.clone();
                                let project_id = project_id.clone();
                                let layout_path = layout_path.clone();
                                cx.listener(move |this, _, _window, cx| {
                                    workspace.update(cx, |ws, cx| {
                                        ws.close_tabs_to_right(&project_id, &layout_path, tab_index, cx);
                                    });
                                    this.hide_tab_context_menu(cx);
                                })
                            })
                    } else {
                        menu_item_disabled("tab-menu-close-to-right", "icons/chevron-right.svg", "Close to Right", &t)
                    }),
            )
    }
}
