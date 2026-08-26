//! Adaptive terminal menu shared by right-click and header triggers.

use crate::actions::Cancel;
use gpui::prelude::*;
use gpui::*;
use okena_terminal::shell_config::ShellType;
use okena_ui::menu::{
    context_menu_panel, menu_item, menu_item_conditional, menu_item_disabled, menu_item_with_color,
    menu_separator,
};
use okena_ui::submenu::{Submenu, SubmenuHost, SubmenuItem, SubmenuState};
use okena_ui::theme::theme;
use okena_workspace::requests::TerminalMenuInvocation;
use okena_workspace::state::SplitDirection;

/// Event emitted by the adaptive terminal menu.
pub enum TerminalMenuEvent {
    Close,
    Copy {
        terminal_id: String,
    },
    /// Annotate the selection and send it back into this terminal.
    AnnotateSelection {
        terminal_id: String,
        position: Point<Pixels>,
    },
    Paste {
        terminal_id: String,
    },
    Clear {
        terminal_id: String,
    },
    SelectAll {
        terminal_id: String,
    },
    /// Flip the pane's unread (bell) mark.
    ToggleUnread {
        terminal_id: String,
    },
    RenameTerminal {
        project_id: String,
        terminal_id: String,
        current_name: String,
    },
    ChangeShell {
        project_id: String,
        terminal_id: String,
        current_shell: ShellType,
    },
    AddTab {
        project_id: String,
        layout_path: Vec<usize>,
    },
    Split {
        project_id: String,
        layout_path: Vec<usize>,
        direction: SplitDirection,
    },
    ZoomTerminal {
        project_id: String,
        terminal_id: String,
    },
    MinimizeTerminal {
        project_id: String,
        terminal_id: String,
    },
    ExportBuffer {
        project_id: String,
        terminal_id: String,
    },
    Detach {
        project_id: String,
        layout_path: Vec<usize>,
    },
    CloseTerminal {
        project_id: String,
        terminal_id: String,
    },
    OpenLink {
        url: String,
    },
    CopyLink {
        url: String,
    },
}

/// Terminal menu whose sections adapt to its invocation source.
pub struct TerminalMenu {
    terminal_id: String,
    project_id: String,
    layout_path: Vec<usize>,
    position: Point<Pixels>,
    current_name: String,
    current_shell: ShellType,
    can_export_buffer: bool,
    /// Set by the host, which owns the terminals and so the bell state.
    has_bell: bool,
    invocation: TerminalMenuInvocation,
    focus_handle: FocusHandle,
    submenus: SubmenuState,
}

impl TerminalMenu {
    // Menu setup: params are target and invocation state.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        terminal_id: String,
        project_id: String,
        layout_path: Vec<usize>,
        position: Point<Pixels>,
        current_name: String,
        current_shell: ShellType,
        can_export_buffer: bool,
        has_bell: bool,
        invocation: TerminalMenuInvocation,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        Self {
            terminal_id,
            project_id,
            layout_path,
            position,
            current_name,
            current_shell,
            can_export_buffer,
            has_bell,
            invocation,
            focus_handle,
            submenus: SubmenuState::default(),
        }
    }

    fn close(&self, cx: &mut Context<Self>) {
        cx.emit(TerminalMenuEvent::Close);
    }
}

impl EventEmitter<TerminalMenuEvent> for TerminalMenu {}

impl SubmenuHost for TerminalMenu {
    fn submenu_state(&mut self) -> &mut SubmenuState {
        &mut self.submenus
    }
}

/// One entry of the Buffer group.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BufferAction {
    SelectAll,
    Clear,
    Export,
}

impl BufferAction {
    fn id(self) -> &'static str {
        match self {
            Self::SelectAll => "terminal-menu-select-all",
            Self::Clear => "terminal-menu-clear",
            Self::Export => "terminal-menu-export-buffer",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Self::SelectAll => "icons/select-all.svg",
            Self::Clear => "icons/eraser.svg",
            Self::Export => "icons/copy.svg",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::SelectAll => "Select All",
            Self::Clear => "Clear",
            Self::Export => "Export to File",
        }
    }
}

