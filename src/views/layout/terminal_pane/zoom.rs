//! Zoom (fullscreen) state and rendering for terminal panes.
//!
//! Handles zoom state queries, zoom navigation between terminals,
//! and the zoom header bar UI.

use crate::theme::theme;
use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::h_flex;

use super::TerminalPane;

impl TerminalPane {
    /// Check if this pane is currently zoomed (fullscreen).
    pub(super) fn is_zoomed(&self, cx: &Context<Self>) -> bool {
        let ws = self.workspace.read(cx);
        self.terminal_id.as_ref().map_or(false, |tid| {
            ws.focus_manager.is_terminal_fullscreened(&self.project_id, tid)
        })
    }

    /// Get all terminal IDs in the current project (for zoom navigation).
    fn get_project_terminals(&self, cx: &Context<Self>) -> Vec<String> {
        let ws = self.workspace.read(cx);
        ws.project(&self.project_id)
            .and_then(|p| p.layout.as_ref())
            .map(|l| l.collect_terminal_ids())
            .unwrap_or_default()
    }

    /// Switch to the next terminal while zoomed.
    pub(super) fn handle_zoom_next_terminal(&mut self, cx: &mut Context<Self>) {
        if !self.is_zoomed(cx) {
            return;
        }
        let terminals = self.get_project_terminals(cx);
        if terminals.len() <= 1 {
            return;
        }
        if let Some(ref current_id) = self.terminal_id {
            if let Some(idx) = terminals.iter().position(|id| id == current_id) {
                let next_idx = (idx + 1) % terminals.len();
                let next_id = terminals[next_idx].clone();
                self.workspace.update(cx, |ws, cx| {
                    ws.set_fullscreen_terminal(self.project_id.clone(), next_id, cx);
                });
            }
        }
    }

    /// Switch to the previous terminal while zoomed.
    pub(super) fn handle_zoom_prev_terminal(&mut self, cx: &mut Context<Self>) {
        if !self.is_zoomed(cx) {
            return;
        }
        let terminals = self.get_project_terminals(cx);
        if terminals.len() <= 1 {
            return;
        }
        if let Some(ref current_id) = self.terminal_id {
            if let Some(idx) = terminals.iter().position(|id| id == current_id) {
                let prev_idx = if idx == 0 { terminals.len() - 1 } else { idx - 1 };
                let prev_id = terminals[prev_idx].clone();
                self.workspace.update(cx, |ws, cx| {
                    ws.set_fullscreen_terminal(self.project_id.clone(), prev_id, cx);
                });
            }
        }
    }

    /// Render the zoom header bar (shown when this pane is zoomed).
    pub(super) fn render_zoom_header(&self, cx: &Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let workspace = self.workspace.clone();

        // Get terminal info
        let ws = self.workspace.read(cx);
        let project_name = ws
            .project(&self.project_id)
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "Unknown".to_string());

        let terminal_name = if let Some(ref tid) = self.terminal_id {
            ws.project(&self.project_id)
                .and_then(|p| p.terminal_names.get(tid).cloned())
                .unwrap_or_else(|| format!("Terminal {}", tid.chars().take(8).collect::<String>()))
        } else {
            "Terminal".to_string()
        };

