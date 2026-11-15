//! Zoom (fullscreen) state and rendering for terminal panes.

use crate::ActionDispatch;
use crate::layout::header_actions::{TabActionContext, render_action_buttons};
use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::h_flex;
use okena_core::api::ActionRequest;
use okena_files::theme::theme;
use okena_ui::header_action_bar::HeaderActionBar;
use okena_ui::header_buttons::{ButtonSize, HeaderAction, header_button_base};
use okena_ui::tokens::{ui_text_md, ui_text_ms};
use okena_workspace::state::LayoutNode;

use super::TerminalPane;

impl<D: ActionDispatch + Send + Sync> TerminalPane<D> {
    pub(super) fn is_zoomed(&self, cx: &Context<Self>) -> bool {
        let fm = self.focus_manager.read(cx);
        self.terminal_id
            .as_ref()
            .is_some_and(|tid| fm.is_terminal_fullscreened(&self.project_id, tid))
    }

    fn get_project_terminals(&self, cx: &Context<Self>) -> Vec<String> {
        let ws = self.workspace.read(cx);
        ws.project(&self.project_id)
            .and_then(|p| p.layout.as_ref())
            .map(|l| l.collect_terminal_ids())
            .unwrap_or_default()
    }

    pub(super) fn handle_zoom_next_terminal(&mut self, cx: &mut Context<Self>) {
        if !self.is_zoomed(cx) {
            return;
        }
        let terminals = self.get_project_terminals(cx);
        if terminals.len() <= 1 {
            return;
        }
        if let Some(ref current_id) = self.terminal_id
            && let Some(idx) = terminals.iter().position(|id| id == current_id)
        {
            let next_idx = (idx + 1) % terminals.len();
            let next_id = terminals[next_idx].clone();
            if let Some(ref dispatcher) = self.action_dispatcher {
                dispatcher.dispatch(
                    ActionRequest::SetFullscreen {
                        project_id: self.project_id.clone(),
                        terminal_id: Some(next_id),
                        window: None,
                    },
                    cx,
                );
            }
        }
    }

    pub(super) fn handle_zoom_prev_terminal(&mut self, cx: &mut Context<Self>) {
        if !self.is_zoomed(cx) {
            return;
        }
        let terminals = self.get_project_terminals(cx);
        if terminals.len() <= 1 {
            return;
        }
        if let Some(ref current_id) = self.terminal_id
            && let Some(idx) = terminals.iter().position(|id| id == current_id)
        {
            let prev_idx = if idx == 0 {
                terminals.len() - 1
            } else {
                idx - 1
            };
            let prev_id = terminals[prev_idx].clone();
            if let Some(ref dispatcher) = self.action_dispatcher {
                dispatcher.dispatch(
                    ActionRequest::SetFullscreen {
                        project_id: self.project_id.clone(),
                        terminal_id: Some(prev_id),
                        window: None,
                    },
                    cx,
                );
            }
        }
    }

