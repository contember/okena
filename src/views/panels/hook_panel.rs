//! HookPanel — per-project hook terminal panel, similar to ServicePanel.
//!
//! Displays hook terminal outputs in a toggleable panel below the main
//! terminal layout. Each hook terminal gets a tab; clicking a tab shows
//! its output in a TerminalPane.

use crate::action_dispatch::ActionDispatcher;
use crate::terminal::backend::TerminalBackend;
use crate::theme::ThemeColors;
use crate::ui::tokens::{ui_text_md, ui_text_ms, ui_text_sm};
use crate::views::root::TerminalsRegistry;
use crate::workspace::request_broker::RequestBroker;
use crate::workspace::state::{HookTerminalEntry, HookTerminalStatus, Workspace};

use gpui::prelude::*;
use gpui::*;
use gpui_component::tooltip::Tooltip;
use okena_ui::icon_button::icon_button_sized;
use okena_views_terminal::elements::resize_handle::ResizeHandle;
use okena_views_terminal::layout::split_pane::{ActiveDrag, DragState};
use okena_views_terminal::layout::terminal_pane::TerminalPane;

use std::sync::Arc;

/// Per-project hook terminal panel entity.
pub struct HookPanel {
    project_id: String,
    workspace: Entity<Workspace>,
    request_broker: Entity<RequestBroker>,
    backend: Arc<dyn TerminalBackend>,
    terminals: TerminalsRegistry,
    active_drag: ActiveDrag,

    /// Whether the hook panel is open.
    panel_open: bool,
    /// Currently active hook terminal ID.
    active_terminal_id: Option<String>,
    /// Terminal pane showing the active hook's output.
    terminal_pane: Option<Entity<TerminalPane<ActionDispatcher>>>,
    /// Height of the hook panel in pixels.
    panel_height: f32,
    /// Number of hook terminals last observed (for auto-open on new hooks).
    last_hook_count: usize,
}

impl HookPanel {
    pub fn new(
        project_id: String,
        workspace: Entity<Workspace>,
        request_broker: Entity<RequestBroker>,
        backend: Arc<dyn TerminalBackend>,
        terminals: TerminalsRegistry,
        active_drag: ActiveDrag,
        initial_height: f32,
        cx: &mut Context<Self>,
    ) -> Self {
        let initial_count = workspace.read(cx).project(&project_id)
            .map(|p| p.hook_terminals.len())
            .unwrap_or(0);

        // Observe workspace — auto-open when new hook terminals appear,
        // auto-close when all hooks finish.
        cx.observe(&workspace, |this: &mut Self, ws, cx| {
            let project = ws.read(cx).project(&this.project_id);
            let current_count = project.map(|p| p.hook_terminals.len()).unwrap_or(0);

            if current_count > this.last_hook_count {
                // New hook terminal(s) appeared — auto-open with the latest one
                if let Some(newest_tid) = project
                    .and_then(|p| {
                        p.hook_terminals.keys()
                            .find(|k| !this.active_terminal_id.as_ref().is_some_and(|a| a == *k))
                            .or_else(|| p.hook_terminals.keys().last())
                            .cloned()
                    })
                {
                    this.show_hook(&newest_tid, cx);
                }
            } else if this.panel_open && current_count > 0 {
                // Auto-close when all hooks succeeded (stay open on failures)
                let all_succeeded = project
                    .map(|p| p.hook_terminals.values()
                        .all(|e| e.status == HookTerminalStatus::Succeeded))
                    .unwrap_or(false);
                if all_succeeded {
                    this.close(cx);
                }
            }

            this.last_hook_count = current_count;
            cx.notify();
        }).detach();

        Self {
            project_id,
            workspace,
            request_broker,
            backend,
            terminals,
            active_drag,
            panel_open: false,
            active_terminal_id: None,
            terminal_pane: None,
            panel_height: initial_height,
            last_hook_count: initial_count,
        }
    }

    /// Whether the hook panel is currently open.
    #[allow(dead_code)]
    pub fn is_open(&self) -> bool {
        self.panel_open
    }

