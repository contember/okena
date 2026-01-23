use crate::terminal::input::key_to_bytes;
use crate::terminal::pty_manager::PtyManager;
use crate::terminal::terminal::{Terminal, TerminalSize};
use crate::theme::theme;
use crate::views::layout::terminal_pane::TerminalContent;
use crate::views::root::TerminalsRegistry;
use crate::workspace::state::Workspace;
use gpui::*;
use gpui::prelude::FluentBuilder;
use std::sync::Arc;

/// Fullscreen terminal overlay
pub struct FullscreenTerminal {
    workspace: Entity<Workspace>,
    pty_manager: Arc<PtyManager>,
    terminals: TerminalsRegistry,
    terminal: Arc<Terminal>,
    terminal_id: String,
    project_id: String,
    focus_handle: FocusHandle,
    pending_focus: bool,
    /// Animation progress (0.0 to 1.0)
    animation_progress: f32,
    /// Terminal content view (handles selection, context menu, etc.)
    content: Entity<TerminalContent>,
}

impl FullscreenTerminal {
    pub fn new(
        workspace: Entity<Workspace>,
        terminal_id: String,
        project_id: String,
        pty_manager: Arc<PtyManager>,
        terminals: TerminalsRegistry,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();

        // Try to get existing terminal from registry
        let terminal = {
            let mut terminals_guard = terminals.lock();
            if let Some(existing) = terminals_guard.get(&terminal_id) {
                existing.clone()
            } else {
                // Create terminal view (connects to existing PTY)
                let size = TerminalSize {
                    cols: 120,
                    rows: 40,
                    cell_width: 8.0,
                    cell_height: 17.0,
                };
                let terminal = Arc::new(Terminal::new(terminal_id.clone(), size, pty_manager.clone()));
                terminals_guard.insert(terminal_id.clone(), terminal.clone());
                terminal
            }
        };

        // Find the actual layout path for this terminal (needed for zoom)
        let layout_path = {
            let ws = workspace.read(cx);
            ws.project(&project_id)
                .and_then(|p| p.layout.as_ref())
                .and_then(|l| l.find_terminal_path(&terminal_id))
                .unwrap_or_default()
        };

        // Create terminal content view
        let content = cx.new(|cx| {
            let mut content = TerminalContent::new(
                focus_handle.clone(),
                project_id.clone(),
                layout_path,
                workspace.clone(),
                cx,
            );
            content.set_terminal(Some(terminal.clone()), cx);
            content.set_focused(true);
            content
        });

        // Start fade-in animation with minimal opacity to ensure first paint runs
        // (GPUI may skip paint for elements with opacity 0)
        let animation_progress = 0.01;
        cx.spawn(async move |this: WeakEntity<FullscreenTerminal>, cx| {
            // Animate from 0 to 1 over ~150ms
            for i in 1..=6 {
                smol::Timer::after(std::time::Duration::from_millis(25)).await;
                let result = this.update(cx, |this, cx| {
                    this.animation_progress = i as f32 / 6.0;
                    cx.notify();
                });
                if result.is_err() {
                    break; // Entity was dropped
                }
            }
        }).detach();

        Self {
            workspace,
            pty_manager,
            terminals,
            terminal,
            terminal_id,
            project_id,
            focus_handle,
            pending_focus: true,
            animation_progress,
            content,
        }
    }

    /// Get the name of the current terminal
    fn get_terminal_name(&self, cx: &Context<Self>) -> String {
        let ws = self.workspace.read(cx);
        if let Some(project) = ws.project(&self.project_id) {
            if let Some(custom_name) = project.terminal_names.get(&self.terminal_id) {
                return custom_name.clone();
            }
        }
        // Default name based on terminal ID
        format!("Terminal {}", self.terminal_id.chars().take(8).collect::<String>())
    }

