//! Terminal header component.
//!
//! An Entity with Render that displays terminal name, shell selector, and controls.

use crate::keybindings::Cancel;
use crate::settings::settings;
use crate::terminal::shell_config::ShellType;
use crate::terminal::terminal::Terminal;
use crate::theme::theme;
use crate::ui::ClickDetector;
use crate::views::components::{cancel_rename, finish_rename, start_rename, rename_input, RenameState, SimpleInput};
use crate::views::chrome::header_buttons::{header_button_base, ButtonSize, HeaderAction};
use crate::views::layout::pane_drag::{PaneDrag, PaneDragView};
use crate::workspace::state::{SplitDirection, Workspace};
use gpui::prelude::FluentBuilder;
use gpui::*;
use std::sync::Arc;

use super::shell_selector::{ShellSelector, ShellSelectorEvent};

/// Events emitted by TerminalHeader.
#[derive(Clone)]
pub enum HeaderEvent {
    Split(SplitDirection),
    AddTab,
    Close,
    Minimize,
    Fullscreen,
    Detach,
    ExportBuffer,
    Renamed(String),
    /// Request to open shell selector overlay
    OpenShellSelector(ShellType),
}

impl EventEmitter<HeaderEvent> for TerminalHeader {}

/// Terminal header view with name, shell selector, and controls.
pub struct TerminalHeader {
    /// Reference to workspace for terminal names
    workspace: Entity<Workspace>,
    /// Project ID
    project_id: String,
    /// Layout path for this terminal within the project
    layout_path: Vec<usize>,
    /// Terminal ID
    terminal_id: Option<String>,
    /// Terminal reference for title
    terminal: Option<Arc<Terminal>>,
    /// Shell selector child entity
    shell_selector: Entity<ShellSelector>,
    /// Rename state
    rename_state: Option<RenameState<()>>,
    /// Double-click detector for rename
    click_detector: ClickDetector<()>,
    /// Whether PTY manager supports buffer capture
    supports_export: bool,
    /// Whether this terminal is remote (hides local-only controls)
    is_remote: bool,
    /// Unique ID suffix
    id_suffix: String,
}

impl TerminalHeader {
    pub fn new(
        workspace: Entity<Workspace>,
        project_id: String,
        layout_path: Vec<usize>,
        terminal_id: Option<String>,
        shell_type: ShellType,
        supports_export: bool,
        is_remote: bool,
        id_suffix: String,
        cx: &mut Context<Self>,
    ) -> Self {
        let shell_selector = cx.new(|cx| ShellSelector::new(shell_type, id_suffix.clone(), cx));

        // Subscribe to shell selector events
        cx.subscribe(&shell_selector, |this, _, event: &ShellSelectorEvent, cx| {
            match event {
                ShellSelectorEvent::OpenSelector => {
                    let current_shell = this.shell_selector.read(cx).current_shell().clone();
                    cx.emit(HeaderEvent::OpenShellSelector(current_shell));
                }
            }
        })
        .detach();

        Self {
            workspace,
            project_id,
            layout_path,
            terminal_id,
            terminal: None,
            shell_selector,
            rename_state: None,
            click_detector: ClickDetector::new(),
            supports_export,
            is_remote,
            id_suffix,
        }
    }

    /// Set terminal reference for title.
    pub fn set_terminal(&mut self, terminal: Option<Arc<Terminal>>) {
        self.terminal = terminal;
    }

    /// Set terminal ID.
    pub fn set_terminal_id(&mut self, terminal_id: Option<String>) {
        self.terminal_id = terminal_id;
    }

    /// Check if currently renaming.
    pub fn is_renaming(&self) -> bool {
        self.rename_state.is_some()
    }

    /// Close shell dropdown if open.
    pub fn close_shell_dropdown(&mut self, cx: &mut Context<Self>) {
        self.shell_selector.update(cx, |selector, cx| {
            selector.close(cx);
        });
    }

