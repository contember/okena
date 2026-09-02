//! Scrollback depth: construction-time sizing and runtime resizing.
//!
//! The grid is okena's dominant memory cost (24 bytes per cell × columns ×
//! history), and it is paid twice — once in the daemon that owns the PTY and
//! again in every client mirroring it. These tests pin that the user's
//! `scrollback_lines` setting actually reaches alacritty, and that shrinking a
//! live terminal really drops the rows rather than just capping future growth.

use super::super::{Terminal, TerminalOptions, TerminalSize};
use super::helpers::NullTransport;
use alacritty_terminal::grid::Dimensions;
use std::sync::Arc;

fn terminal_with(scrollback_lines: u32) -> Terminal {
    Terminal::with_options(
        "scrollback-test".to_string(),
        TerminalSize::default(),
        Arc::new(NullTransport),
        "/tmp".to_string(),
        TerminalOptions::with_scrollback_lines(scrollback_lines),
    )
}

fn feed_lines(terminal: &Terminal, count: usize) {
    for i in 0..count {
        terminal.process_output(format!("line {i}\r\n").as_bytes());
    }
}

fn history_size(terminal: &Terminal) -> usize {
    terminal.with_content(|term| term.grid().history_size())
}

#[test]
fn options_cap_the_history_at_the_configured_depth() {
    let terminal = terminal_with(200);
    feed_lines(&terminal, 1000);

    assert_eq!(
        history_size(&terminal),
        200,
        "history must stop growing at the configured scrollback depth"
    );
}

#[test]
fn default_options_keep_alacrittys_ten_thousand_lines() {
    let terminal = Terminal::new(
        "default-scrollback".to_string(),
        TerminalSize::default(),
        Arc::new(NullTransport),
        "/tmp".to_string(),
    );

    assert_eq!(terminal.scrollback_lines(), 10_000);
}

#[test]
fn shrinking_drops_history_rows() {
    let terminal = terminal_with(500);
    feed_lines(&terminal, 1000);
    assert_eq!(history_size(&terminal), 500);

    terminal.set_scrollback_lines(50);

    assert_eq!(
        history_size(&terminal),
        50,
        "shrinking must truncate existing history, not just cap future growth"
    );
    assert_eq!(terminal.scrollback_lines(), 50);
}

#[test]
fn shrinking_to_zero_leaves_only_the_viewport() {
    let terminal = terminal_with(500);
    feed_lines(&terminal, 1000);

    // What hiding a project does to a client mirror: keep the visible rows,
    // drop everything the user cannot see.
    terminal.set_scrollback_lines(0);

    assert_eq!(history_size(&terminal), 0);
    let visible = terminal.with_content(|term| term.grid().screen_lines());
    assert_eq!(visible, TerminalSize::default().rows as usize);
}

#[test]
fn growing_again_lets_history_refill() {
    let terminal = terminal_with(500);
    feed_lines(&terminal, 1000);
    terminal.set_scrollback_lines(0);
    assert_eq!(history_size(&terminal), 0);

    // Showing the project again restores the cap; new output refills history.
    terminal.set_scrollback_lines(500);
    feed_lines(&terminal, 300);

    assert_eq!(history_size(&terminal), 300);
}

#[test]
fn resizing_scrollback_preserves_the_rest_of_the_config() {
    let terminal = terminal_with(500);
    // Kitty keyboard support is enabled in the config literal; `set_options`
    // resets the keyboard-mode stacks when that flag differs between the old
    // and new config, so a scrollback change must not disturb it.
    terminal.process_output(b"\x1b[>1u");
    assert!(
        terminal
            .kitty_keyboard_flags()
            .disambiguate_escape_codes,
        "expected the app's kitty keyboard request to take effect"
    );

    terminal.set_scrollback_lines(100);

    assert!(
        terminal
            .kitty_keyboard_flags()
            .disambiguate_escape_codes,
        "changing scrollback must not reset kitty keyboard state"
    );
}

#[test]
fn setting_the_same_depth_is_a_no_op() {
    let terminal = terminal_with(500);
    feed_lines(&terminal, 1000);
    terminal.process_output(b"\x1b]0;my title\x07");

    terminal.set_scrollback_lines(500);

    // `Term::set_options` unconditionally re-emits a title event, which would
    // clobber the OSC-set title with a reset. Skipping the no-op call avoids it.
    assert_eq!(terminal.title(), Some("my title".to_string()));
    assert_eq!(history_size(&terminal), 500);
}

#[test]
fn resizing_scrollback_preserves_an_app_set_title() {
    let terminal = terminal_with(500);
    terminal.process_output(b"\x1b]0;my title\x07");

    // `Term::set_options` always re-emits a title event, which reaches our
    // shared title through the listener. It replays the term's own title, so
    // an OSC-set title must survive a real depth change too.
    terminal.set_scrollback_lines(100);

    assert_eq!(terminal.title(), Some("my title".to_string()));
}

#[test]
fn shrinking_works_while_the_app_is_in_the_alternate_screen() {
    let terminal = terminal_with(500);
    feed_lines(&terminal, 1000);
    assert_eq!(history_size(&terminal), 500);

    // Enter the alternate screen, as a full-screen app (vim, less, a TUI) does.
    // alacritty swaps the grids, so the primary — the one holding the history —
    // becomes the *inactive* grid, which is what `set_options` then resizes.
    // A long-running agent left in a TUI is exactly the terminal we most want
    // to reclaim, so this must work without waiting for it to exit.
    terminal.process_output(b"\x1b[?1049h");

    terminal.set_scrollback_lines(0);

    terminal.process_output(b"\x1b[?1049l");
    assert_eq!(
        history_size(&terminal),
        0,
        "history must be freed even when the shrink happened during alt-screen"
    );
}

#[test]
fn shrinking_drops_prompt_marks_that_fell_out_of_history() {
    let terminal = terminal_with(500);

    // Emit a prompt mark, then push it deep into scrollback.
    terminal.process_output(b"\x1b]133;A\x07prompt$ ");
    feed_lines(&terminal, 300);
    assert!(
        !terminal.prompt_marks().is_empty(),
        "expected the OSC 133 mark to be tracked while it is still in history"
    );

    terminal.set_scrollback_lines(10);

    assert!(
        terminal.prompt_marks().is_empty(),
        "marks whose rows were freed must be dropped, not left pointing above the grid"
    );
}
