//! Hook log overlay — shows recent hook execution history.

use crate::Cancel;
use okena_ui::badge::keyboard_hints_footer;
use okena_ui::modal::{modal_backdrop, modal_content, modal_header};
use okena_ui::theme::theme;
use okena_ui::tokens::{ui_text_ms, ui_text_md, ui_text};
use okena_workspace::hook_monitor::{HookExecution, HookMonitor, HookStatus};
use okena_core::theme::ThemeColors;
use gpui::*;
use gpui::prelude::*;
use gpui_component::v_flex;
use std::time::Duration;

/// Hook log overlay — shows recent hook execution history.
pub struct HookLog {
    focus_handle: FocusHandle,
    history: Vec<HookExecution>,
    /// Last seen HookMonitor version — used to skip expensive history cloning
    /// when nothing has changed.
    last_version: u64,
}

impl HookLog {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let (history, last_version) = cx
            .try_global::<HookMonitor>()
            .map(|m| (m.history(), m.version()))
            .unwrap_or_default();

        let focus_handle = cx.focus_handle();

        // Poll for changes at 1s intervals. The version check makes the common
        // case (no changes) a single lock + u64 compare with no allocation.
        cx.spawn(async move |this: WeakEntity<HookLog>, cx| {
            loop {
                smol::Timer::after(Duration::from_secs(1)).await;
                let result = this.update(cx, |this, cx| {
                    if let Some(monitor) = cx.try_global::<HookMonitor>() {
                        let current_version = monitor.version();
                        if current_version != this.last_version {
                            this.history = monitor.history();
                            this.last_version = current_version;
                            cx.notify();
                        }
                    }
                });
                if result.is_err() {
                    break;
                }
            }
        })
        .detach();

        Self { focus_handle, history, last_version }
    }

    fn close(&self, cx: &mut Context<Self>) {
        cx.emit(HookLogEvent::Close);
    }
}

pub enum HookLogEvent {
    Close,
}

impl okena_ui::overlay::CloseEvent for HookLogEvent {
    fn is_close(&self) -> bool { matches!(self, Self::Close) }
}

impl EventEmitter<HookLogEvent> for HookLog {}

fn format_duration(d: Duration) -> String {
    let ms = d.as_millis();
    if ms < 1000 {
        format!("{}ms", ms)
    } else {
        format!("{:.1}s", d.as_secs_f64())
    }
}

fn format_elapsed(execution: &HookExecution) -> String {
    let elapsed = execution.started_at.elapsed();
    match &execution.status {
        HookStatus::Running => format!("{}... (running)", format_duration(elapsed)),
        HookStatus::Succeeded { duration } => format_duration(*duration),
        HookStatus::Failed { duration, .. } => format_duration(*duration),
        HookStatus::SpawnError { .. } => "—".to_string(),
    }
}

impl Render for HookLog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let focus_handle = self.focus_handle.clone();

        if !focus_handle.is_focused(window) {
            window.focus(&focus_handle, cx);
        }

        modal_backdrop("hook-log-backdrop", &t)
            .track_focus(&focus_handle)
            .key_context("HookLog")
            .on_action(cx.listener(|this, _: &Cancel, _window, cx| {
                this.close(cx);
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _window, cx| {
                    this.close(cx);
                }),
            )
            .child(
                modal_content("hook-log-modal", &t)
                    .w(px(600.0))
                    .max_h(px(500.0))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(modal_header(
                        "Hook Log",
                        None::<&str>,
                        &t,
                        cx,
                        cx.listener(|this, _, _window, cx| this.close(cx)),
                    ))
                    .child(
                        v_flex()
                            .id("hook-log-list")
                            .flex_1()
                            .overflow_y_scroll()
                            .children(if self.history.is_empty() {
                                vec![div()
                                    .px(px(16.0))
                                    .py(px(24.0))
                                    .text_size(ui_text(13.0, cx))
                                    .text_color(rgb(t.text_muted))
                                    .child("No hooks have been executed yet.")
                                    .into_any_element()]
                            } else {
                                self.history
                                    .iter()
                                    .enumerate()
                                    .map(|(i, exec)| {
                                        render_hook_row(i, exec, &t, cx).into_any_element()
                                    })
                                    .collect()
                            }),
                    )
                    .child(keyboard_hints_footer(&[("Esc", "to close")], &t)),
            )
    }
}

fn render_hook_row(
    index: usize,
    exec: &HookExecution,
    t: &ThemeColors,
    cx: &App,
) -> impl IntoElement {
    let (status_icon, status_color) = match &exec.status {
        HookStatus::Running => ("◦", t.term_yellow),
        HookStatus::Succeeded { .. } => ("✓", t.success),
        HookStatus::Failed { .. } => ("✗", t.error),
        HookStatus::SpawnError { .. } => ("✗", t.error),
    };

    let duration_str = format_elapsed(exec);
    let hook_type = exec.hook_type.to_string();
    let project_name = exec.project_name.clone();
    let command = exec.command.clone();

    let error_detail = match &exec.status {
        HookStatus::Failed { stderr, exit_code, .. } => {
            Some(format!("Exit {}: {}", exit_code, stderr))
        }
        HookStatus::SpawnError { message } => Some(message.clone()),
        _ => None,
    };

    div()
        .id(ElementId::Name(format!("hook-{}", index).into()))
        .px(px(16.0))
        .py(px(8.0))
        .border_b_1()
        .border_color(rgb(t.border))
        .flex()
        .flex_col()
        .gap(px(3.0))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(
                    div()
                        .text_color(rgb(status_color))
                        .text_size(ui_text(13.0, cx))
                        .flex_shrink_0()
                        .child(status_icon),
                )
                .child(
                    div()
                        .text_size(ui_text_md(cx))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(t.text_primary))
                        .child(hook_type),
                )
                .child(
                    div()
                        .text_size(ui_text_ms(cx))
                        .text_color(rgb(t.text_muted))
                        .child(project_name),
                )
                .child(div().flex_1())
                .child(
                    div()
                        .text_size(ui_text_ms(cx))
                        .font_family("monospace")
                        .text_color(rgb(t.text_secondary))
                        .child(duration_str),
                ),
        )
        .child(
            div()
                .pl(px(21.0))
                .text_size(ui_text_ms(cx))
                .font_family("monospace")
                .text_color(rgb(t.text_secondary))
                .overflow_x_hidden()
                .whitespace_nowrap()
                .child(command),
        )
        .when_some(error_detail, |el, detail| {
            el.child(
                div()
                    .pl(px(21.0))
                    .text_size(ui_text_ms(cx))
                    .font_family("monospace")
                    .text_color(rgb(t.error))
                    .overflow_x_hidden()
                    .whitespace_normal()
                    .child(detail),
            )
        })
}

okena_ui::impl_focusable!(HookLog);
