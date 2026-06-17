//! Log console overlay (`ShowLogConsole`).
//!
//! A live view over the in-app log ring buffer (see `crate::logging`). Shows
//! everything captured and filters it two ways:
//! - **Capture filter** (the runtime switch): a RUST_LOG-syntax directive
//!   applied with Enter that changes what the logger actually records into the
//!   ring — escalate e.g. `okena::cmd=trace` without restarting.
//! - **Display filter**: a live substring match plus a minimum-severity chip,
//!   applied in-memory over what's already captured.

use crate::keybindings::Cancel;
use crate::logging::{self, LogLine};
use crate::theme::theme;
use crate::ui::tokens::{ui_text, ui_text_ms};
use gpui::prelude::*;
use gpui::*;
use gpui_component::{h_flex, v_flex};
use okena_core::theme::ThemeColors;
use okena_ui::badge::keyboard_hints_footer;
use okena_ui::modal::{modal_backdrop, modal_content, modal_header};
use std::time::Duration;

/// Cap on lines mirrored locally for rendering (the ring itself also caps).
const DISPLAY_CAP: usize = 10_000;
/// How often the console pulls new lines from the ring.
const POLL_INTERVAL: Duration = Duration::from_millis(250);

/// Which faux-input the keyboard edits.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ActiveField {
    Capture,
    Filter,
}

pub enum LogConsoleEvent {
    Close,
}

impl okena_ui::overlay::CloseEvent for LogConsoleEvent {
    fn is_close(&self) -> bool {
        matches!(self, Self::Close)
    }
}

pub struct LogConsole {
    focus_handle: FocusHandle,
    lines: Vec<LogLine>,
    /// Next seq we haven't pulled yet.
    cursor: u64,
    /// Editable capture directive (applied to the logger on Enter).
    capture_input: String,
    /// Live display substring filter (matches target + message).
    filter_input: String,
    /// Minimum severity to display (`Trace` = show everything).
    min_level: log::Level,
    active: ActiveField,
    auto_scroll: bool,
    scroll: UniformListScrollHandle,
    /// Set when new lines arrived and we should stick to the bottom.
    pending_scroll: bool,
}

impl LogConsole {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let (lines, cursor, capture_input) = logging::hub()
            .map(|h| (h.snapshot_since(0), h.next_seq(), h.directives()))
            .unwrap_or_else(|| (Vec::new(), 0, logging::DEFAULT_CAPTURE.to_string()));

        // Poll the ring for new lines while the console is open.
        cx.spawn(async move |this: WeakEntity<LogConsole>, cx| {
            loop {
                smol::Timer::after(POLL_INTERVAL).await;
                let alive = this
                    .update(cx, |this, cx| this.pull_new(cx))
                    .is_ok();
                if !alive {
                    break;
                }
            }
        })
        .detach();

        Self {
            focus_handle: cx.focus_handle(),
            lines,
            cursor,
            capture_input,
            filter_input: String::new(),
            min_level: log::Level::Trace,
            active: ActiveField::Filter,
            auto_scroll: true,
            scroll: UniformListScrollHandle::new(),
            pending_scroll: true,
        }
    }

    /// Append any lines captured since our cursor, capping the local mirror.
    fn pull_new(&mut self, cx: &mut Context<Self>) {
        let Some(hub) = logging::hub() else { return };
        if hub.next_seq() == self.cursor {
            return;
        }
        let mut fresh = hub.snapshot_since(self.cursor);
        if fresh.is_empty() {
            return;
        }
        self.cursor = hub.next_seq();
        self.lines.append(&mut fresh);
        if self.lines.len() > DISPLAY_CAP {
            let overflow = self.lines.len() - DISPLAY_CAP;
            self.lines.drain(0..overflow);
        }
        if self.auto_scroll {
            self.pending_scroll = true;
        }
        cx.notify();
    }

    fn close(&self, cx: &mut Context<Self>) {
        cx.emit(LogConsoleEvent::Close);
    }

    fn apply_capture(&mut self, cx: &mut Context<Self>) {
        if let Some(hub) = logging::hub() {
            hub.set_capture_filter(&self.capture_input);
        }
        cx.notify();
    }

    fn active_buf(&mut self) -> &mut String {
        match self.active {
            ActiveField::Capture => &mut self.capture_input,
            ActiveField::Filter => &mut self.filter_input,
        }
    }

    fn on_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let m = &event.keystroke.modifiers;
        if m.control || m.alt || m.platform {
            return; // leave chords / actions alone
        }
        let key = event.keystroke.key.as_str();
        match key {
            "tab" => {
                self.active = match self.active {
                    ActiveField::Capture => ActiveField::Filter,
                    ActiveField::Filter => ActiveField::Capture,
                };
            }
            "enter" => {
                if self.active == ActiveField::Capture {
                    self.apply_capture(cx);
                }
            }
            "backspace" => {
                self.active_buf().pop();
            }
            "space" => self.active_buf().push(' '),
            k if k.chars().count() == 1 => {
                if let Some(ch) = k.chars().next() {
                    self.active_buf().push(ch);
                }
            }
            _ => return,
        }
        cx.notify();
    }

    fn set_level(&mut self, level: log::Level, cx: &mut Context<Self>) {
        self.min_level = level;
        cx.notify();
    }

    /// Lines passing the display filter (severity + substring on target+msg).
    fn visible(&self) -> Vec<LogLine> {
        let needle = self.filter_input.to_ascii_lowercase();
        self.lines
            .iter()
            .filter(|l| l.level <= self.min_level)
            .filter(|l| {
                needle.is_empty()
                    || l.message.to_ascii_lowercase().contains(&needle)
                    || l.target.to_ascii_lowercase().contains(&needle)
            })
            .cloned()
            .collect()
    }
}