    /// Get terminal display name.
    fn get_terminal_name(&self, cx: &Context<Self>) -> String {
        if let Some(ref terminal_id) = self.terminal_id {
            // Check for custom name first
            let custom_name = {
                let workspace = self.workspace.read(cx);
                workspace
                    .project(&self.project_id)
                    .and_then(|p| p.terminal_names.get(terminal_id).cloned())
            };

            if let Some(name) = custom_name {
                name
            } else if let Some(ref terminal) = self.terminal {
                terminal.title().unwrap_or_else(|| terminal_id.chars().take(8).collect())
            } else {
                terminal_id.chars().take(8).collect()
            }
        } else {
            "Terminal".to_string()
        }
    }

    /// Start renaming.
    fn start_rename(&mut self, current_name: String, window: &mut Window, cx: &mut Context<Self>) {
        self.rename_state = Some(start_rename(
            (),
            &current_name,
            "Terminal name...",
            window,
            cx,
        ));
        self.workspace.update(cx, |ws, cx| ws.clear_focused_terminal(cx));
        cx.notify();
    }

    /// Finish renaming.
    fn finish_rename(&mut self, cx: &mut Context<Self>) {
        if let Some(((), new_name)) = finish_rename(&mut self.rename_state, cx) {
            cx.emit(HeaderEvent::Renamed(new_name));
        }
        self.workspace.update(cx, |ws, cx| ws.restore_focused_terminal(cx));
        cx.notify();
    }

    /// Cancel renaming.
    fn cancel_rename(&mut self, cx: &mut Context<Self>) {
        cancel_rename(&mut self.rename_state);
        self.workspace.update(cx, |ws, cx| ws.restore_focused_terminal(cx));
        cx.notify();
    }

    /// Render the controls buttons.
    fn render_controls(&self, cx: &Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let id = &self.id_suffix;
        let supports_export = self.supports_export;
        let is_remote = self.is_remote;

        div()
            .flex()
            .flex_none()
            .gap(px(2.0))
            .opacity(0.0)
            .group_hover("terminal-header", |s| s.opacity(1.0))
            .child(
                header_button_base(HeaderAction::SplitVertical, id, ButtonSize::REGULAR, &t, None)
                    .on_click(cx.listener(|_this, _, _window, cx| {
                        cx.stop_propagation();
                        cx.emit(HeaderEvent::Split(SplitDirection::Vertical));
                    })),
            )
            .child(
                header_button_base(HeaderAction::SplitHorizontal, id, ButtonSize::REGULAR, &t, None)
                    .on_click(cx.listener(|_this, _, _window, cx| {
                        cx.stop_propagation();
                        cx.emit(HeaderEvent::Split(SplitDirection::Horizontal));
                    })),
            )
            .child(
                header_button_base(HeaderAction::AddTab, id, ButtonSize::REGULAR, &t, None)
                    .on_click(cx.listener(|_this, _, _window, cx| {
                        cx.stop_propagation();
                        cx.emit(HeaderEvent::AddTab);
                    })),
            )
            .child(
                header_button_base(HeaderAction::Minimize, id, ButtonSize::REGULAR, &t, None)
                    .on_click(cx.listener(|_this, _, _window, cx| {
                        cx.stop_propagation();
                        cx.emit(HeaderEvent::Minimize);
                    })),
            )
            .when(supports_export, |el| {
                el.child(
                    header_button_base(HeaderAction::ExportBuffer, id, ButtonSize::REGULAR, &t, None)
                        .on_click(cx.listener(|_this, _, _window, cx| {
                            cx.stop_propagation();
                            cx.emit(HeaderEvent::ExportBuffer);
                        })),
                )
            })
            .child(
                header_button_base(HeaderAction::Fullscreen, id, ButtonSize::REGULAR, &t, None)
                    .on_click(cx.listener(|_this, _, _window, cx| {
                        cx.stop_propagation();
                        cx.emit(HeaderEvent::Fullscreen);
                    })),
            )
            .when(!is_remote, |el| {
                el.child(
                    header_button_base(HeaderAction::Detach, id, ButtonSize::REGULAR, &t, None)
                        .on_click(cx.listener(|_this, _, _window, cx| {
                            cx.stop_propagation();
                            cx.emit(HeaderEvent::Detach);
                        })),
                )
            })
            .child(
                header_button_base(HeaderAction::Close, id, ButtonSize::REGULAR, &t, None)
                    .on_click(cx.listener(|_this, _, _window, cx| {
                        cx.stop_propagation();
                        cx.emit(HeaderEvent::Close);
                    })),
            )
    }
}

