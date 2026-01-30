use crate::terminal::pty_manager::PtyManager;
use crate::terminal::terminal::Terminal;
use crate::theme::theme;
use crate::views::layout::terminal_pane::TerminalContent;
use crate::views::root::TerminalsRegistry;
use crate::workspace::state::Workspace;
use super::terminal_overlay_utils::{
    create_terminal_content, get_or_create_terminal, handle_pending_focus, handle_terminal_key_input,
};
use gpui::*;
use gpui::prelude::FluentBuilder;
use std::sync::Arc;

/// Detached terminal window view
pub struct DetachedTerminalView {
    workspace: Entity<Workspace>,
    terminal: Arc<Terminal>,
    terminal_id: String,
    terminal_name: String,
    focus_handle: FocusHandle,
    pending_focus: bool,
    /// Flag to track if we should close the window
    should_close: bool,
    /// Terminal content view (handles selection, context menu, etc.)
    content: Entity<TerminalContent>,
}

impl DetachedTerminalView {
    pub fn new(
        workspace: Entity<Workspace>,
        terminal_id: String,
        pty_manager: Arc<PtyManager>,
        terminals: TerminalsRegistry,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();

        // Get terminal name and project_id from workspace
        let (terminal_name, project_id, layout_path, project_path) = {
            let ws = workspace.read(cx);
            let mut name = terminal_id.chars().take(8).collect::<String>();
            let mut found_project_id = String::new();
            let mut found_layout_path = vec![];
            let mut found_project_path = String::new();

            for project in ws.projects() {
                if let Some(custom_name) = project.terminal_names.get(&terminal_id) {
                    name = custom_name.clone();
                }
                // Find layout path for this terminal
                if let Some(layout) = &project.layout {
                    if let Some(path) = layout.find_terminal_path(&terminal_id) {
                        found_project_id = project.id.clone();
                        found_layout_path = path;
                        found_project_path = project.path.clone();
                        break;
                    }
                }
            }
            (name, found_project_id, found_layout_path, found_project_path)
        };

        // Get or create terminal from registry
        let terminal = get_or_create_terminal(&terminal_id, &pty_manager, &terminals, &project_path);

        // Create terminal content view
        let content = create_terminal_content(
            cx,
            focus_handle.clone(),
            project_id,
            layout_path,
            workspace.clone(),
            terminal.clone(),
        );

        // Observe workspace for changes (to detect when re-attached)
        let terminal_id_for_observer = terminal_id.clone();
        cx.observe(&workspace, move |this, workspace, cx| {
            let ws = workspace.read(cx);
            // Check if terminal is still detached
            let is_still_detached = ws.is_terminal_detached(&terminal_id_for_observer);
            if !is_still_detached && !this.should_close {
                // Terminal was re-attached, close the window
                this.should_close = true;
                cx.notify();
            }
        })
        .detach();

        // Refresh timer - checks terminal dirty flag and notifies only when content changed
        let terminal_for_refresh = terminal.clone();
        cx.spawn(async move |this: WeakEntity<DetachedTerminalView>, cx| {
            loop {
                smol::Timer::after(std::time::Duration::from_millis(16)).await; // ~60fps check rate

                // Only notify if terminal has new content
                if terminal_for_refresh.take_dirty() {
                    let should_continue = this.update(cx, |this, cx| {
                        if this.should_close {
                            return false;
                        }
                        cx.notify();
                        true
                    });
                    match should_continue {
                        Ok(true) => continue,
                        _ => break,
                    }
                } else {
                    // Check if view still exists
                    let should_continue = this.update(cx, |this, _| !this.should_close);
                    match should_continue {
                        Ok(true) => continue,
                        _ => break,
                    }
                }
            }
        })
        .detach();

        Self {
            workspace,
            terminal,
            terminal_id,
            terminal_name,
            focus_handle,
            pending_focus: true,
            should_close: false,
            content,
        }
    }

    fn handle_key(&mut self, event: &KeyDownEvent, _cx: &mut Context<Self>) {
        // Forward keys to terminal
        handle_terminal_key_input(&self.terminal, event);
    }

    fn handle_reattach(&mut self, cx: &mut Context<Self>) {
        let terminal_id = self.terminal_id.clone();
        self.workspace.update(cx, |ws, cx| {
            ws.attach_terminal(&terminal_id, cx);
        });
    }
}