impl EventEmitter<LogConsoleEvent> for LogConsole {}

fn level_color(level: log::Level, t: &ThemeColors) -> u32 {
    match level {
        log::Level::Error => t.error,
        log::Level::Warn => t.term_yellow,
        log::Level::Info => t.success,
        log::Level::Debug => t.text_secondary,
        log::Level::Trace => t.text_muted,
    }
}

fn level_label(level: log::Level) -> &'static str {
    match level {
        log::Level::Error => "ERR",
        log::Level::Warn => "WRN",
        log::Level::Info => "INF",
        log::Level::Debug => "DBG",
        log::Level::Trace => "TRC",
    }
}

/// One log line as a plain, paste-friendly string (mirrors the row layout).
fn format_line_for_copy(line: &LogLine) -> String {
    format!(
        "{} {} {} {}",
        format_time(&line.timestamp),
        level_label(line.level),
        line.target,
        line.message
    )
}

fn format_time(ts: &time::OffsetDateTime) -> String {
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        ts.hour(),
        ts.minute(),
        ts.second(),
        ts.millisecond()
    )
}

impl Render for LogConsole {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let focus_handle = self.focus_handle.clone();
        if !focus_handle.is_focused(window) {
            window.focus(&focus_handle, cx);
        }

        let visible = self.visible();
        let total = self.lines.len();
        let shown = visible.len();

        if self.pending_scroll && shown > 0 {
            self.scroll
                .scroll_to_item(shown - 1, ScrollStrategy::Top);
            self.pending_scroll = false;
        }

        modal_backdrop("log-console-backdrop", &t)
            .track_focus(&focus_handle)
            .key_context("LogConsole")
            .on_action(cx.listener(|this, _: &Cancel, _window, cx| this.close(cx)))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                this.on_key(event, cx)
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _window, cx| this.close(cx)),
            )
            .child(
                modal_content("log-console-modal", &t)
                    .w(px(900.0))
                    .max_h(px(620.0))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(modal_header(
                        "Log Console",
                        Some(format!("{shown} shown / {total} buffered")),
                        &t,
                        cx,
                        cx.listener(|this, _, _window, cx| this.close(cx)),
                    ))
                    .child(self.render_toolbar(&t, cx))
                    .child(self.render_list(visible, &t, cx))
                    .child(keyboard_hints_footer(
                        &[
                            ("Tab", "switch field"),
                            ("Enter", "apply capture"),
                            ("Esc", "close"),
                        ],
                        &t,
                    )),
            )
    }
}

impl LogConsole {
    fn render_toolbar(&self, t: &ThemeColors, cx: &mut Context<Self>) -> impl IntoElement {
        let capture_active = self.active == ActiveField::Capture;
        let filter_active = self.active == ActiveField::Filter;

        v_flex()
            .px(px(16.0))
            .py(px(10.0))
            .gap(px(8.0))
            .border_b_1()
            .border_color(rgb(t.border))
            // Capture directive row (the runtime switch).
            .child(
                h_flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(field_label("capture", t, cx))
                    .child(faux_input(
                        "okena::cmd=trace,info",
                        &self.capture_input,
                        capture_active,
                        t,
                        cx,
                    )),
            )
            // Display filter + severity chips + controls.
            .child(
                h_flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(field_label("filter", t, cx))
                    .child(faux_input(
                        "substring (target or message)",
                        &self.filter_input,
                        filter_active,
                        t,
                        cx,
                    ))
                    .child(
                        h_flex()
                            .gap(px(4.0))
                            .children([
                                log::Level::Error,
                                log::Level::Warn,
                                log::Level::Info,
                                log::Level::Debug,
                                log::Level::Trace,
                            ]
                            .into_iter()
                            .map(|lvl| {
                                level_chip(lvl, self.min_level == lvl, t, cx).into_any_element()
                            })),
                    )
                    .child(toggle_chip(
                        "autoscroll",
                        self.auto_scroll,
                        t,
                        cx,
                        cx.listener(|this, _, _w, cx| {
                            this.auto_scroll = !this.auto_scroll;
                            this.pending_scroll = this.auto_scroll;
                            cx.notify();
                        }),
                    ))
                    .child(toggle_chip(
                        "copy",
                        false,
                        t,
                        cx,
                        cx.listener(|this, _, _w, cx| {
                            // Copy exactly what's on screen (respects the active
                            // filter + severity), so a review can paste the
                            // relevant slice straight out.
                            let text = this
                                .visible()
                                .iter()
                                .map(format_line_for_copy)
                                .collect::<Vec<_>>()
                                .join("\n");
                            cx.write_to_clipboard(ClipboardItem::new_string(text));
                        }),
                    ))
                    .child(toggle_chip(
                        "clear",
                        false,
                        t,
                        cx,
                        cx.listener(|this, _, _w, cx| {
                            if let Some(hub) = logging::hub() {
                                hub.clear();
                            }
                            this.lines.clear();
                            if let Some(hub) = logging::hub() {
                                this.cursor = hub.next_seq();
                            }
                            cx.notify();
                        }),
                    )),
            )
    }

