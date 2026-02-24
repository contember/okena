//! Context menu for remote connections in the sidebar.

use crate::keybindings::Cancel;
use crate::theme::theme;
use crate::views::components::{context_menu_panel, menu_item, menu_item_with_color, menu_separator};
use gpui::prelude::*;
use gpui::*;

/// Event emitted by RemoteContextMenu
pub enum RemoteContextMenuEvent {
    Close,
    Reconnect { connection_id: String },
    Pair { connection_id: String },
    RemoveConnection { connection_id: String },
}

/// Context menu for remote connections
pub struct RemoteContextMenu {
    connection_id: String,
    connection_name: String,
    is_pairing: bool,
    position: Point<Pixels>,
    focus_handle: FocusHandle,
}

impl RemoteContextMenu {
    pub fn new(
        connection_id: String,
        connection_name: String,
        is_pairing: bool,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        Self {
            connection_id,
            connection_name,
            is_pairing,
            position,
            focus_handle,
        }
    }

    fn close(&self, cx: &mut Context<Self>) {
        cx.emit(RemoteContextMenuEvent::Close);
    }

    fn reconnect(&self, cx: &mut Context<Self>) {
        cx.emit(RemoteContextMenuEvent::Reconnect {
            connection_id: self.connection_id.clone(),
        });
    }

    fn pair(&self, cx: &mut Context<Self>) {
        cx.emit(RemoteContextMenuEvent::Pair {
            connection_id: self.connection_id.clone(),
        });
    }

    fn remove(&self, cx: &mut Context<Self>) {
        cx.emit(RemoteContextMenuEvent::RemoveConnection {
            connection_id: self.connection_id.clone(),
        });
    }
}

impl EventEmitter<RemoteContextMenuEvent> for RemoteContextMenu {}

impl Render for RemoteContextMenu {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        // Focus on first render
        if !self.focus_handle.is_focused(window) {
            window.focus(&self.focus_handle, cx);
        }

        let position = self.position;

        div()
            .track_focus(&self.focus_handle)
            .key_context("RemoteContextMenu")
            .on_action(cx.listener(|this, _: &Cancel, _window, cx| {
                this.close(cx);
            }))
            .absolute()
            .inset_0()
            .id("remote-context-menu-backdrop")
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
                        context_menu_panel("remote-context-menu", &t)
                            .when(self.is_pairing, |el| {
                                el.child(
                                    menu_item("remote-ctx-pair", "icons/keyboard.svg", "Pair", &t)
                                        .on_click(cx.listener(|this, _, _window, cx| {
                                            this.pair(cx);
                                        })),
                                )
                            })
                            .child(
                                menu_item("remote-ctx-reconnect", "icons/terminal.svg", "Reconnect", &t)
                                    .on_click(cx.listener(|this, _, _window, cx| {
                                        this.reconnect(cx);
                                    })),
                            )
                            .child(menu_separator(&t))
                            .child(
                                menu_item_with_color(
                                    "remote-ctx-remove",
                                    "icons/trash.svg",
                                    format!("Remove \"{}\"", self.connection_name),
                                    t.error,
                                    t.error,
                                    &t,
                                )
                                .on_click(cx.listener(|this, _, _window, cx| {
                                    this.remove(cx);
                                })),
                            ),
                    ),
            ))
    }
}

impl_focusable!(RemoteContextMenu);
