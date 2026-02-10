//! Context menu for terminal content (right-click on terminal area).

use crate::keybindings::Cancel;
use crate::theme::theme;
use crate::views::components::{
    context_menu_panel, menu_item, menu_item_conditional, menu_item_with_color, menu_separator,
};
use crate::workspace::state::SplitDirection;
use gpui::prelude::*;
use gpui::*;

/// Event emitted by TerminalContextMenu
pub enum TerminalContextMenuEvent {
    Close,
    Copy { terminal_id: String },
    Paste { terminal_id: String },
    Clear { terminal_id: String },
    SelectAll { terminal_id: String },
    Split { project_id: String, layout_path: Vec<usize>, direction: SplitDirection },
    CloseTerminal { project_id: String, terminal_id: String },
}

/// Context menu for terminal content
pub struct TerminalContextMenu {
    terminal_id: String,
    project_id: String,
    layout_path: Vec<usize>,
    position: Point<Pixels>,
    has_selection: bool,
    focus_handle: FocusHandle,
}

impl TerminalContextMenu {
    pub fn new(
        terminal_id: String,
        project_id: String,
        layout_path: Vec<usize>,
        position: Point<Pixels>,
        has_selection: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        Self {
            terminal_id,
            project_id,
            layout_path,
            position,
            has_selection,
            focus_handle,
        }
    }

    fn close(&self, cx: &mut Context<Self>) {
        cx.emit(TerminalContextMenuEvent::Close);
    }
}

impl EventEmitter<TerminalContextMenuEvent> for TerminalContextMenu {}

impl Render for TerminalContextMenu {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        // Focus on first render
        if !self.focus_handle.is_focused(window) {
            window.focus(&self.focus_handle, cx);
        }

        let position = self.position;
        let has_selection = self.has_selection;

        div()
            .track_focus(&self.focus_handle)
            .key_context("TerminalContextMenu")
            .on_action(cx.listener(|this, _: &Cancel, _window, cx| {
                this.close(cx);
            }))
            .absolute()
            .inset_0()
            .id("terminal-context-menu-backdrop")
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
                        context_menu_panel("terminal-context-menu", &t)
                            // Copy (conditional - requires selection)
                            .child(
                                menu_item_conditional("ctx-copy", "icons/copy.svg", "Copy", has_selection, &t)
                                    .when(has_selection, |el| {
                                        el.on_click(cx.listener(|this, _, _window, cx| {
                                            cx.emit(TerminalContextMenuEvent::Copy {
                                                terminal_id: this.terminal_id.clone(),
                                            });
                                        }))
                                    }),
                            )
                            // Paste
                            .child(
                                menu_item("ctx-paste", "icons/clipboard-paste.svg", "Paste", &t)
                                    .on_click(cx.listener(|this, _, _window, cx| {
                                        cx.emit(TerminalContextMenuEvent::Paste {
                                            terminal_id: this.terminal_id.clone(),
                                        });
                                    })),
                            )
                            .child(menu_separator(&t))
                            // Clear
                            .child(
                                menu_item("ctx-clear", "icons/eraser.svg", "Clear", &t)
                                    .on_click(cx.listener(|this, _, _window, cx| {
                                        cx.emit(TerminalContextMenuEvent::Clear {
                                            terminal_id: this.terminal_id.clone(),
                                        });
                                    })),
                            )
                            // Select All
                            .child(
                                menu_item("ctx-select-all", "icons/select-all.svg", "Select All", &t)
                                    .on_click(cx.listener(|this, _, _window, cx| {
                                        cx.emit(TerminalContextMenuEvent::SelectAll {
                                            terminal_id: this.terminal_id.clone(),
                                        });
                                    })),
                            )
                            .child(menu_separator(&t))
                            // Split Horizontal
                            .child(
                                menu_item("ctx-split-h", "icons/split-horizontal.svg", "Split Horizontal", &t)
                                    .on_click(cx.listener(|this, _, _window, cx| {
                                        cx.emit(TerminalContextMenuEvent::Split {
                                            project_id: this.project_id.clone(),
                                            layout_path: this.layout_path.clone(),
                                            direction: SplitDirection::Horizontal,
                                        });
                                    })),
                            )
                            // Split Vertical
                            .child(
                                menu_item("ctx-split-v", "icons/split-vertical.svg", "Split Vertical", &t)
                                    .on_click(cx.listener(|this, _, _window, cx| {
                                        cx.emit(TerminalContextMenuEvent::Split {
                                            project_id: this.project_id.clone(),
                                            layout_path: this.layout_path.clone(),
                                            direction: SplitDirection::Vertical,
                                        });
                                    })),
                            )
                            .child(menu_separator(&t))
                            // Close
                            .child(
                                menu_item_with_color("ctx-close", "icons/close.svg", "Close", t.error, t.error, &t)
                                    .on_click(cx.listener(|this, _, _window, cx| {
                                        cx.emit(TerminalContextMenuEvent::CloseTerminal {
                                            project_id: this.project_id.clone(),
                                            terminal_id: this.terminal_id.clone(),
                                        });
                                    })),
                            ),
                    ),
            ))
    }
}

impl_focusable!(TerminalContextMenu);