    /// Get the project name
    fn get_project_name(&self, cx: &Context<Self>) -> String {
        let ws = self.workspace.read(cx);
        ws.project(&self.project_id)
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "Unknown".to_string())
    }

    /// Get all terminal IDs in the current project for quick-switch
    fn get_project_terminals(&self, cx: &Context<Self>) -> Vec<String> {
        let ws = self.workspace.read(cx);
        ws.project(&self.project_id)
            .and_then(|p| p.layout.as_ref())
            .map(|l| l.collect_terminal_ids())
            .unwrap_or_default()
    }

    /// Switch to another terminal in fullscreen mode
    fn switch_to_terminal(&mut self, new_terminal_id: String, cx: &mut Context<Self>) {
        // Get or create the terminal
        let terminal = {
            let mut terminals_guard = self.terminals.lock();
            if let Some(existing) = terminals_guard.get(&new_terminal_id) {
                existing.clone()
            } else {
                let size = TerminalSize {
                    cols: 120,
                    rows: 40,
                    cell_width: 8.0,
                    cell_height: 17.0,
                };
                let terminal = Arc::new(Terminal::new(new_terminal_id.clone(), size, self.pty_manager.clone()));
                terminals_guard.insert(new_terminal_id.clone(), terminal.clone());
                terminal
            }
        };

        // Update workspace fullscreen state (preserve previous_focused_project_id)
        self.workspace.update(cx, |ws, cx| {
            let previous_focused_project_id = ws.fullscreen_terminal
                .as_ref()
                .and_then(|fs| fs.previous_focused_project_id.clone());
            ws.fullscreen_terminal = Some(crate::workspace::state::FullscreenState {
                project_id: self.project_id.clone(),
                terminal_id: new_terminal_id.clone(),
                previous_focused_project_id,
            });
            cx.notify();
        });

        self.terminal = terminal.clone();
        self.terminal_id = new_terminal_id.clone();

        // Find layout path for the new terminal (needed for zoom)
        let new_layout_path = {
            let ws = self.workspace.read(cx);
            ws.project(&self.project_id)
                .and_then(|p| p.layout.as_ref())
                .and_then(|l| l.find_terminal_path(&new_terminal_id))
                .unwrap_or_default()
        };

        // Update content's terminal and layout path
        self.content.update(cx, |content, cx| {
            content.set_terminal(Some(terminal), cx);
            content.set_layout_path(new_layout_path);
        });

        cx.notify();
    }

    /// Switch to the next terminal in the project
    fn next_terminal(&mut self, cx: &mut Context<Self>) {
        let terminals = self.get_project_terminals(cx);
        if terminals.len() <= 1 {
            return;
        }
        if let Some(current_idx) = terminals.iter().position(|id| id == &self.terminal_id) {
            let next_idx = (current_idx + 1) % terminals.len();
            self.switch_to_terminal(terminals[next_idx].clone(), cx);
        }
    }

    /// Switch to the previous terminal in the project
    fn prev_terminal(&mut self, cx: &mut Context<Self>) {
        let terminals = self.get_project_terminals(cx);
        if terminals.len() <= 1 {
            return;
        }
        if let Some(current_idx) = terminals.iter().position(|id| id == &self.terminal_id) {
            let prev_idx = if current_idx == 0 { terminals.len() - 1 } else { current_idx - 1 };
            self.switch_to_terminal(terminals[prev_idx].clone(), cx);
        }
    }

    fn handle_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;

        // Escape exits fullscreen
        if keystroke.key.as_str() == "escape" {
            self.workspace.update(cx, |ws, cx| {
                ws.exit_fullscreen(cx);
            });
            return;
        }

        // Forward other keys to terminal
        if let Some(input) = self.key_to_input(event) {
            self.terminal.send_bytes(&input);
            // Don't call cx.notify() here - the PTY event loop will trigger
            // a render when the terminal responds
        }
    }

    fn key_to_input(&self, event: &KeyDownEvent) -> Option<Vec<u8>> {
        // Use the shared key_to_bytes function for consistent key handling
        key_to_bytes(event)
    }
}