/// Buffer-group entries for one invocation, in flyout order.
///
/// The group shrinks with the invocation, and the submenu collapse rule takes it from there:
/// a flyout in Content mode, a plain Export row in Header mode, nothing at all with neither.
fn buffer_actions(include_content_actions: bool, can_export_buffer: bool) -> Vec<BufferAction> {
    let mut actions = Vec::new();
    if include_content_actions {
        actions.push(BufferAction::SelectAll);
        actions.push(BufferAction::Clear);
    }
    if can_export_buffer {
        actions.push(BufferAction::Export);
    }
    actions
}

impl Render for TerminalMenu {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        // Never pull focus off an open flyout — it lives in this menu's own subtree.
        if !self.focus_handle.is_focused(window) && !self.submenus.holds_focus(window) {
            window.focus(&self.focus_handle, cx);
        }

        let position = self.position;
        let include_content_actions = self.invocation.includes_content_actions();
        let include_primary_actions = self.invocation.includes_primary_actions();
        let has_bell = self.has_bell;
        let (has_selection, link_url) = match &self.invocation {
            TerminalMenuInvocation::Content {
                has_selection,
                link_url,
            } => (*has_selection, link_url.clone()),
            TerminalMenuInvocation::Header { .. } => (false, None),
        };

        let buffer_items: Vec<SubmenuItem> =
            buffer_actions(include_content_actions, self.can_export_buffer)
                .into_iter()
                .map(|action| {
                    SubmenuItem::new(
                        action.id(),
                        action.icon(),
                        action.label(),
                        cx.listener(move |this: &mut Self, _, _window, cx| match action {
                            BufferAction::SelectAll => cx.emit(TerminalMenuEvent::SelectAll {
                                terminal_id: this.terminal_id.clone(),
                            }),
                            BufferAction::Clear => cx.emit(TerminalMenuEvent::Clear {
                                terminal_id: this.terminal_id.clone(),
                            }),
                            BufferAction::Export => cx.emit(TerminalMenuEvent::ExportBuffer {
                                project_id: this.project_id.clone(),
                                terminal_id: this.terminal_id.clone(),
                            }),
                        }),
                    )
                })
                .collect();
        let buffer_group = Submenu::new("terminal-menu-buffer", "icons/file.svg", "Buffer")
            .items(buffer_items)
            .render(&mut self.submenus, &t, window, cx);