    /// Show a specific hook terminal in the panel.
    pub fn show_hook(&mut self, terminal_id: &str, cx: &mut Context<Self>) {
        self.active_terminal_id = Some(terminal_id.to_string());
        self.panel_open = true;

        let project_path = self.workspace.read(cx).project(&self.project_id)
            .map(|p| p.path.clone())
            .unwrap_or_default();

        let ws = self.workspace.clone();
        let rb = self.request_broker.clone();
        let backend = self.backend.clone();
        let terminals = self.terminals.clone();
        let pid = self.project_id.clone();
        let tid = terminal_id.to_string();

        let pane = cx.new(move |cx| {
            TerminalPane::new(
                ws,
                rb,
                pid,
                project_path,
                vec![usize::MAX],
                Some(tid),
                false,
                false,
                backend,
                terminals,
                None,
                cx,
            )
        });

        self.terminal_pane = Some(pane);
        cx.notify();
    }

    /// Toggle the panel open with the first hook, or close if already open.
    pub fn toggle(&mut self, cx: &mut Context<Self>) {
        if self.panel_open {
            self.close(cx);
        } else {
            // Open with the first hook terminal, or just open empty
            let first_tid = self.workspace.read(cx).project(&self.project_id)
                .and_then(|p| p.hook_terminals.keys().next().cloned());
            if let Some(tid) = first_tid {
                self.show_hook(&tid, cx);
            } else {
                self.panel_open = true;
                cx.notify();
            }
        }
    }

    /// Close the hook panel.
    pub fn close(&mut self, cx: &mut Context<Self>) {
        self.panel_open = false;
        self.terminal_pane = None;
        self.active_terminal_id = None;
        cx.notify();
    }

    /// Set the panel height (called during drag resize).
    pub fn set_panel_height(&mut self, height: f32, cx: &mut Context<Self>) {
        self.panel_height = height.clamp(80.0, 600.0);
        let project_id = self.project_id.clone();
        let h = self.panel_height;
        self.workspace.update(cx, |ws, cx| {
            ws.update_hook_panel_height(&project_id, h, cx);
        });
        cx.notify();
    }

