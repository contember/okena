//! Remote app pane — renders KruhViewState received from a connected server.
//!
//! Created by `LayoutContainer` when a remote project contains an App layout node.
//! State is pushed in via `update_state_value` when a `ConnectionEvent::AppStateChanged`
//! arrives in `RemoteConnectionManager`.

use crate::action_dispatch::ActionDispatcher;
use crate::theme::theme;
use crate::ui::tokens::*;
use crate::views::layout::kruh_pane::types::{KruhAction, KruhScreen, KruhViewState};
use gpui::prelude::*;
use gpui::*;
use okena_core::api::ActionRequest;

pub struct RemoteAppPane {
    app_id: String,
    #[allow(dead_code)]
    app_kind: String,
    project_id: String,
    state: Option<KruhViewState>,
    dispatcher: Option<ActionDispatcher>,
    pub focus_handle: FocusHandle,
}

impl RemoteAppPane {
    pub fn new(
        app_id: String,
        app_kind: String,
        initial_state: Option<KruhViewState>,
        dispatcher: Option<ActionDispatcher>,
        project_id: String,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        Self {
            app_id,
            app_kind,
            project_id,
            state: initial_state,
            dispatcher,
            focus_handle,
        }
    }

    /// Update the view state from a raw JSON value (deserialized in this method).
    pub fn update_state_value(&mut self, value: serde_json::Value, cx: &mut Context<Self>) {
        if let Ok(view_state) = serde_json::from_value::<KruhViewState>(value) {
            self.state = Some(view_state);
            cx.notify();
        }
    }

    fn dispatch_action(&self, action: KruhAction, cx: &mut impl AppContext) {
        let payload = match serde_json::to_value(&action) {
            Ok(v) => v,
            Err(e) => {
                log::warn!("Failed to serialize KruhAction: {}", e);
                return;
            }
        };
        if let Some(ref dispatcher) = self.dispatcher {
            dispatcher.dispatch(
                ActionRequest::AppAction {
                    project_id: self.project_id.clone(),
                    app_id: self.app_id.clone(),
                    payload,
                },
                cx,
            );
        }
    }
}

impl Render for RemoteAppPane {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        let body: AnyElement = match &self.state {
            None => div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .text_size(TEXT_MD)
                        .text_color(rgb(t.text_muted))
                        .child("Connecting to remote app..."),
                )
                .into_any_element(),

            Some(state) => match &state.screen {
                KruhScreen::Scanning => div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .text_size(TEXT_MD)
                            .text_color(rgb(t.text_muted))
                            .child("Scanning for plans..."),
                    )
                    .into_any_element(),

                KruhScreen::PlanPicker { plans, selected_index } => {
                    let mut list = div()
                        .id("remote-plan-list")
                        .flex_col()
                        .flex_1()
                        .w_full()
                        .min_h_0()
                        .overflow_y_scroll();
                    for (i, plan) in plans.iter().enumerate() {
                        let is_selected = i == *selected_index;
                        let plan_name = plan.name.clone();
                        let completed = plan.completed_count;
                        let total = plan.issue_count;

                        let mut card = div()
                            .id(ElementId::Name(format!("remote-plan-{i}").into()))
                            .px(SPACE_MD)
                            .py(SPACE_SM)
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _window, cx| {
                                this.dispatch_action(KruhAction::SelectPlan { index: i }, cx);
                            }));

                        if is_selected {
                            card = card.bg(rgb(t.bg_secondary));
                        } else {
                            card = card.hover(|s| s.bg(rgb(t.bg_hover)));
                        }

                        card = card
                            .child(
                                div()
                                    .text_size(TEXT_MD)
                                    .text_color(rgb(t.text_primary))
                                    .child(plan_name.clone()),
                            )
                            .child(
                                div()
                                    .text_size(TEXT_SM)
                                    .text_color(rgb(t.text_muted))
                                    .child(format!("{}/{} done", completed, total)),
                            );