        let all_terminals = ws
            .project(&self.project_id)
            .and_then(|p| p.layout.as_ref())
            .map(|l| l.collect_terminal_ids())
            .unwrap_or_default();
        let terminal_count = all_terminals.len();
        let current_index = self
            .terminal_id
            .as_ref()
            .and_then(|tid| all_terminals.iter().position(|id| id == tid))
            .unwrap_or(0);
        let has_multiple = terminal_count > 1;

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
                h_flex()
                    .gap(px(12.0))
                    .child(
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
                        div()
                            .text_size(px(13.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(t.text_primary))
                            .child(terminal_name),
                    )
                    .when(has_multiple, |d| {
                        d.child(
                            div()
                                .text_size(px(11.0))
                                .text_color(rgb(t.text_muted))
                                .child(format!("{}/{}", current_index + 1, terminal_count)),
                        )
                    }),
            )
            .child(
                // Right side: Controls
                h_flex()
                    .gap(px(8.0))
                    .when(has_multiple, |d| {
                        d.child(
                            h_flex()
                                .gap(px(4.0))
                                .child(
                                    div()
                                        .id("zoom-prev-btn")
                                        .cursor_pointer()
                                        .px(px(8.0))
                                        .py(px(4.0))
                                        .rounded(px(4.0))
                                        .bg(rgb(t.bg_secondary))
                                        .hover(|s| s.bg(rgb(t.bg_hover)))
                                        .text_size(px(12.0))
                                        .text_color(rgb(t.text_primary))
                                        .child("◀ Prev")
                                        .on_click({
                                            let workspace = workspace.clone();
                                            let project_id = self.project_id.clone();
                                            let terminal_id = self.terminal_id.clone();
                                            move |_, _window, cx| {
                                                let terminals = {
                                                    let ws = workspace.read(cx);
                                                    ws.project(&project_id)
                                                        .and_then(|p| p.layout.as_ref())
                                                        .map(|l| l.collect_terminal_ids())
                                                        .unwrap_or_default()
                                                };
                                                if let Some(ref tid) = terminal_id {
                                                    if let Some(idx) = terminals.iter().position(|id| id == tid) {
                                                        let prev = if idx == 0 { terminals.len() - 1 } else { idx - 1 };
                                                        workspace.update(cx, |ws, cx| {
                                                            ws.set_fullscreen_terminal(project_id.clone(), terminals[prev].clone(), cx);
                                                        });
                                                    }
                                                }
                                            }
                                        }),
                                )
                                .child(
                                    div()
                                        .id("zoom-next-btn")
                                        .cursor_pointer()
                                        .px(px(8.0))
                                        .py(px(4.0))
                                        .rounded(px(4.0))
                                        .bg(rgb(t.bg_secondary))
                                        .hover(|s| s.bg(rgb(t.bg_hover)))
                                        .text_size(px(12.0))
                                        .text_color(rgb(t.text_primary))
                                        .child("Next ▶")
                                        .on_click({
                                            let workspace = workspace.clone();
                                            let project_id = self.project_id.clone();
                                            let terminal_id = self.terminal_id.clone();
                                            move |_, _window, cx| {
                                                let terminals = {
                                                    let ws = workspace.read(cx);
                                                    ws.project(&project_id)
                                                        .and_then(|p| p.layout.as_ref())
                                                        .map(|l| l.collect_terminal_ids())
                                                        .unwrap_or_default()
                                                };
                                                if let Some(ref tid) = terminal_id {
                                                    if let Some(idx) = terminals.iter().position(|id| id == tid) {
                                                        let next = (idx + 1) % terminals.len();
                                                        workspace.update(cx, |ws, cx| {
                                                            ws.set_fullscreen_terminal(project_id.clone(), terminals[next].clone(), cx);
                                                        });
                                                    }
                                                }
                                            }
                                        }),
                                ),
                        )
                    })
                    .child(
                        div()
                            .w(px(1.0))
                            .h(px(20.0))
                            .bg(rgb(t.border)),
                    )
                    .child(
                        div()
                            .id("zoom-close-btn")
                            .cursor_pointer()
                            .px(px(8.0))
                            .py(px(4.0))
                            .rounded(px(4.0))
                            .bg(rgb(t.bg_secondary))
                            .hover(|s| s.bg(rgb(t.bg_hover)))
                            .text_size(px(12.0))
                            .text_color(rgb(t.text_primary))
                            .child("✕ Exit Zoom")
                            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                cx.stop_propagation();
                            })
                            .on_click({
                                let workspace = workspace.clone();
                                move |_, _window, cx| {
                                    cx.stop_propagation();
                                    workspace.update(cx, |ws, cx| {
                                        ws.exit_fullscreen(cx);
                                    });
                                }
                            }),
                    ),
            )
    }
}
