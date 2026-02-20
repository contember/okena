//! Zoom (fullscreen) state and rendering for terminal panes.
//!
//! Handles zoom state queries, zoom navigation between terminals,
//! and the zoom header bar UI.

use crate::theme::theme;
use crate::views::chrome::header_buttons::{header_button_base, ButtonSize, HeaderAction};
use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::h_flex;
use okena_core::api::ActionRequest;

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
                if let Some(ref dispatcher) = self.action_dispatcher {
                    dispatcher.dispatch(ActionRequest::SetFullscreen {
                        project_id: self.project_id.clone(),
                        terminal_id: Some(next_id),
                    }, cx);
                }
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
                if let Some(ref dispatcher) = self.action_dispatcher {
                    dispatcher.dispatch(ActionRequest::SetFullscreen {
                        project_id: self.project_id.clone(),
                        terminal_id: Some(prev_id),
                    }, cx);
                }
            }
        }
    }

    /// Render the zoom header bar (shown when this pane is zoomed).
    pub(super) fn render_zoom_header(&self, cx: &Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let workspace = self.workspace.clone();
        let dispatcher = self.action_dispatcher.clone();

        // Get terminal info
        let ws = self.workspace.read(cx);

        let terminal_name = if let Some(ref tid) = self.terminal_id {
            let osc_title = self.terminal.as_ref().and_then(|t| t.title());
            ws.project(&self.project_id)
                .map(|p| p.terminal_display_name(tid, osc_title))
                .unwrap_or_else(|| "Terminal".to_string())
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

        let size = ButtonSize::COMPACT;
        let id_suffix = "zoom";

        div()
            .h(px(28.0))
            .px(px(8.0))
            .flex()
            .items_center()
            .justify_between()
            .bg(rgb(t.term_background_unfocused))
            .child(
                // Left side: terminal icon + name
                h_flex()
                    .gap(px(6.0))
                    .items_center()
                    .child(
                        svg()
                            .path("icons/terminal.svg")
                            .size(px(12.0))
                            .text_color(rgb(t.success)),
                    )
                    .child(
                        div()
                            .text_size(px(12.0))
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
                // Right side: nav + exit zoom
                h_flex()
                    .gap(px(2.0))
                    .items_center()
                    .when(has_multiple, |d| {
                        d.child(
                            header_button_base(HeaderAction::ZoomPrev, id_suffix, size, &t, None)
                                .on_click({
                                    let workspace = workspace.clone();
                                    let project_id = self.project_id.clone();
                                    let terminal_id = self.terminal_id.clone();
                                    let dispatcher = dispatcher.clone();
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
                                                if let Some(ref dispatcher) = dispatcher {
                                                    dispatcher.dispatch(ActionRequest::SetFullscreen {
                                                        project_id: project_id.clone(),
                                                        terminal_id: Some(terminals[prev].clone()),
                                                    }, cx);
                                                }
                                            }
                                        }
                                    }
                                }),
                        )
                        .child(
                            header_button_base(HeaderAction::ZoomNext, id_suffix, size, &t, None)
                                .on_click({
                                    let workspace = workspace.clone();
                                    let project_id = self.project_id.clone();
                                    let terminal_id = self.terminal_id.clone();
                                    let dispatcher = dispatcher.clone();
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
                                                if let Some(ref dispatcher) = dispatcher {
                                                    dispatcher.dispatch(ActionRequest::SetFullscreen {
                                                        project_id: project_id.clone(),
                                                        terminal_id: Some(terminals[next].clone()),
                                                    }, cx);
                                                }
                                            }
                                        }
                                    }
                                }),
                        )
                    })
                    .child(
                        header_button_base(HeaderAction::ExitZoom, id_suffix, size, &t, None)
                            .on_click({
                                let project_id = self.project_id.clone();
                                let dispatcher = dispatcher.clone();
                                move |_, _window, cx| {
                                    cx.stop_propagation();
                                    if let Some(ref dispatcher) = dispatcher {
                                        dispatcher.dispatch(ActionRequest::SetFullscreen {
                                            project_id: project_id.clone(),
                                            terminal_id: None,
                                        }, cx);
                                    }
                                }
                            }),
                    ),
            )
    }
}
