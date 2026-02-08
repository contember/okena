//! Render implementation for TerminalPane.
//!
//! Contains the GPUI Render trait implementation which composes
//! the header, content, search bar, and zoom header into the final view.

use crate::keybindings::{
    AddTab, CloseSearch, CloseTerminal, Copy, FocusDown, FocusLeft, FocusNextTerminal,
    FocusPrevTerminal, FocusRight, FocusUp, FullscreenNextTerminal, FullscreenPrevTerminal,
    MinimizeTerminal, Paste, ResetZoom, Search, SearchNext, SearchPrev, SendBacktab, SendEscape,
    SendTab, SplitHorizontal, SplitVertical, ToggleFullscreen, ZoomIn, ZoomOut,
};
use crate::settings::settings;
use crate::theme::theme;
use crate::views::layout::navigation::NavigationDirection;
use crate::workspace::state::SplitDirection;
use gpui::prelude::FluentBuilder;
use gpui::*;

use super::TerminalPane;

impl Render for TerminalPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        let focus_handle = self.focus_handle.clone();
        let id_suffix = self.id_suffix();

        // Check focus state
        let is_modal = {
            let ws = self.workspace.read(cx);
            let is_modal = ws.focus_manager.is_modal();
            let header_renaming = self.header.read(cx).is_renaming();
            let search_active = self.search_bar.read(cx).is_active();

            if !header_renaming && !search_active && !is_modal {
                if let Some(focused) = ws.focus_manager.focused_terminal_state() {
                    if focused.project_id == self.project_id
                        && focused.layout_path == self.layout_path
                        && !focus_handle.is_focused(window)
                    {
                        self.pending_focus = true;
                    }
                }

                // Fullscreen focus: if this terminal is in fullscreen and doesn't have focus, restore it
                if let Some(ref tid) = self.terminal_id {
                    if ws.focus_manager.is_terminal_fullscreened(&self.project_id, tid)
                        && !focus_handle.is_focused(window)
                    {
                        self.pending_focus = true;
                    }
                }
            }
            is_modal
        };

        // Handle pending focus
        let header_renaming = self.header.read(cx).is_renaming();
        let search_active = self.search_bar.read(cx).is_active();
        if self.pending_focus
            && self.terminal.is_some()
            && !header_renaming
            && !search_active
            && !is_modal
        {
            self.pending_focus = false;
            window.focus(&self.focus_handle, cx);
        }

        let is_focused = focus_handle.is_focused(window);

        // Bell handling
        let has_bell = self.terminal.as_ref().map_or(false, |t| t.has_bell());
        if is_focused && has_bell {
            if let Some(ref terminal) = self.terminal {
                terminal.clear_bell();
            }
        }

        let show_focused_border = settings(cx).show_focused_border;
        let show_border = (is_focused && show_focused_border) || has_bell;
        let border_color = if is_focused && show_focused_border {
            rgb(t.border_focused)
        } else {
            rgb(t.border_bell)
        };

        let in_tab_group = self.is_in_tab_group(cx);
        let is_zoomed = self.is_zoomed(cx);
        let zoom_header = if is_zoomed {
            Some(self.render_zoom_header(cx))
        } else {
            None
        };

        div()
            .id(format!("terminal-pane-main-{}", id_suffix))
            .track_focus(&focus_handle)
            .key_context("TerminalPane")
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseDownEvent, window, cx| {
                    this.header.update(cx, |header, cx| {
                        header.close_shell_dropdown(cx);
                    });
                    window.focus(&this.focus_handle, cx);
                    this.workspace.update(cx, |ws, cx| {
                        ws.set_focused_terminal(
                            this.project_id.clone(),
                            this.layout_path.clone(),
                            cx,
                        );
                    });
                }),
            )
            // Actions
            .on_action(cx.listener(|this, _: &SplitVertical, _window, cx| {
                this.handle_split(SplitDirection::Vertical, cx);
            }))
            .on_action(cx.listener(|this, _: &SplitHorizontal, _window, cx| {
                this.handle_split(SplitDirection::Horizontal, cx);
            }))
            .on_action(cx.listener(|this, _: &AddTab, _window, cx| {
                this.handle_add_tab(cx);
            }))
            .on_action(cx.listener(|this, _: &CloseTerminal, _window, cx| {
                this.handle_close(cx);
            }))
            .on_action(cx.listener(|this, _: &MinimizeTerminal, _window, cx| {
                this.handle_minimize(cx);
            }))
            .on_action(cx.listener(|this, _: &Copy, _window, cx| {
                this.handle_copy(cx);
            }))
            .on_action(cx.listener(|this, _: &Paste, _window, cx| {
                this.handle_paste(cx);
            }))
            .on_action(cx.listener(|this, _: &Search, window, cx| {
                if !this.search_bar.read(cx).is_active() {
                    this.start_search(window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &CloseSearch, _window, cx| {
                if this.search_bar.read(cx).is_active() {
                    this.close_search(cx);
                }
            }))
            .on_action(cx.listener(|this, _: &SearchNext, _window, cx| {
                this.next_match(cx);
            }))
            .on_action(cx.listener(|this, _: &SearchPrev, _window, cx| {
                this.prev_match(cx);
            }))
            .on_action(cx.listener(|this, _: &FocusLeft, window, cx| {
                this.handle_navigation(NavigationDirection::Left, window, cx);
            }))
            .on_action(cx.listener(|this, _: &FocusRight, window, cx| {
                this.handle_navigation(NavigationDirection::Right, window, cx);
            }))
            .on_action(cx.listener(|this, _: &FocusUp, window, cx| {
                this.handle_navigation(NavigationDirection::Up, window, cx);
            }))
            .on_action(cx.listener(|this, _: &FocusDown, window, cx| {
                this.handle_navigation(NavigationDirection::Down, window, cx);
            }))
            .on_action(cx.listener(|this, _: &FocusNextTerminal, window, cx| {
                this.handle_sequential_navigation(true, window, cx);
            }))
            .on_action(cx.listener(|this, _: &FocusPrevTerminal, window, cx| {
                this.handle_sequential_navigation(false, window, cx);
            }))
            .on_action(cx.listener(|this, _: &SendTab, _window, _cx| {
                if let Some(ref terminal) = this.terminal {
                    terminal.send_bytes(b"\t");
                }
            }))
            .on_action(cx.listener(|this, _: &SendBacktab, _window, _cx| {
                if let Some(ref terminal) = this.terminal {
                    terminal.send_bytes(b"\x1b[Z");
                }
            }))
            .on_action(cx.listener(|this, _: &SendEscape, _window, _cx| {
                if let Some(ref terminal) = this.terminal {
                    terminal.send_bytes(b"\x1b");
                }
            }))
            .on_action(cx.listener(|this, _: &ZoomIn, _window, cx| {
                let current = this.workspace.read(cx).get_terminal_zoom(&this.project_id, &this.layout_path);
                let new_zoom = (current + 0.1).clamp(0.5, 3.0);
                let project_id = this.project_id.clone();
                let layout_path = this.layout_path.clone();
                this.workspace.update(cx, |ws, cx| {
                    ws.set_terminal_zoom(&project_id, &layout_path, new_zoom, cx);
                });
            }))
            .on_action(cx.listener(|this, _: &ZoomOut, _window, cx| {
                let current = this.workspace.read(cx).get_terminal_zoom(&this.project_id, &this.layout_path);
                let new_zoom = (current - 0.1).clamp(0.5, 3.0);
                let project_id = this.project_id.clone();
                let layout_path = this.layout_path.clone();
                this.workspace.update(cx, |ws, cx| {
                    ws.set_terminal_zoom(&project_id, &layout_path, new_zoom, cx);
                });
            }))
            .on_action(cx.listener(|this, _: &ResetZoom, _window, cx| {
                let project_id = this.project_id.clone();
                let layout_path = this.layout_path.clone();
                this.workspace.update(cx, |ws, cx| {
                    ws.set_terminal_zoom(&project_id, &layout_path, 1.0, cx);
                });
            }))
            .on_action(cx.listener(|this, _: &ToggleFullscreen, _window, cx| {
                let is_fullscreen = this.workspace.read(cx).focus_manager.has_fullscreen();
                if is_fullscreen {
                    this.workspace.update(cx, |ws, cx| {
                        ws.exit_fullscreen(cx);
                    });
                } else {
                    this.handle_fullscreen(cx);
                }
            }))
            .on_action(cx.listener(|this, _: &FullscreenNextTerminal, _window, cx| {
                this.handle_zoom_next_terminal(cx);
            }))
            .on_action(cx.listener(|this, _: &FullscreenPrevTerminal, _window, cx| {
                this.handle_zoom_prev_terminal(cx);
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                this.handle_key(event, cx);
            }))
            .on_click(cx.listener(|this, _, window, cx| {
                window.focus(&this.focus_handle, cx);
                this.workspace.update(cx, |ws, cx| {
                    ws.set_focused_terminal(this.project_id.clone(), this.layout_path.clone(), cx);
                });
            }))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _window, cx| {
                this.handle_file_drop(paths, cx);
            }))
            .flex()
            .flex_col()
            .size_full()
            .min_h_0()
            .min_w_0()
            .bg(rgb(t.bg_primary))
            .group("terminal-pane")
            .relative()
            // Zoom header (shown when zoomed)
            .children(zoom_header)
            // Regular header (hidden in tab groups and when zoomed)
            .when(!in_tab_group && !self.minimized && !is_zoomed, |el| {
                el.child(self.header.clone())
            })
            // Content (hidden when minimized or detached)
            .when(!self.minimized && !self.detached, |el| {
                el.child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .min_w_0()
                        .overflow_hidden()
                        .relative()
                        .child(self.content.clone())
                        // Focus/bell border overlay
                        .when(show_border, |el| {
                            el.child(
                                div()
                                    .absolute()
                                    .inset_0()
                                    .border_1()
                                    .border_color(border_color),
                            )
                        }),
                )
            })
            // Search bar (when active)
            .when(search_active, |el: Stateful<Div>| {
                el.child(self.search_bar.clone())
            })
    }
}