impl Render for DetachedTerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Close window when terminal is re-attached
        if self.should_close {
            window.remove_window();
            // Return empty element while closing
            return div().into_any_element();
        }

        handle_pending_focus(&mut self.pending_focus, &self.focus_handle, window, cx);

        let t = theme(cx);
        let focus_handle = self.focus_handle.clone();
        let terminal_name = self.terminal_name.clone();

        let is_maximized = window.is_maximized();
        let decorations = window.window_decorations();
        let needs_controls = match decorations {
            Decorations::Server => false,
            Decorations::Client { .. } => true,
        };

        // On macOS with server decorations, we need to leave space for traffic lights
        let traffic_light_padding = if cfg!(target_os = "macos") && !needs_controls {
            px(80.0) // Space for macOS traffic lights (close, minimize, fullscreen)
        } else {
            px(12.0)
        };

        div()
            .track_focus(&focus_handle)
            .key_context("DetachedTerminal")
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _event: &MouseDownEvent, window, cx| {
                window.focus(&this.focus_handle, cx);
            }))
            .on_key_down(cx.listener(|this, event, _window, cx| {
                this.handle_key(event, cx);
            }))
            .size_full()
            .bg(rgb(t.bg_primary))
            .flex()
            .flex_col()
            .child(
                // Header bar - draggable for window move
                div()
                    .h(px(35.0))
                    .pl(traffic_light_padding)
                    .pr(px(12.0))
                    .flex()
                    .items_center()
                    .justify_between()
                    .bg(rgb(t.bg_header))
                    .border_b_1()
                    .border_color(rgb(t.border))
                    // Make header draggable for window move
                    .on_mouse_down(MouseButton::Left, |_, window, cx| {
                        window.start_window_move();
                        cx.stop_propagation();
                    })
                    .child(
                        div()
                            .text_size(px(13.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(t.text_primary))
                            .child(terminal_name),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(rgb(t.text_muted))
                                    .child("Detached"),
                            )
                            .child(
                                div()
                                    .id("reattach-btn")
                                    .cursor_pointer()
                                    .px(px(8.0))
                                    .py(px(4.0))
                                    .rounded(px(4.0))
                                    .bg(rgb(t.bg_secondary))
                                    .hover(|s| s.bg(rgb(t.bg_hover)))
                                    .text_size(px(12.0))
                                    .text_color(rgb(t.text_primary))
                                    .child("Re-attach")
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                    .on_click(cx.listener(|this, _, _window, cx| {
                                        this.handle_reattach(cx);
                                    })),
                            )
                            // Window controls - only show if client-side decorations
                            .when(needs_controls, |d| {
                                d.child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(2.0))
                                        // Minimize
                                        .child(
                                            div()
                                                .id("minimize-btn")
                                                .cursor_pointer()
                                                .w(px(28.0))
                                                .h(px(28.0))
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .rounded(px(4.0))
                                                .text_size(px(12.0))
                                                .text_color(rgb(t.text_secondary))
                                                .hover(|s| s.bg(rgb(t.bg_hover)))
                                                .child("─")
                                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                                .on_click(|_, window, cx| {
                                                    cx.stop_propagation();
                                                    window.minimize_window();
                                                }),
                                        )
                                        // Maximize/Restore
                                        .child(
                                            div()
                                                .id("maximize-btn")
                                                .cursor_pointer()
                                                .w(px(28.0))
                                                .h(px(28.0))
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .rounded(px(4.0))
                                                .text_size(px(12.0))
                                                .text_color(rgb(t.text_secondary))
                                                .hover(|s| s.bg(rgb(t.bg_hover)))
                                                .child(if is_maximized { "❐" } else { "□" })
                                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                                .on_click(|_, window, cx| {
                                                    cx.stop_propagation();
                                                    window.zoom_window();
                                                }),
                                        )
                                        // Close
                                        .child(
                                            div()
                                                .id("close-btn")
                                                .cursor_pointer()
                                                .w(px(28.0))
                                                .h(px(28.0))
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .rounded(px(4.0))
                                                .text_size(px(12.0))
                                                .text_color(rgb(t.text_secondary))
                                                .hover(|s| s.bg(rgb(0xE81123)).text_color(rgb(0xffffff)))
                                                .child("✕")
                                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                                .on_click(cx.listener(|this, _, _window, cx| {
                                                    // Close = re-attach
                                                    this.handle_reattach(cx);
                                                })),
                                        ),
                                )
                            }),
                    ),
            )
            .child(
                // Terminal content (reuses TerminalContent for selection, context menu, etc.)
                div()
                    .flex_1()
                    .min_h_0()
                    .child(self.content.clone()),
            )
            .id("detached-terminal-main")
            .on_click(cx.listener(|this, _, window, cx| {
                window.focus(&this.focus_handle, cx);
            }))
            .into_any_element()
    }
}

impl_focusable!(DetachedTerminalView);
