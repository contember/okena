use crate::settings::settings_entity;
use crate::theme::theme;
use crate::workspace::settings::CursorShape;
use gpui::prelude::FluentBuilder;
use gpui::*;
use okena_terminal::terminal::scrollback_memory_bytes;

use super::SettingsPanel;
use super::components::*;

/// Left indent of a setting that only applies while the toggle above it is on.
const NESTED_SETTING_INDENT: Pixels = px(16.0);

/// Width the scrollback cost is quoted at: terminals grow and shrink with the
/// window, and a wide pane is the case worth warning about.
const SCROLLBACK_COST_COLUMNS: u16 = 200;

/// What a full scrollback of `lines` costs the daemon, per terminal and for the
/// `open_terminals` the workspace holds now.
fn scrollback_cost(lines: u32, open_terminals: usize) -> String {
    let per_terminal = scrollback_memory_bytes(lines, SCROLLBACK_COST_COLUMNS);
    let total = per_terminal.saturating_mul(open_terminals as u64);
    let noun = if open_terminals == 1 {
        "terminal"
    } else {
        "terminals"
    };
    format!(
        "Daemon memory once full: up to {} per {SCROLLBACK_COST_COLUMNS}-column terminal, {} for the {open_terminals} open {noun}",
        format_memory(per_terminal),
        format_memory(total),
    )
}

/// `bytes` in MB below a gigabyte, else in GB with one decimal.
fn format_memory(bytes: u64) -> String {
    const MB: u64 = 1024 * 1024;
    const GB: u64 = 1024 * MB;
    if bytes < GB {
        format!("{} MB", (bytes + MB / 2) / MB)
    } else {
        let tenths = (bytes.saturating_mul(10) + GB / 2) / GB;
        format!("{}.{} GB", tenths / 10, tenths % 10)
    }
}

impl SettingsPanel {
    /// Terminals of every project, shown or not: the daemon keeps a scrollback
    /// for each of them.
    fn open_terminal_count(&self, cx: &App) -> usize {
        let workspace = self.workspace.read(cx);
        workspace
            .projects()
            .iter()
            .map(|project| workspace.all_terminal_ids_for_project(&project.id).len())
            .sum()
    }