        div()
            .track_focus(&self.focus_handle)
            .key_context("TerminalMenu")
            .on_action(cx.listener(|this, _: &Cancel, _window, cx| {
                // Escape peels off the open flyout before it closes the menu itself.
                if this.submenus.close() {
                    cx.notify();
                } else {
                    this.close(cx);
                }
            }))
            .absolute()
            .inset_0()
            .id("terminal-menu-backdrop")
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _window, cx| {
                    this.close(cx);
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _, _window, cx| {
                    this.close(cx);
                }),
            )
            .child(deferred(
                anchored().position(position).snap_to_window().child(
                    context_menu_panel("terminal-menu", &t)
                        .child(menu_item_disabled(
                            "terminal-menu-target",
                            "icons/terminal.svg",
                            self.current_name.clone(),
                            &t,
                        ))
                        .child(menu_separator(&t))
                        .when(include_content_actions, |menu| {
                            menu.when_some(link_url, |menu, url| {
                                let url_for_copy = url.clone();
                                menu.child(
                                    menu_item(
                                        "terminal-menu-open-link",
                                        "icons/external-link.svg",
                                        "Open in Browser",
                                        &t,
                                    )
                                    .on_click(cx.listener(
                                        move |_this, _, _window, cx| {
                                            cx.emit(TerminalMenuEvent::OpenLink {
                                                url: url.clone(),
                                            });
                                        },
                                    )),
                                )
                                .child(
                                    menu_item(
                                        "terminal-menu-copy-link",
                                        "icons/link.svg",
                                        "Copy Link",
                                        &t,
                                    )
                                    .on_click(cx.listener(
                                        move |_this, _, _window, cx| {
                                            cx.emit(TerminalMenuEvent::CopyLink {
                                                url: url_for_copy.clone(),
                                            });
                                        },
                                    )),
                                )
                                .child(menu_separator(&t))
                            })
                            .child(
                                menu_item_conditional(
                                    "terminal-menu-copy",
                                    "icons/copy.svg",
                                    "Copy",
                                    has_selection,
                                    &t,
                                )
                                .when(has_selection, |item| {
                                    item.on_click(cx.listener(|this, _, _window, cx| {
                                        cx.emit(TerminalMenuEvent::Copy {
                                            terminal_id: this.terminal_id.clone(),
                                        });
                                    }))
                                }),
                            )
                            .child(
                                menu_item_conditional(
                                    "terminal-menu-annotate",
                                    "icons/terminal.svg",
                                    "Send to Terminal…",
                                    has_selection,
                                    &t,
                                )
                                .when(has_selection, |item| {
                                    item.on_click(cx.listener(|this, _, _window, cx| {
                                        cx.emit(TerminalMenuEvent::AnnotateSelection {
                                            terminal_id: this.terminal_id.clone(),
                                            position: this.position,
                                        });
                                    }))
                                }),
                            )
                            .child(
                                menu_item(
                                    "terminal-menu-paste",
                                    "icons/clipboard-paste.svg",
                                    "Paste",
                                    &t,
                                )
                                .on_click(cx.listener(
                                    |this, _, _window, cx| {
                                        cx.emit(TerminalMenuEvent::Paste {
                                            terminal_id: this.terminal_id.clone(),
                                        });
                                    },
                                )),
                            )
                            .child(menu_separator(&t))
                            .child(
                                menu_item(
                                    "terminal-menu-toggle-unread",
                                    "icons/bell.svg",
                                    if has_bell {
                                        "Mark as Read"
                                    } else {
                                        "Mark as Unread"
                                    },
                                    &t,
                                )
                                .on_click(cx.listener(
                                    |this, _, _window, cx| {
                                        cx.emit(TerminalMenuEvent::ToggleUnread {
                                            terminal_id: this.terminal_id.clone(),
                                        });
                                    },
                                )),
                            )
                            .child(menu_separator(&t))
                        })
                        .child(
                            menu_item(
                                "terminal-menu-rename",
                                "icons/edit.svg",
                                "Rename Terminal…",
                                &t,
                            )
                            .on_click(cx.listener(
                                |this, _, _window, cx| {
                                    cx.emit(TerminalMenuEvent::RenameTerminal {
                                        project_id: this.project_id.clone(),
                                        terminal_id: this.terminal_id.clone(),
                                        current_name: this.current_name.clone(),
                                    });
                                },
                            )),
                        )
                        .child(
                            menu_item(
                                "terminal-menu-change-shell",
                                "icons/terminal.svg",
                                "Change Shell…",
                                &t,
                            )
                            .on_click(cx.listener(
                                |this, _, _window, cx| {
                                    cx.emit(TerminalMenuEvent::ChangeShell {
                                        project_id: this.project_id.clone(),
                                        terminal_id: this.terminal_id.clone(),
                                        current_shell: this.current_shell.clone(),
                                    });
                                },
                            )),
                        )
                        .child(menu_separator(&t))
                        .when(include_primary_actions, |menu| {
                            menu.child(
                                menu_item("terminal-menu-add-tab", "icons/tabs.svg", "Add Tab", &t)
                                    .on_click(cx.listener(|this, _, _window, cx| {
                                        cx.emit(TerminalMenuEvent::AddTab {
                                            project_id: this.project_id.clone(),
                                            layout_path: this.layout_path.clone(),
                                        });
                                    })),
                            )
                            .child(
                                menu_item(
                                    "terminal-menu-split-horizontal",
                                    "icons/split-horizontal.svg",
                                    "Split Horizontal",
                                    &t,
                                )
                                .on_click(cx.listener(
                                    |this, _, _window, cx| {
                                        cx.emit(TerminalMenuEvent::Split {
                                            project_id: this.project_id.clone(),
                                            layout_path: this.layout_path.clone(),
                                            direction: SplitDirection::Horizontal,
                                        });
                                    },
                                )),
                            )
                            .child(
                                menu_item(
                                    "terminal-menu-split-vertical",
                                    "icons/split-vertical.svg",
                                    "Split Vertical",
                                    &t,
                                )
                                .on_click(cx.listener(
                                    |this, _, _window, cx| {
                                        cx.emit(TerminalMenuEvent::Split {
                                            project_id: this.project_id.clone(),
                                            layout_path: this.layout_path.clone(),
                                            direction: SplitDirection::Vertical,
                                        });
                                    },
                                )),
                            )
                            .child(menu_separator(&t))
                        })
                        .child(
                            menu_item(
                                "terminal-menu-minimize",
                                "icons/minimize.svg",
                                "Minimize Terminal",
                                &t,
                            )
                            .on_click(cx.listener(
                                |this, _, _window, cx| {
                                    cx.emit(TerminalMenuEvent::MinimizeTerminal {
                                        project_id: this.project_id.clone(),
                                        terminal_id: this.terminal_id.clone(),
                                    });
                                },
                            )),
                        )
                        .child(buffer_group)
                        .child(
                            menu_item(
                                "terminal-menu-zoom",
                                "icons/fullscreen.svg",
                                "Zoom Terminal",
                                &t,
                            )
                            .on_click(cx.listener(
                                |this, _, _window, cx| {
                                    cx.emit(TerminalMenuEvent::ZoomTerminal {
                                        project_id: this.project_id.clone(),
                                        terminal_id: this.terminal_id.clone(),
                                    });
                                },
                            )),
                        )
                        .child(
                            menu_item(
                                "terminal-menu-detach",
                                "icons/detach.svg",
                                "Detach to Window",
                                &t,
                            )
                            .on_click(cx.listener(
                                |this, _, _window, cx| {
                                    cx.emit(TerminalMenuEvent::Detach {
                                        project_id: this.project_id.clone(),
                                        layout_path: this.layout_path.clone(),
                                    });
                                },
                            )),
                        )
                        .when(include_primary_actions, |menu| {
                            menu.child(menu_separator(&t)).child(
                                menu_item_with_color(
                                    "terminal-menu-close",
                                    "icons/close.svg",
                                    "Close Terminal",
                                    t.error,
                                    t.error,
                                    &t,
                                )
                                .on_click(cx.listener(
                                    |this, _, _window, cx| {
                                        cx.emit(TerminalMenuEvent::CloseTerminal {
                                            project_id: this.project_id.clone(),
                                            terminal_id: this.terminal_id.clone(),
                                        });
                                    },
                                )),
                            )
                        }),
                ),
            ))
    }
}