    /// Get the hook terminals for this project.
    fn get_hook_list(&self, cx: &Context<Self>) -> Vec<(String, HookTerminalEntry)> {
        self.workspace.read(cx).project(&self.project_id)
            .map(|p| {
                p.hook_terminals.iter()
                    .map(|(id, entry)| (id.clone(), entry.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Rerun a hook by killing the old PTY and creating a new one.
    fn rerun_hook(&mut self, terminal_id: &str, command: &str, cwd: &str, cx: &mut Context<Self>) {
        let Some(runner) = cx.try_global::<crate::workspace::hooks::HookRunner>().cloned() else {
            log::warn!("Cannot rerun hook: no HookRunner available");
            return;
        };

        // Kill old PTY
        runner.backend.kill(terminal_id);

        match runner.backend.create_terminal(cwd, None) {
            Ok(new_terminal_id) => {
                let transport = runner.backend.transport();
                let terminal = Arc::new(okena_terminal::terminal::Terminal::new(
                    new_terminal_id.clone(),
                    okena_terminal::terminal::TerminalSize::default(),
                    transport.clone(),
                    cwd.to_string(),
                ));

                // Replace in TerminalsRegistry
                {
                    let mut guard = self.terminals.lock();
                    guard.remove(terminal_id);
                    guard.insert(new_terminal_id.clone(), terminal);
                }

                // Swap terminal ID in workspace
                self.workspace.update(cx, |ws, cx| {
                    ws.swap_hook_terminal_id(&self.project_id, terminal_id, &new_terminal_id, cx);
                });

                // Type the command into the new shell
                let cmd_with_newline = format!("{}\n", command);
                transport.send_input(&new_terminal_id, cmd_with_newline.as_bytes());

                // Switch the panel to the new terminal
                self.show_hook(&new_terminal_id, cx);

                log::info!("Hook rerun: replaced {} with {}", terminal_id, new_terminal_id);
            }
            Err(e) => {
                log::error!("Failed to rerun hook terminal: {}", e);
            }
        }
    }

    /// Dismiss a hook terminal.
    fn dismiss_hook(&mut self, terminal_id: &str, cx: &mut Context<Self>) {
        if let Some(monitor) = cx.try_global::<crate::workspace::hook_monitor::HookMonitor>() {
            monitor.notify_exit(terminal_id, None);
        }
        self.workspace.update(cx, |ws, cx| {
            ws.cancel_pending_worktree_close(terminal_id);
            ws.remove_hook_terminal(terminal_id, cx);
        });
        self.terminals.lock().remove(terminal_id);

        // If we just dismissed the active terminal, switch to another or close
        if self.active_terminal_id.as_deref() == Some(terminal_id) {
            let next = self.workspace.read(cx).project(&self.project_id)
                .and_then(|p| p.hook_terminals.keys().next().cloned());
            if let Some(next_tid) = next {
                self.show_hook(&next_tid, cx);
            } else {
                self.close(cx);
            }
        }
    }

    /// Dismiss all hooks that are not currently running.
    fn dismiss_finished_hooks(&mut self, cx: &mut Context<Self>) {
        let finished: Vec<String> = self.get_hook_list(cx)
            .into_iter()
            .filter(|(_, e)| e.status != HookTerminalStatus::Running)
            .map(|(id, _)| id)
            .collect();
        for tid in finished {
            self.dismiss_hook(&tid, cx);
        }
    }

    // ── Rendering ───────────────────────────────────────────────────

    /// Render the hook indicator button for the project header.
    pub fn render_hook_indicator(&self, t: &ThemeColors, cx: &mut Context<Self>) -> AnyElement {
        let hooks = self.get_hook_list(cx);

        if hooks.is_empty() {
            return div().into_any_element();
        }

        // Compute aggregate status color
        let has_failed = hooks.iter().any(|(_, e)| matches!(e.status, HookTerminalStatus::Failed { .. }));
        let has_running = hooks.iter().any(|(_, e)| e.status == HookTerminalStatus::Running);
        let all_succeeded = hooks.iter().all(|(_, e)| e.status == HookTerminalStatus::Succeeded);

        let dot_color = if has_failed {
            t.term_red
        } else if has_running {
            t.term_yellow
        } else if all_succeeded {
            t.success
        } else {
            t.text_muted
        };

        let tooltip_text = format!("{} hook{}", hooks.len(), if hooks.len() == 1 { "" } else { "s" });
        let entity = cx.entity().downgrade();

        div()
            .id("hook-indicator-btn")
            .cursor_pointer()
            .w(px(24.0))
            .h(px(24.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(4.0))
            .hover(|s| s.bg(rgb(t.bg_hover)))
            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                cx.stop_propagation();
            })
            .on_click(move |_, _window, cx| {
                cx.stop_propagation();
                if let Some(e) = entity.upgrade() {
                    e.update(cx, |this, cx| {
                        this.toggle(cx);
                    });
                }
            })
            .child(
                svg()
                    .path("icons/terminal.svg")
                    .size(px(12.0))
                    .text_color(rgb(dot_color)),
            )
            .tooltip(move |_window, cx| Tooltip::new(tooltip_text.clone()).build(_window, cx))
            .into_any_element()
    }

    /// Render the hook panel (resize handle + tab header + terminal pane).
    pub fn render_panel(&self, t: &ThemeColors, cx: &mut Context<Self>) -> AnyElement {
        if !self.panel_open {
            return div().into_any_element();
        }
        let hooks = self.get_hook_list(cx);

        if hooks.is_empty() {
            return div().into_any_element();
        }

        let active_tid = self.active_terminal_id.clone();
        let project_id = self.project_id.clone();
        let active_drag = self.active_drag.clone();
        let panel_height = self.panel_height;

        div()
            .id("hook-panel")
            .flex()
            .flex_col()
            .h(px(panel_height))
            .flex_shrink_0()
            // Resize handle
            .child(
                ResizeHandle::new(
                    true,
                    t.border,
                    t.border_active,
                    move |mouse_pos, _cx| {
                        *active_drag.borrow_mut() = Some(DragState::HookPanel {
                            project_id: project_id.clone(),
                            initial_mouse_y: f32::from(mouse_pos.y),
                            initial_height: panel_height,
                        });
                    },
                ),
            )
            // Tab header
            .child(self.render_header(&hooks, active_tid.as_deref(), t, cx))
            // Content
            .child(
                if self.terminal_pane.is_some() {
                    div()
                        .flex_1()
                        .min_h_0()
                        .min_w_0()
                        .overflow_hidden()
                        .children(self.terminal_pane.clone())
                        .into_any_element()
                } else {
                    div()
                        .flex_1()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(ui_text_ms(cx))
                        .text_color(rgb(t.text_muted))
                        .child("Select a hook to view its output")
                        .into_any_element()
                },
            )
            .into_any_element()
    }

    /// Render the tab header row.
    fn render_header(
        &self,
        hooks: &[(String, HookTerminalEntry)],
        active_tid: Option<&str>,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let entity = cx.entity().downgrade();

        div()
            .id("hook-panel-header")
            .flex_shrink_0()
            .bg(rgb(t.bg_header))
            .border_b_1()
            .border_color(rgb(t.border))
            .flex()
            .items_center()
            .child(
                // Tabs area
                div()
                    .id("hook-tabs-scroll")
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .overflow_x_scroll()
                    .children(
                        hooks.iter().map(|(tid, entry)| {
                            let is_active = active_tid == Some(tid.as_str());
                            let status_color = match &entry.status {
                                HookTerminalStatus::Running => t.term_yellow,
                                HookTerminalStatus::Succeeded => t.success,
                                HookTerminalStatus::Failed { .. } => t.error,
                            };

                            let tid_for_click = tid.clone();
                            let entity_for_click = entity.clone();

                            div()
                                .id(ElementId::Name(format!("hook-tab-{}", tid).into()))
                                .cursor_pointer()
                                .h(px(34.0))
                                .px(px(12.0))
                                .flex()
                                .items_center()
                                .flex_shrink_0()
                                .gap(px(6.0))
                                .text_size(ui_text_md(cx))
                                .when(is_active, |d| {
                                    d.bg(rgb(t.bg_primary))
                                        .text_color(rgb(t.text_primary))
                                })
                                .when(!is_active, |d| {
                                    d.text_color(rgb(t.text_secondary))
                                        .hover(|s| s.bg(rgb(t.bg_hover)))
                                })
                                .child(
                                    div()
                                        .flex_shrink_0()
                                        .w(px(7.0))
                                        .h(px(7.0))
                                        .rounded(px(3.5))
                                        .bg(rgb(status_color)),
                                )
                                .child(entry.label.clone())
                                .on_click(move |_, _window, cx| {
                                    if let Some(e) = entity_for_click.upgrade() {
                                        e.update(cx, |this, cx| {
                                            this.show_hook(&tid_for_click, cx);
                                        });
                                    }
                                })
                        }),
                    ),
            )
            // "Clear finished" link (when 2+ hooks are done)
            .when(
                hooks.iter().filter(|(_, e)| e.status != HookTerminalStatus::Running).count() >= 2,
                |d| {
                    let entity_clear = entity.clone();
                    d.child(
                        div()
                            .id("hook-clear-finished")
                            .cursor_pointer()
                            .flex_shrink_0()
                            .h(px(34.0))
                            .px(px(8.0))
                            .flex()
                            .items_center()
                            .text_size(ui_text_sm(cx))
                            .text_color(rgb(t.text_muted))
                            .hover(|s| s.text_color(rgb(t.text_secondary)))
                            .child("Clear finished")
                            .on_click(move |_, _window, cx| {
                                cx.stop_propagation();
                                if let Some(e) = entity_clear.upgrade() {
                                    e.update(cx, |this, cx| {
                                        this.dismiss_finished_hooks(cx);
                                    });
                                }
                            }),
                    )
                },
            )
            // Action buttons for active hook
            .child({
                let active_entry = active_tid.and_then(|tid| {
                    hooks.iter().find(|(id, _)| id == tid).map(|(id, e)| (id.clone(), e.clone()))
                });

                let mut actions = div()
                    .flex()
                    .flex_shrink_0()
                    .h(px(34.0))
                    .items_center()
                    .gap(px(2.0))
                    .mr(px(4.0))
                    .border_l_1()
                    .border_color(rgb(t.border))
                    .pl(px(6.0));

                if let Some((tid, entry)) = active_entry {
                    let is_running = entry.status == HookTerminalStatus::Running;

                    // Exit code badge (before action buttons)
                    if let HookTerminalStatus::Failed { exit_code } = &entry.status {
                        actions = actions.child(
                            div()
                                .px(px(5.0))
                                .py(px(1.0))
                                .rounded(px(3.0))
                                .text_size(ui_text_ms(cx))
                                .text_color(rgb(t.term_red))
                                .child(format!("exit {}", exit_code)),
                        );
                    }

                    // Rerun button (always visible, dimmed when running)
                    let entity_rerun = entity.clone();
                    let tid_rerun = tid.clone();
                    let command = entry.command.clone();
                    let cwd = entry.cwd.clone();
                    let mut rerun_btn = icon_button_sized(
                        "hook-panel-rerun", "icons/refresh.svg", 22.0, 12.0, t,
                    );
                    if is_running {
                        rerun_btn = rerun_btn
                            .opacity(0.3)
                            .cursor_default();
                    } else {
                        rerun_btn = rerun_btn
                            .on_click(move |_, _window, cx| {
                                cx.stop_propagation();
                                if let Some(e) = entity_rerun.upgrade() {
                                    e.update(cx, |this, cx| {
                                        this.rerun_hook(&tid_rerun, &command, &cwd, cx);
                                    });
                                }
                            });
                    }
                    actions = actions.child(
                        rerun_btn.tooltip(move |_window, cx| {
                            Tooltip::new(if is_running { "Running\u{2026}" } else { "Rerun Hook" })
                                .build(_window, cx)
                        }),
                    );

                    // Dismiss button (trash icon, red)
                    let entity_dismiss = entity.clone();
                    let tid_dismiss = tid.clone();
                    actions = actions.child(
                        div()
                            .id("hook-panel-dismiss")
                            .flex_shrink_0()
                            .cursor_pointer()
                            .w(px(22.0))
                            .h(px(22.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(3.0))
                            .hover(|s| s.bg(rgba(0xf14c4c33)))
                            .child(
                                svg()
                                    .path("icons/trash.svg")
                                    .size(px(12.0))
                                    .text_color(rgb(t.term_red)),
                            )
                            .on_click(move |_, _window, cx| {
                                cx.stop_propagation();
                                if let Some(e) = entity_dismiss.upgrade() {
                                    e.update(cx, |this, cx| {
                                        this.dismiss_hook(&tid_dismiss, cx);
                                    });
                                }
                            })
                            .tooltip(|_window, cx| {
                                Tooltip::new("Dismiss Hook").build(_window, cx)
                            }),
                    );
                }

                actions
            })
            // Hide panel button (chevron-down, non-destructive)
            .child(
                div()
                    .flex_shrink_0()
                    .h(px(34.0))
                    .flex()
                    .items_center()
                    .child({
                        let entity_close = entity.clone();
                        div()
                            .id("hook-panel-hide")
                            .cursor_pointer()
                            .w(px(26.0))
                            .h(px(26.0))
                            .mx(px(4.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(3.0))
                            .hover(|s| s.bg(rgb(t.bg_hover)))
                            .child(
                                svg()
                                    .path("icons/chevron-down.svg")
                                    .size(px(14.0))
                                    .text_color(rgb(t.text_secondary)),
                            )
                            .on_click(move |_, _window, cx| {
                                if let Some(e) = entity_close.upgrade() {
                                    e.update(cx, |this, cx| this.close(cx));
                                }
                            })
                            .tooltip(|_window, cx| {
                                Tooltip::new("Hide Panel").build(_window, cx)
                            })
                    }),
            )
    }
}