                        list = list.child(card);
                    }
                    list.into_any_element()
                }

                KruhScreen::TaskBrowser { plan_name, issues } => {
                    let plan_name = plan_name.clone();
                    let mut list = div()
                        .id("remote-task-list")
                        .flex_col()
                        .flex_1()
                        .w_full()
                        .min_h_0()
                        .overflow_y_scroll();
                    list = list.child(
                        div()
                            .px(SPACE_MD)
                            .py(SPACE_SM)
                            .border_b_1()
                            .border_color(rgb(t.border))
                            .child(
                                div()
                                    .text_size(TEXT_MD)
                                    .text_color(rgb(t.text_primary))
                                    .child(plan_name),
                            ),
                    );
                    for issue in issues.iter() {
                        let status_color = match issue.status.as_str() {
                            "completed" => rgb(t.success),
                            "in_progress" => rgb(t.term_yellow),
                            _ => rgb(t.text_muted),
                        };
                        list = list.child(
                            div()
                                .px(SPACE_MD)
                                .py(SPACE_XS)
                                .flex()
                                .gap(SPACE_SM)
                                .child(
                                    div()
                                        .text_size(TEXT_SM)
                                        .text_color(status_color)
                                        .child(issue.status.clone()),
                                )
                                .child(
                                    div()
                                        .text_size(TEXT_SM)
                                        .text_color(rgb(t.text_primary))
                                        .child(issue.title.clone()),
                                ),
                        );
                    }
                    list.into_any_element()
                }

                KruhScreen::LoopOverview { loops, focused_index } => {
                    let mut list = div()
                        .id("remote-loop-list")
                        .flex_col()
                        .flex_1()
                        .w_full()
                        .min_h_0()
                        .overflow_y_scroll();
                    for (i, lp) in loops.iter().enumerate() {
                        let is_focused = i == *focused_index;
                        let plan_name = lp.plan_name.clone();
                        let state_str = lp.state.clone();
                        let phase_str = lp.phase.clone();
                        let completed = lp.progress.completed;
                        let total = lp.progress.total;

                        let mut card = div()
                            .id(ElementId::Name(format!("remote-loop-{i}").into()))
                            .px(SPACE_MD)
                            .py(SPACE_SM)
                            .flex()
                            .flex_col()
                            .gap(SPACE_XS)
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _window, cx| {
                                this.dispatch_action(KruhAction::FocusLoop { index: i }, cx);
                            }));

                        if is_focused {
                            card = card.bg(rgb(t.bg_secondary));
                        } else {
                            card = card.hover(|s| s.bg(rgb(t.bg_hover)));
                        }

                        card = card
                            .child(
                                div()
                                    .text_size(TEXT_MD)
                                    .text_color(rgb(t.text_primary))
                                    .child(plan_name),
                            )
                            .child(
                                div()
                                    .text_size(TEXT_SM)
                                    .text_color(rgb(t.text_muted))
                                    .child(format!("{} — {}", state_str, phase_str)),
                            )
                            .child(
                                div()
                                    .text_size(TEXT_SM)
                                    .text_color(rgb(t.text_muted))
                                    .child(format!("{}/{} done", completed, total)),
                            );

                        list = list.child(card);
                    }
                    list.into_any_element()
                }

                KruhScreen::Settings { model, max_iterations, auto_start } => div()
                    .flex_col()
                    .flex_1()
                    .w_full()
                    .min_h_0()
                    .px(SPACE_MD)
                    .py(SPACE_MD)
                    .gap(SPACE_SM)
                    .child(
                        div()
                            .text_size(TEXT_MD)
                            .text_color(rgb(t.text_muted))
                            .child(format!("Model: {}", model)),
                    )
                    .child(
                        div()
                            .text_size(TEXT_MD)
                            .text_color(rgb(t.text_muted))
                            .child(format!("Max iterations: {}", max_iterations)),
                    )
                    .child(
                        div()
                            .text_size(TEXT_MD)
                            .text_color(rgb(t.text_muted))
                            .child(format!("Auto start: {}", auto_start)),
                    )
                    .into_any_element(),

                KruhScreen::Editing { file_path, content, .. } => {
                    let file_path = file_path.clone();
                    let content = content.clone();
                    div()
                        .flex_col()
                        .flex_1()
                        .w_full()
                        .min_h_0()
                        .child(
                            div()
                                .px(SPACE_MD)
                                .py(SPACE_SM)
                                .flex_shrink_0()
                                .border_b_1()
                                .border_color(rgb(t.border))
                                .child(
                                    div()
                                        .text_size(TEXT_SM)
                                        .text_color(rgb(t.text_muted))
                                        .child(file_path),
                                ),
                        )
                        .child(
                            div()
                                .id("remote-edit-content")
                                .flex_1()
                                .min_h_0()
                                .overflow_y_scroll()
                                .px(SPACE_MD)
                                .py(SPACE_SM)
                                .child(
                                    div()
                                        .text_size(TEXT_SM)
                                        .text_color(rgb(t.text_primary))
                                        .child(content),
                                ),
                        )
                        .into_any_element()
                }
            },
        };

        div()
            .flex_1()
            .w_full()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(rgb(t.bg_primary))
            .text_color(rgb(t.text_primary))
            .track_focus(&self.focus_handle)
            .child(body)
    }
}