    pub(super) fn render_terminal(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let s = settings_entity(cx).read(cx).settings.clone();

        div().child(section_header("Terminal", &t, cx)).child(
            section_container(&t)
                .child(self.render_shell_dropdown_row(&s.default_shell, cx))
                .child(self.render_session_backend_dropdown_row(&s.session_backend, cx))
                // Sits next to the session backend because that is what decides
                // whether it can ever apply: with a live backend the pane
                // reattaches to a still-running agent and nothing is resumed.
                .child(self.render_toggle(
                    "auto-resume-agent-sessions",
                    "Auto-Resume Agent Sessions",
                    s.auto_resume_agent_sessions,
                    true,
                    |state, val, cx| state.set_auto_resume_agent_sessions(val, cx),
                    cx,
                ))
                // Hidden where the daemon cannot apply it: the preload is Unix only.
                .when(cfg!(unix), |el| {
                    el.child(Self::render_toggle_with_desc(
                        "claude-remote",
                        "Claude Remote",
                        "Lets phone and web clients follow and answer claude started in new panes",
                        s.claude_remote,
                        true,
                        |state, val, cx| state.set_claude_remote(val, cx),
                        cx,
                    ))
                })
                .when(cfg!(unix) && s.claude_remote, |el| {
                    el.child(
                        div()
                            .pl(NESTED_SETTING_INDENT)
                            .child(Self::render_toggle_with_desc(
                                "claude-remote-channel",
                                "Use MCP Channel for Prompts",
                                "Claude asks for confirmation at start",
                                s.claude_remote_channel,
                                false,
                                |state, val, cx| state.set_claude_remote_channel(val, cx),
                                cx,
                            )),
                    )
                })
                .child(self.render_toggle(
                    "show-shell-selector",
                    "Show Shell Selector",
                    s.show_shell_selector,
                    true,
                    |state, val, cx| state.set_show_shell_selector(val, cx),
                    cx,
                ))
                .child(self.render_toggle(
                    "auto-hide-single-terminal-header",
                    "Auto-hide Single-Terminal Header",
                    s.auto_hide_single_terminal_header,
                    true,
                    |state, val, cx| state.set_auto_hide_single_terminal_header(val, cx),
                    cx,
                ))
                .child(self.render_toggle(
                    "show-terminal-name-when-inactive",
                    "Show Name on Inactive Terminals",
                    s.show_terminal_name_when_inactive,
                    true,
                    |state, val, cx| state.set_show_terminal_name_when_inactive(val, cx),
                    cx,
                ))
                .child(self.render_toggle(
                    "show-active-terminal-title",
                    "Show Active Terminal Title in Window Title",
                    s.show_active_terminal_title,
                    true,
                    |state, val, cx| state.set_show_active_terminal_title(val, cx),
                    cx,
                ))
                .child(self.render_cursor_style_row(s.cursor_style, cx))
                .child(self.render_toggle(
                    "cursor-blink",
                    "Cursor Blink",
                    s.cursor_blink,
                    true,
                    |state, val, cx| state.set_cursor_blink(val, cx),
                    cx,
                ))
                .child(self.render_integer_stepper_with_desc(
                    "scrollback",
                    "Scrollback Lines",
                    &scrollback_cost(s.scrollback_lines, self.open_terminal_count(cx)),
                    s.scrollback_lines,
                    1000,
                    70.0,
                    true,
                    |state, val, cx| state.set_scrollback_lines(val, cx),
                    cx,
                ))
                .child(self.render_toggle(
                    "ctrl-c-copies",
                    "Ctrl+C Copies Selection",
                    s.terminal_ctrl_c_copies_selection,
                    true,
                    |state, val, cx| state.set_terminal_ctrl_c_copies_selection(val, cx),
                    cx,
                ))
                .child(self.render_toggle(
                    "right-click-menu",
                    "Right Click Opens Menu",
                    s.terminal_right_click_opens_menu,
                    true,
                    |state, val, cx| state.set_terminal_right_click_opens_menu(val, cx),
                    cx,
                ))
                .child(self.render_toggle(
                    "drag-selects",
                    "Drag Selects in Mouse Apps",
                    s.terminal_drag_selects_in_mouse_mode,
                    true,
                    |state, val, cx| state.set_terminal_drag_selects_in_mouse_mode(val, cx),
                    cx,
                ))
                .child(self.render_toggle(
                    "double-click-selects",
                    "Double Click Selects in Mouse Apps",
                    s.terminal_double_click_selects_in_mouse_mode,
                    true,
                    |state, val, cx| state.set_terminal_double_click_selects_in_mouse_mode(val, cx),
                    cx,
                ))
                .child(self.render_toggle(
                    "option-as-meta",
                    "Option as Meta (macOS)",
                    s.terminal_option_as_meta,
                    true,
                    |state, val, cx| state.set_terminal_option_as_meta(val, cx),
                    cx,
                ))
                .child(self.render_toggle(
                    "idle-detection",
                    "Idle Detection",
                    s.idle_timeout_secs > 0,
                    true,
                    |state, val, cx| state.set_idle_timeout_secs(if val { 5 } else { 0 }, cx),
                    cx,
                ))
                .when(s.idle_timeout_secs > 0, |el| {
                    el.child(self.render_integer_stepper(
                        "idle-timeout",
                        "Idle Timeout (seconds)",
                        s.idle_timeout_secs,
                        1,
                        50.0,
                        false,
                        |state, val, cx| state.set_idle_timeout_secs(val, cx),
                        cx,
                    ))
                })
                .child(self.render_toggle(
                    "close-grace",
                    "Undo Close (busy terminals)",
                    s.terminal_close_grace_secs > 0,
                    true,
                    |state, val, cx| {
                        state.set_terminal_close_grace_secs(if val { 5 } else { 0 }, cx)
                    },
                    cx,
                ))
                .when(s.terminal_close_grace_secs > 0, |el| {
                    el.child(self.render_integer_stepper(
                        "close-grace-secs",
                        "Undo Window (seconds)",
                        s.terminal_close_grace_secs,
                        1,
                        50.0,
                        false,
                        |state, val, cx| state.set_terminal_close_grace_secs(val, cx),
                        cx,
                    ))
                }),
        )
    }

    fn render_cursor_style_row(
        &self,
        current: CursorShape,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = theme(cx);
        let variants = CursorShape::all_variants();
        let segments: Vec<Segment<'_>> = variants
            .iter()
            .map(|style| Segment {
                id: format!("{:?}", style).into(),
                label: style.display_name(),
                selected: *style == current,
            })
            .collect();

        settings_row("cursor-style".to_string(), "Cursor Style", &t, cx, true).child(
            segmented_control("cursor-style", &segments, &t, cx, move |i, _, cx| {
                let style = variants[i];
                settings_entity(cx).update(cx, |state, cx| {
                    state.set_cursor_style(style, cx);
                });
            }),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{format_memory, scrollback_cost};

    #[test]
    fn memory_reads_in_megabytes_below_a_gigabyte_and_in_gigabytes_above() {
        assert_eq!(format_memory(0), "0 MB");
        assert_eq!(format_memory(96 * 1024 * 1024), "96 MB");
        assert_eq!(format_memory(1024 * 1024 * 1024 - 1), "1024 MB");
        assert_eq!(format_memory(1024 * 1024 * 1024), "1.0 GB");
        assert_eq!(format_memory(5_900_000_000), "5.5 GB");
    }

    #[test]
    fn the_cost_scales_with_lines_and_open_terminals() {
        // 1 000 live rows x 200 columns x 24-byte cells, 9 000 archived rows
        // at up to a byte a cell.
        assert_eq!(
            scrollback_cost(10_000, 3),
            "Daemon memory once full: up to 6 MB per 200-column terminal, 19 MB for the 3 open terminals"
        );
        assert_eq!(
            scrollback_cost(10_000, 1),
            "Daemon memory once full: up to 6 MB per 200-column terminal, 6 MB for the 1 open terminal"
        );
        // Under the live rows everything sits in the grid.
        assert_eq!(
            scrollback_cost(500, 1),
            "Daemon memory once full: up to 2 MB per 200-column terminal, 2 MB for the 1 open terminal"
        );
    }
}
