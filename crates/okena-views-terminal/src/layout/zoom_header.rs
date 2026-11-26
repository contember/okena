//! Shared Focus chrome for terminals and Start Pages.

use super::header_actions::{TabActionContext, render_action_buttons, render_exit_zoom_button};
use crate::ActionDispatch;
use crate::actions::{FullscreenNextTerminal, FullscreenPrevTerminal};
use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::h_flex;
use okena_core::{api::ActionRequest, shell::ShellType};
use okena_files::theme::theme;
use okena_ui::header_action_bar::HeaderActionBar;
use okena_ui::header_buttons::{ButtonSize, HeaderAction, header_button_base};
use okena_ui::shortcuts::ActionShortcut;
use okena_ui::tokens::{ui_text_md, ui_text_ms};
use okena_workspace::{focus::FocusManager, request_broker::RequestBroker, state::Workspace};

pub(super) fn navigate_zoom<D: ActionDispatch>(
    workspace: &Entity<Workspace>,
    dispatcher: Option<&D>,
    project_id: &str,
    pane_id: &str,
    forward: bool,
    cx: &mut App,
) {
    let Some(dispatcher) = dispatcher else { return };
    let Some(layout) = workspace
        .read(cx)
        .project(project_id)
        .and_then(|p| p.layout.as_ref())
    else {
        return;
    };
    let panes = layout.collect_pane_ids();
    if panes.len() <= 1 {
        return;
    }
    if let Some(index) = panes.iter().position(|id| id == pane_id) {
        let offset = if forward { 1 } else { panes.len() - 1 };
        dispatcher.dispatch(
            ActionRequest::SetFullscreen {
                project_id: project_id.to_owned(),
                terminal_id: Some(panes[(index + offset) % panes.len()].clone()),
                window: None,
            },
            cx,
        );
    }
}

pub(super) struct ZoomHeader<'a, D: ActionDispatch> {
    pub workspace: &'a Entity<Workspace>,
    pub request_broker: &'a Entity<RequestBroker>,
    pub focus_manager: &'a Entity<FocusManager>,
    pub dispatcher: &'a Option<D>,
    pub project_id: &'a str,
    pub layout_path: &'a [usize],
    pub pane_id: Option<&'a str>,
    pub focus: &'a FocusHandle,
    pub shell: Option<ShellType>,
    pub can_export_buffer: bool,
}

fn navigation_button<D: ActionDispatch>(
    header: &ZoomHeader<'_, D>,
    forward: bool,
    cx: &App,
) -> Stateful<Div> {
    let (action, shortcut) = if forward {
        let focus = header.focus.clone();
        (
            HeaderAction::ZoomNext,
            ActionShortcut::new(FullscreenNextTerminal, move |window, cx| {
                focus.contains_focused(window, cx)
            }),
        )
    } else {
        let focus = header.focus.clone();
        (
            HeaderAction::ZoomPrev,
            ActionShortcut::new(FullscreenPrevTerminal, move |window, cx| {
                focus.contains_focused(window, cx)
            }),
        )
    };
    let workspace = header.workspace.clone();
    let dispatcher = header.dispatcher.clone();
    let project_id = header.project_id.to_owned();
    let pane_id = header.pane_id.map(str::to_owned);
    header_button_base(
        action,
        "zoom",
        ButtonSize::COMPACT,
        &theme(cx),
        None,
        Some(shortcut),
    )
    .on_click(move |_, _, cx| {
        if let Some(id) = &pane_id {
            navigate_zoom(
                &workspace,
                dispatcher.as_ref(),
                &project_id,
                id,
                forward,
                cx,
            );
        }
    })
}

impl<D: ActionDispatch + Send + Sync> ZoomHeader<'_, D> {
    pub fn render(&self, title: String, is_hook: bool, cx: &App) -> HeaderActionBar {
        let t = theme(cx);
        let id_suffix = "zoom";
        let layout = self
            .workspace
            .read(cx)
            .project(self.project_id)
            .and_then(|p| p.layout.as_ref());
        let all_panes = layout
            .map(|layout| layout.collect_pane_ids())
            .unwrap_or_default();
        let pane_count = all_panes.len();
        let current_index = self
            .pane_id
            .and_then(|id| all_panes.iter().position(|pane| pane == id))
            .unwrap_or(0);
        let has_multiple = pane_count > 1;
        let in_tab_group = layout.is_some_and(|layout| layout.is_in_tab_group(self.layout_path));
        let group_path = if in_tab_group {
            &self.layout_path[..self.layout_path.len() - 1]
        } else {
            self.layout_path
        };
        let (mut extra, always) = render_action_buttons(
            TabActionContext {
                project_id: self.project_id.to_owned(),
                layout_path: group_path.to_vec(),
                active_tab: self.layout_path.last().copied().unwrap_or(0),
                standalone: !in_tab_group,
                action_dispatcher: self.dispatcher.clone(),
            },
            self.pane_id.map(str::to_owned),
            self.request_broker.clone(),
            self.can_export_buffer,
            self.focus_manager.clone(),
            self.workspace.clone(),
            cx,
        );
        if let Some(shell) = &self.shell
            && crate::terminal_view_settings(cx).show_shell_selector
        {
            extra.insert(
                0,
                super::tabs::shell_selector::render_shell_indicator(
                    id_suffix,
                    shell.clone(),
                    self.pane_id.map(str::to_owned),
                    self.project_id.to_owned(),
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
                                    .child(title),
                            )
                            .when(has_multiple, |d| {
                                d.child(
                                    div()
                                        .text_size(ui_text_ms(cx))
                                        .text_color(rgb(t.text_muted))
                                        .debug_selector(move || {
                                            format!(
                                                "zoom-pane-position-{}-{}",
                                                current_index + 1,
                                                pane_count
                                            )
                                        })
                                        .child(format!("{}/{}", current_index + 1, pane_count)),
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
                        d.child(navigation_button(self, false, cx))
                            .child(navigation_button(self, true, cx))
                    })
                    .child(render_exit_zoom_button(
                        id_suffix,
                        self.project_id,
                        self.focus.clone(),
                        self.dispatcher.clone(),
                        cx,
                    )),
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
