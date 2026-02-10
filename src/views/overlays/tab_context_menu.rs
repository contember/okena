//! Context menu for tab bar (right-click on a tab).

use crate::keybindings::Cancel;
use crate::theme::theme;
use crate::views::components::{context_menu_panel, menu_item, menu_item_disabled, menu_separator};
use gpui::prelude::*;
use gpui::*;

/// Event emitted by TabContextMenu
pub enum TabContextMenuEvent {
    Close,
    CloseTab { project_id: String, layout_path: Vec<usize>, tab_index: usize },
    CloseOtherTabs { project_id: String, layout_path: Vec<usize>, tab_index: usize },
    CloseTabsToRight { project_id: String, layout_path: Vec<usize>, tab_index: usize },
}

/// Context menu for tab bar
pub struct TabContextMenu {
    tab_index: usize,
    num_tabs: usize,
    project_id: String,
    layout_path: Vec<usize>,
    position: Point<Pixels>,
    focus_handle: FocusHandle,
}

impl TabContextMenu {
    pub fn new(
        tab_index: usize,
        num_tabs: usize,
        project_id: String,
        layout_path: Vec<usize>,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        Self {
            tab_index,
            num_tabs,
            project_id,
            layout_path,
            position,
            focus_handle,
        }
    }

    fn close(&self, cx: &mut Context<Self>) {
        cx.emit(TabContextMenuEvent::Close);
    }
}

impl EventEmitter<TabContextMenuEvent> for TabContextMenu {}

impl Render for TabContextMenu {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        // Focus on first render
        if !self.focus_handle.is_focused(window) {
            window.focus(&self.focus_handle, cx);
        }

        let position = self.position;
        let has_other_tabs = self.num_tabs > 1;
        let has_tabs_to_right = self.tab_index < self.num_tabs.saturating_sub(1);

        div()
            .track_focus(&self.focus_handle)
            .key_context("TabContextMenu")
            .on_action(cx.listener(|this, _: &Cancel, _window, cx| {
                this.close(cx);
            }))
            .absolute()
            .inset_0()
            .id("tab-context-menu-backdrop")
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _window, cx| {
                this.close(cx);
            }))
            .on_mouse_down(MouseButton::Right, cx.listener(|this, _, _window, cx| {
                this.close(cx);
            }))
            .child(deferred(
                anchored()
                    .position(position)
                    .snap_to_window()
                    .child(
                        context_menu_panel("tab-context-menu", &t)
                            // Close tab
                            .child(
                                menu_item("tab-ctx-close", "icons/close.svg", "Close", &t)
                                    .on_click(cx.listener(|this, _, _window, cx| {
                                        cx.emit(TabContextMenuEvent::CloseTab {
                                            project_id: this.project_id.clone(),
                                            layout_path: this.layout_path.clone(),
                                            tab_index: this.tab_index,
                                        });
                                    })),
                            )
                            .child(menu_separator(&t))
                            // Close Others
                            .child(if has_other_tabs {
                                menu_item("tab-ctx-close-others", "icons/close.svg", "Close Others", &t)
                                    .on_click(cx.listener(|this, _, _window, cx| {
                                        cx.emit(TabContextMenuEvent::CloseOtherTabs {
                                            project_id: this.project_id.clone(),
                                            layout_path: this.layout_path.clone(),
                                            tab_index: this.tab_index,
                                        });
                                    }))
                            } else {
                                menu_item_disabled("tab-ctx-close-others", "icons/close.svg", "Close Others", &t)
                            })
                            // Close to Right
                            .child(if has_tabs_to_right {
                                menu_item("tab-ctx-close-to-right", "icons/chevron-right.svg", "Close to Right", &t)
                                    .on_click(cx.listener(|this, _, _window, cx| {
                                        cx.emit(TabContextMenuEvent::CloseTabsToRight {
                                            project_id: this.project_id.clone(),
                                            layout_path: this.layout_path.clone(),
                                            tab_index: this.tab_index,
                                        });
                                    }))
                            } else {
                                menu_item_disabled("tab-ctx-close-to-right", "icons/chevron-right.svg", "Close to Right", &t)
                            }),
                    ),
            ))
    }
}

impl_focusable!(TabContextMenu);