impl gpui::Focusable for TerminalMenu {
    fn focus_handle(&self, _cx: &gpui::App) -> gpui::FocusHandle {
        self.focus_handle.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::{BufferAction, buffer_actions};
    use okena_ui::submenu::{SubmenuLayout, submenu_layout};
    use okena_workspace::requests::TerminalMenuInvocation;

    #[test]
    fn content_mode_gives_the_buffer_group_a_flyout() {
        assert_eq!(
            submenu_layout(buffer_actions(true, true).len()),
            SubmenuLayout::Flyout
        );
        assert_eq!(
            submenu_layout(buffer_actions(true, false).len()),
            SubmenuLayout::Flyout
        );
    }

    #[test]
    fn header_mode_collapses_the_buffer_group_to_a_plain_export_row() {
        let actions = buffer_actions(false, true);

        assert_eq!(actions, vec![BufferAction::Export]);
        assert_eq!(submenu_layout(actions.len()), SubmenuLayout::Collapsed);
    }

    #[test]
    fn header_mode_without_export_drops_the_buffer_group() {
        assert_eq!(
            submenu_layout(buffer_actions(false, false).len()),
            SubmenuLayout::Empty
        );
    }

    #[test]
    fn content_invocation_includes_context_and_primary_actions() {
        let invocation = TerminalMenuInvocation::Content {
            has_selection: false,
            link_url: None,
        };

        assert!(invocation.includes_content_actions());
        assert!(invocation.includes_primary_actions());
    }

    #[test]
    fn project_header_includes_primary_but_not_content_actions() {
        let invocation = TerminalMenuInvocation::Header {
            include_primary_actions: true,
        };

        assert!(!invocation.includes_content_actions());
        assert!(invocation.includes_primary_actions());
    }

    #[test]
    fn tab_overflow_includes_only_secondary_management_actions() {
        let invocation = TerminalMenuInvocation::Header {
            include_primary_actions: false,
        };

        assert!(!invocation.includes_content_actions());
        assert!(!invocation.includes_primary_actions());
    }
}