impl Render for TerminalHeader {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let terminal_name = self.get_terminal_name(cx);
        let terminal_name_for_rename = terminal_name.clone();

        // Check if this terminal can be dragged (must have an ID and not be the only terminal)
        let can_drag = self.terminal_id.is_some() && {
            let ws = self.workspace.read(cx);
            ws.project(&self.project_id)
                .and_then(|p| p.layout.as_ref())
                .map(|l| l.collect_terminal_ids().len() > 1)
                .unwrap_or(false)
        };

        let drag_payload = can_drag.then(|| PaneDrag {
            project_id: self.project_id.clone(),
            layout_path: self.layout_path.clone(),
            terminal_id: self.terminal_id.clone().unwrap_or_default(),
            terminal_name: terminal_name.clone(),
        });

        div()
            .id("terminal-header-wrapper")
            .child(
                div()
                    .id("terminal-header")
                    .group("terminal-header")
                    .h(px(28.0))
                    .px(px(8.0))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(4.0))
                    .min_w_0()
                    .overflow_hidden()
                    .bg(rgb(t.bg_header))
                    .when_some(drag_payload, |el, payload| {
                        el.on_drag(payload, |drag, _position, _window, cx| {
                            cx.new(|_| PaneDragView::new(drag.terminal_name.clone()))
                        })
                    })
                    .child(
                        if let Some(input) = rename_input(&self.rename_state) {
                            div()
                                .id("terminal-rename-input")
                                .key_context("TerminalRename")
                                .flex_1()
                                .min_w_0()
                                .bg(rgb(t.bg_secondary))
                                .border_1()
                                .border_color(rgb(t.border_active))
                                .rounded(px(4.0))
                                .child(SimpleInput::new(input).text_size(px(12.0)))
                                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                    cx.stop_propagation();
                                })
                                .on_click(|_, _window, cx| {
                                    cx.stop_propagation();
                                })
                                .on_action(cx.listener(|this, _: &Cancel, _window, cx| {
                                    this.cancel_rename(cx);
                                }))
                                .on_key_down(cx.listener(
                                    |this, event: &KeyDownEvent, _window, cx| {
                                        cx.stop_propagation();
                                        if event.keystroke.key.as_str() == "enter" {
                                            this.finish_rename(cx);
                                        }
                                    },
                                ))
                                .into_any_element()
                        } else {
                            div()
                                .id("terminal-header-name")
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .items_center()
                                .gap(px(4.0))
                                .text_size(px(12.0))
                                .text_ellipsis()
                                .child(
                                    div()
                                        .flex_shrink_0()
                                        .text_color(rgb(t.success))
                                        .child(">")
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .text_color(rgb(t.text_primary))
                                        .text_ellipsis()
                                        .child(terminal_name)
                                )
                                .on_click(cx.listener({
                                    let name = terminal_name_for_rename;
                                    move |this, _, window, cx| {
                                        if this.click_detector.check(()) {
                                            this.start_rename(name.clone(), window, cx);
                                        }
                                    }
                                }))
                                .into_any_element()
                        },
                    )
                    .when(settings(cx).show_shell_selector && !self.is_remote, |el| {
                        el.child(
                            div()
                                .opacity(0.0)
                                .group_hover("terminal-header", |s| s.opacity(1.0))
                                .child(self.shell_selector.clone()),
                        )
                    })
                    .child(self.render_controls(cx)),
            )
    }
}