impl Render for FullscreenTerminal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        if self.pending_focus {
            self.pending_focus = false;
            _window.focus(&self.focus_handle, cx);
        }

        let focus_handle = self.focus_handle.clone();
        let workspace = self.workspace.clone();

        // Get terminal info for status bar
        let terminal_name = self.get_terminal_name(cx);
        let project_name = self.get_project_name(cx);
        let all_terminals = self.get_project_terminals(cx);
        let terminal_count = all_terminals.len();
        let current_index = all_terminals.iter().position(|id| id == &self.terminal_id).unwrap_or(0);
        let has_multiple_terminals = terminal_count > 1;

        // Animation opacity (smooth fade-in)
        let opacity = self.animation_progress;

        div()
            .track_focus(&focus_handle)
            .key_context("FullscreenTerminal")
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _event: &MouseDownEvent, window, cx| {
                window.focus(&this.focus_handle, cx);
            }))
            .on_key_down(cx.listener(|this, event, _window, cx| {
                this.handle_key(event, cx);
            }))
            // Register action handlers for keybindings
            .on_action(cx.listener(|this, _: &crate::keybindings::FullscreenNextTerminal, _window, cx| {
                this.next_terminal(cx);
            }))
            .on_action(cx.listener(|this, _: &crate::keybindings::FullscreenPrevTerminal, _window, cx| {
                this.prev_terminal(cx);
            }))
            .on_action(cx.listener(|this, _: &crate::keybindings::SendTab, _window, _cx| {
                this.terminal.send_bytes(b"\t");
            }))
            .on_action(cx.listener(|this, _: &crate::keybindings::SendBacktab, _window, _cx| {
                this.terminal.send_bytes(b"\x1b[Z");
            }))
            .on_action(cx.listener(|this, _: &crate::keybindings::ToggleFullscreen, _window, cx| {
                // In fullscreen mode, toggle means exit
                this.workspace.update(cx, |ws, cx| {
                    ws.exit_fullscreen(cx);
                });
            }))
            .absolute()
            .inset_0()
            .size_full()
            .min_h_0()
            .bg(rgb(t.bg_primary))
            .opacity(opacity)
            .flex()
            .flex_col()
            .child(
                // Header bar / Status bar
                div()
                    .h(px(40.0))
                    .px(px(16.0))
                    .flex()
                    .items_center()
                    .justify_between()
                    .bg(rgb(t.bg_header))
                    .border_b_1()
                    .border_color(rgb(t.border))
                    .child(
                        // Left side: Terminal info
                        div()
                            .flex()
                            .items_center()
                            .gap(px(12.0))
                            .child(
                                // Project badge
                                div()
                                    .px(px(8.0))
                                    .py(px(3.0))
                                    .rounded(px(4.0))
                                    .bg(rgb(t.bg_secondary))
                                    .text_size(px(11.0))
                                    .text_color(rgb(t.text_muted))
                                    .child(project_name),
                            )
                            .child(
                                // Terminal name
                                div()
                                    .text_size(px(13.0))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(rgb(t.text_primary))
                                    .child(terminal_name),
                            )
                            .when(has_multiple_terminals, |d| {
                                d.child(
                                    // Terminal position indicator
                                    div()
                                        .text_size(px(11.0))
                                        .text_color(rgb(t.text_muted))
                                        .child(format!("{}/{}", current_index + 1, terminal_count)),
                                )
                            }),
                    )
                    .child(
                        // Right side: Controls
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            // Quick-switch navigation (only show if multiple terminals)
                            .when(has_multiple_terminals, |d| {
                                d.child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(4.0))
                                        .child(
                                            // Previous terminal button
                                            div()
                                                .id("fullscreen-prev-btn")
                                                .cursor_pointer()
                                                .px(px(8.0))
                                                .py(px(4.0))
                                                .rounded(px(4.0))
                                                .bg(rgb(t.bg_secondary))
                                                .hover(|s| s.bg(rgb(t.bg_hover)))
                                                .text_size(px(12.0))
                                                .text_color(rgb(t.text_primary))
                                                .child("◀ Prev")
                                                .on_click(cx.listener(|this, _, _window, cx| {
                                                    this.prev_terminal(cx);
                                                })),
                                        )
                                        .child(
                                            // Next terminal button
                                            div()
                                                .id("fullscreen-next-btn")
                                                .cursor_pointer()
                                                .px(px(8.0))
                                                .py(px(4.0))
                                                .rounded(px(4.0))
                                                .bg(rgb(t.bg_secondary))
                                                .hover(|s| s.bg(rgb(t.bg_hover)))
                                                .text_size(px(12.0))
                                                .text_color(rgb(t.text_primary))
                                                .child("Next ▶")
                                                .on_click(cx.listener(|this, _, _window, cx| {
                                                    this.next_terminal(cx);
                                                })),
                                        ),
                                )
                            })
                            .child(
                                // Separator
                                div()
                                    .w(px(1.0))
                                    .h(px(20.0))
                                    .bg(rgb(t.border)),
                            )
                            .child(
                                div()
                                    .id("fullscreen-close-btn")
                                    .cursor_pointer()
                                    .px(px(8.0))
                                    .py(px(4.0))
                                    .rounded(px(4.0))
                                    .bg(rgb(t.bg_secondary))
                                    .hover(|s| s.bg(rgb(t.bg_hover)))
                                    .text_size(px(12.0))
                                    .text_color(rgb(t.text_primary))
                                    .child("✕ Close")
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                        cx.stop_propagation();
                                    })
                                    .on_click(move |_, _window, cx| {
                                        cx.stop_propagation();
                                        workspace.update(cx, |ws, cx| {
                                            ws.exit_fullscreen(cx);
                                        });
                                    }),
                            ),
                    ),
            )
            .child(
                // Terminal content (reuses TerminalContent for selection, context menu, etc.)
                div()
                    .id("fullscreen-content-wrapper")
                    .flex_1()
                    .min_h_0()
                    .size_full()
                    .overflow_hidden()
                    .child(self.content.clone()),
            )
            .id("fullscreen-terminal-main")
    }
}

impl_focusable!(FullscreenTerminal);