    fn render_list(
        &self,
        visible: Vec<LogLine>,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = *t;
        if visible.is_empty() {
            return v_flex()
                .id("log-console-empty")
                .flex_1()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .text_size(ui_text(13.0, cx))
                        .text_color(rgb(t.text_muted))
                        .child("No log lines match the current filters."),
                )
                .into_any_element();
        }

        let count = visible.len();
        uniform_list(
            "log-console-list",
            count,
            move |range, _window, cx| {
                range
                    .map(|i| render_row(&visible[i], &t, cx).into_any_element())
                    .collect::<Vec<_>>()
            },
        )
        .track_scroll(&self.scroll)
        .flex_1()
        .into_any_element()
    }
}

fn render_row(line: &LogLine, t: &ThemeColors, cx: &App) -> impl IntoElement {
    h_flex()
        .px(px(16.0))
        .py(px(2.0))
        .gap(px(8.0))
        .items_start()
        .border_b_1()
        .border_color(rgb(t.border))
        .font_family("monospace")
        .text_size(ui_text_ms(cx))
        .child(
            div()
                .flex_shrink_0()
                .w(px(92.0))
                .text_color(rgb(t.text_muted))
                .child(format_time(&line.timestamp)),
        )
        .child(
            div()
                .flex_shrink_0()
                .w(px(34.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(level_color(line.level, t)))
                .child(level_label(line.level)),
        )
        .child(
            div()
                .flex_shrink_0()
                .w(px(170.0))
                .text_color(rgb(t.text_secondary))
                .overflow_x_hidden()
                .whitespace_nowrap()
                .child(line.target.clone()),
        )
        .child(
            div()
                .flex_1()
                .text_color(rgb(t.text_primary))
                .whitespace_normal()
                .child(line.message.clone()),
        )
}

fn field_label(label: &'static str, t: &ThemeColors, cx: &App) -> impl IntoElement {
    div()
        .w(px(52.0))
        .flex_shrink_0()
        .text_size(ui_text_ms(cx))
        .text_color(rgb(t.text_muted))
        .child(label)
}

fn faux_input(
    placeholder: &str,
    value: &str,
    active: bool,
    t: &ThemeColors,
    cx: &App,
) -> impl IntoElement {
    let (text, color) = if value.is_empty() {
        (placeholder.to_string(), t.text_muted)
    } else {
        (value.to_string(), t.text_primary)
    };
    h_flex()
        .flex_1()
        .px(px(8.0))
        .py(px(4.0))
        .rounded(px(4.0))
        .bg(rgb(t.bg_secondary))
        .border_1()
        .border_color(rgb(if active { t.border_active } else { t.border }))
        .text_size(ui_text_ms(cx))
        .font_family("monospace")
        .child(div().text_color(rgb(color)).child(text))
        .when(active, |el| {
            el.child(div().text_color(rgb(t.border_active)).child("▏"))
        })
}

fn level_chip(
    level: log::Level,
    selected: bool,
    t: &ThemeColors,
    cx: &mut Context<LogConsole>,
) -> impl IntoElement {
    let color = level_color(level, t);
    div()
        .id(ElementId::Name(format!("lvl-{}", level_label(level)).into()))
        .px(px(6.0))
        .py(px(2.0))
        .rounded(px(4.0))
        .cursor_pointer()
        .text_size(ui_text_ms(cx))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(if selected { color } else { t.text_muted }))
        .bg(rgb(if selected { t.bg_hover } else { t.bg_secondary }))
        .border_1()
        .border_color(rgb(if selected { t.border_active } else { t.border }))
        .child(level_label(level))
        .on_click(cx.listener(move |this, _, _w, cx| this.set_level(level, cx)))
}

fn toggle_chip<F>(
    label: &'static str,
    on: bool,
    t: &ThemeColors,
    cx: &App,
    handler: F,
) -> impl IntoElement
where
    F: Fn(&ClickEvent, &mut Window, &mut App) + 'static,
{
    div()
        .id(ElementId::Name(label.into()))
        .px(px(6.0))
        .py(px(2.0))
        .rounded(px(4.0))
        .cursor_pointer()
        .flex_shrink_0()
        .text_size(ui_text_ms(cx))
        .text_color(rgb(if on { t.text_primary } else { t.text_muted }))
        .bg(rgb(if on { t.bg_hover } else { t.bg_secondary }))
        .border_1()
        .border_color(rgb(if on { t.border_active } else { t.border }))
        .child(label)
        .on_click(handler)
}

okena_ui::impl_focusable!(LogConsole);