    pub(super) fn render_zoom_header(&self, cx: &Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let workspace = self.workspace.clone();
        let dispatcher = self.action_dispatcher.clone();

        let ws = self.workspace.read(cx);

        let terminal_name = if let Some(ref tid) = self.terminal_id {
            let osc_title = self.terminal.as_ref().and_then(|t| t.title());
            ws.project(&self.project_id)
                .map(|p| p.terminal_display_name(tid, osc_title))
                .unwrap_or_else(|| "Terminal".to_string())
        } else {
            "Terminal".to_string()
        };

        let is_hook = self.terminal_id.as_ref().is_some_and(|tid| {
            ws.project(&self.project_id)
                .is_some_and(|p| p.hook_terminals.contains_key(tid))
        });
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

        let parent_path = &self.layout_path[..self.layout_path.len().saturating_sub(1)];
        let in_tab_group = !self.layout_path.is_empty()
            && ws
                .project(&self.project_id)
                .and_then(|p| p.layout.as_ref())
                .and_then(|l| l.get_at_path(parent_path))
                .is_some_and(|node| matches!(node, LayoutNode::Tabs { .. }));
        let (mut extra, always) = render_action_buttons(
            TabActionContext {
                project_id: self.project_id.clone(),
                layout_path: if in_tab_group {
                    parent_path.to_vec()
                } else {
                    self.layout_path.clone()
                },
                active_tab: self.layout_path.last().copied().unwrap_or(0),
                standalone: !in_tab_group,
                action_dispatcher: self.action_dispatcher.clone(),
            },
            self.terminal_id.clone(),
            self.request_broker.clone(),
            self.backend.supports_buffer_capture(),
            self.focus_manager.clone(),
            self.workspace.clone(),
            cx,
        );
        if crate::terminal_view_settings(cx).show_shell_selector {
            extra.insert(
                0,
                crate::layout::tabs::shell_selector::render_shell_indicator(
                    "zoom",
                    self.shell_type.clone(),
                    self.terminal_id.clone(),
                    self.project_id.clone(),
                    self.request_broker.clone(),
                    cx,
                )
                .into_any_element(),
            );
        }

        let header_measure = ScrollHandle::new();
        let title_measure = ScrollHandle::new();
        let header = div()
            .id("focus-header-content")
            .track_scroll(&header_measure)
            .h(px(28.0))
            .px(px(8.0))
            .flex()
            .items_center()
            .justify_between()
            .child(
                div()
                    .id("focus-title-viewport")
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .flex()
                    .track_scroll(&title_measure)
                    .child(
                        h_flex()
                            .flex_shrink_0()
                            .gap(px(6.0))
                            .items_center()
                            .child(svg().path("icons/terminal.svg").size(px(12.0)).text_color(
                                if is_hook {
                                    rgb(t.term_yellow)
                                } else {
                                    rgb(t.success)
                                },
                            ))
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .whitespace_nowrap()
                                    .text_size(ui_text_md(cx))
                                    .text_color(rgb(t.text_primary))
                                    .child(terminal_name.clone()),
                            )
                            .when(has_multiple, |d| {
                                d.child(
                                    div()
                                        .text_size(ui_text_ms(cx))
                                        .text_color(rgb(t.text_muted))
                                        .child(format!("{}/{}", current_index + 1, terminal_count)),
                                )
                            }),
                    ),
            )
            .child(
                h_flex()
                    .flex_shrink_0()
                    .gap(px(2.0))
                    .items_center()
                    .when(has_multiple, |d| {
                        d.child(
                            header_button_base(
                                HeaderAction::ZoomPrev,
                                id_suffix,
                                size,
                                &t,
                                None,
                                Some(okena_ui::shortcuts::ActionShortcut::new(
                                    crate::actions::FullscreenPrevTerminal,
                                    {
                                        let focus = self.focus_handle.clone();
                                        move |window, _| focus.is_focused(window)
                                    },
                                )),
                            )
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
                                    if let Some(ref tid) = terminal_id
                                        && let Some(idx) = terminals.iter().position(|id| id == tid)
                                    {
                                        let prev = if idx == 0 {
                                            terminals.len() - 1
                                        } else {
                                            idx - 1
                                        };
                                        if let Some(ref dispatcher) = dispatcher {
                                            dispatcher.dispatch(
                                                ActionRequest::SetFullscreen {
                                                    project_id: project_id.clone(),
                                                    terminal_id: Some(terminals[prev].clone()),
                                                    window: None,
                                                },
                                                cx,
                                            );
                                        }
                                    }
                                }
                            }),
                        )
                        .child(
                            header_button_base(
                                HeaderAction::ZoomNext,
                                id_suffix,
                                size,
                                &t,
                                None,
                                Some(okena_ui::shortcuts::ActionShortcut::new(
                                    crate::actions::FullscreenNextTerminal,
                                    {
                                        let focus = self.focus_handle.clone();
                                        move |window, _| focus.is_focused(window)
                                    },
                                )),
                            )
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
                                    if let Some(ref tid) = terminal_id
                                        && let Some(idx) = terminals.iter().position(|id| id == tid)
                                    {
                                        let next = (idx + 1) % terminals.len();
                                        if let Some(ref dispatcher) = dispatcher {
                                            dispatcher.dispatch(
                                                ActionRequest::SetFullscreen {
                                                    project_id: project_id.clone(),
                                                    terminal_id: Some(terminals[next].clone()),
                                                    window: None,
                                                },
                                                cx,
                                            );
                                        }
                                    }
                                }
                            }),
                        )
                    })
                    .child(
                        header_button_base(
                            HeaderAction::ExitZoom,
                            id_suffix,
                            size,
                            &t,
                            None,
                            Some(okena_ui::shortcuts::ActionShortcut::new(
                                crate::actions::ToggleFullscreen,
                                {
                                    let focus = self.focus_handle.clone();
                                    move |window, _| focus.is_focused(window)
                                },
                            )),
                        )
                        .on_click({
                            let project_id = self.project_id.clone();
                            let dispatcher = dispatcher.clone();
                            move |_, _window, cx| {
                                cx.stop_propagation();
                                if let Some(ref dispatcher) = dispatcher {
                                    dispatcher.dispatch(
                                        ActionRequest::SetFullscreen {
                                            project_id: project_id.clone(),
                                            terminal_id: None,
                                            window: None,
                                        },
                                        cx,
                                    );
                                }
                            }
                        }),
                    ),
            );
        HeaderActionBar::new("zoom-header-actions", header, extra, always, move || {
            // Measure the non-shrinking title inside its clipped viewport.
            // Using the viewport itself would treat a truncated title as fitting.
            title_measure
                .bounds_for_item(0)
                .map(|b| b.size.width)
                .unwrap_or_default()
                + header_measure
                    .bounds_for_item(1)
                    .map(|b| b.size.width)
                    .unwrap_or_default()
                + px(16.0)
        })
        .background(rgb(t.term_background_unfocused))
        .min_header_width(px(if has_multiple { 80.0 } else { 36.0 }))
    }
}
