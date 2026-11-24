//! History past the live grid rows sits compressed in the archive, yet scrolls,
//! searches, selects, exports and replays as one history with the grid's.

use super::super::scroll::cells_text;
use super::super::{Terminal, TerminalOptions, TerminalSize};
use super::helpers::NullTransport;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::Side;
use std::sync::Arc;

/// History rows the grid keeps in these tests.
const LIVE: usize = 20;
const ROWS: usize = 5;
const COLS: usize = 20;

fn terminal(scrollback_lines: u32) -> Terminal {
    Terminal::with_options(
        "archive-test".into(),
        TerminalSize {
            cols: COLS as u16,
            rows: ROWS as u16,
            cell_width: 8.0,
            cell_height: 16.0,
        },
        Arc::new(NullTransport),
        "/tmp".into(),
        TerminalOptions::with_scrollback_lines(scrollback_lines).with_live_history_lines(LIVE),
    )
}

/// `line <i>` for every `i`, in one write so it spans many parser steps.
fn feed_lines(terminal: &Terminal, lines: std::ops::Range<usize>) {
    let output: String = lines.map(|i| format!("line {i}\r\n")).collect();
    terminal.process_output(output.as_bytes());
}

fn lines(range: std::ops::RangeInclusive<usize>) -> String {
    range
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn grid_history(terminal: &Terminal) -> usize {
    terminal.with_content(|term| term.grid().history_size())
}

fn visible_rows(terminal: &Terminal) -> Vec<String> {
    terminal.with_history(|view| {
        (0..view.screen_lines())
            .map(|row| {
                let row = view.visible_row(row);
                cells_text(&row, 0..row.len()).trim_end().to_owned()
            })
            .collect()
    })
}

/// 300 lines into a 5-row screen: 296 history rows (content 0..=295), the
/// newest 20 of them in the grid, and `line 296..=299` plus the cursor row on
/// screen. Content row `i` sits on line `i - 296`.
fn filled() -> Terminal {
    let terminal = terminal(1_000);
    feed_lines(&terminal, 0..300);
    terminal
}

const HISTORY: usize = 296;

#[test]
fn the_grid_keeps_only_the_live_rows() {
    let terminal = filled();
    assert_eq!(grid_history(&terminal), LIVE);
    assert_eq!(terminal.scroll_info(), (HISTORY + ROWS, ROWS, 0));
}

#[test]
fn the_history_stops_at_the_scrollback_setting() {
    let terminal = terminal(100);
    feed_lines(&terminal, 0..300);
    assert_eq!(grid_history(&terminal), LIVE);
    assert_eq!(terminal.scroll_info().0, 100 + ROWS);
    terminal.scroll_to(usize::MAX);
    assert_eq!(visible_rows(&terminal)[0], "line 196");
}

#[test]
fn scrolling_reaches_the_oldest_archived_row() {
    let terminal = filled();
    terminal.scroll_to(usize::MAX);
    assert_eq!(terminal.display_offset(), HISTORY);
    assert_eq!(
        visible_rows(&terminal),
        ["line 0", "line 1", "line 2", "line 3", "line 4"]
    );

    terminal.scroll_to(100);
    assert_eq!(visible_rows(&terminal)[0], "line 196");

    // Crossing from the archive into the grid row by row.
    terminal.scroll_to(LIVE + 2);
    assert_eq!(
        visible_rows(&terminal)[..3],
        ["line 274", "line 275", "line 276"]
    );
    terminal.scroll_down(3);
    assert_eq!(terminal.display_offset(), LIVE - 1);
    assert_eq!(visible_rows(&terminal)[0], "line 277");

    terminal.scroll_to_bottom();
    assert_eq!(terminal.display_offset(), 0);
    assert_eq!(visible_rows(&terminal)[0], "line 296");
}

#[test]
fn output_keeps_a_view_scrolled_into_the_archive_on_its_rows() {
    let terminal = filled();
    terminal.scroll_to(250);
    assert_eq!(visible_rows(&terminal)[0], "line 46");

    feed_lines(&terminal, 300..400);
    assert_eq!(terminal.display_offset(), 350);
    assert_eq!(visible_rows(&terminal)[0], "line 46");
}

#[test]
fn search_finds_archived_lines() {
    let terminal = filled();
    assert_eq!(
        // A row's text runs to its last column, blanks included.
        terminal.search_grid("^line 7 ", true, true),
        vec![(7 - HISTORY as i32, 0, 7)]
    );
    // Every line matches once, archive, grid and screen alike.
    assert_eq!(terminal.search_grid("line", true, false).len(), 300);
}

#[test]
fn a_selection_reads_across_the_archive_and_the_grid() {
    let terminal = filled();
    // Content 270 at the top: archived up to 275, grid from 276.
    terminal.scroll_to(HISTORY - 270);
    terminal.start_selection(0, 0, Side::Left);
    terminal.update_selection(COLS - 1, 10, Side::Right);
    terminal.end_selection();
    assert!(terminal.has_selection());
    assert_eq!(
        terminal.get_selected_text().as_deref(),
        Some(&*lines(270..=280))
    );
    assert_eq!(
        terminal.selection_bounds(),
        Some(((0, 270 - HISTORY as i32), (COLS - 1, 280 - HISTORY as i32)))
    );

    // Output moves the selection with its text.
    feed_lines(&terminal, 300..310);
    assert_eq!(
        terminal.get_selected_text().as_deref(),
        Some(&*lines(270..=280))
    );
    assert_eq!(
        terminal.selection_bounds(),
        Some(((0, 260 - HISTORY as i32), (COLS - 1, 270 - HISTORY as i32)))
    );

    terminal.clear_selection();
    assert!(!terminal.has_selection());
    assert_eq!(terminal.get_selected_text(), None);
}

#[test]
fn a_grid_selection_keeps_its_text_when_drained_into_the_archive() {
    let terminal = filled();
    // Content 290..=292, in the grid: alacritty resolves it.
    terminal.scroll_to(HISTORY - 290);
    terminal.start_selection(0, 0, Side::Left);
    terminal.update_selection(COLS - 1, 2, Side::Right);
    terminal.end_selection();
    let in_grid = terminal.get_selected_text();
    assert_eq!(in_grid.as_deref(), Some(&*lines(290..=292)));

    feed_lines(&terminal, 300..400);
    assert_eq!(terminal.get_selected_text(), in_grid);
}

#[test]
fn erased_selected_text_drops_the_selection() {
    let terminal = filled();
    terminal.start_selection(0, 0, Side::Left);
    terminal.update_selection(3, 0, Side::Right);
    terminal.end_selection();
    assert!(terminal.has_selection());
    terminal.process_output(b"\x1b[H\x1b[2K");
    assert!(!terminal.has_selection());
}

/// Each kind of selection gives the same text over archived rows as alacritty
/// gives over the same rows in its grid.
#[test]
fn archived_selections_read_like_alacrittys() {
    type Select = fn(&Terminal);
    let selections: [(&str, Select); 4] = [
        ("word", |t| t.start_word_selection(7, 0)),
        ("line", |t| t.start_line_selection(3, 1)),
        ("simple", |t| {
            t.start_selection(6, 0, Side::Right);
            t.update_selection(4, 2, Side::Left);
        }),
        ("semantic drag", |t| {
            t.start_word_selection(12, 0);
            t.update_selection(2, 1, Side::Left);
        }),
    ];
    let content = concat!(
        "alpha beta,gamma\r\n",
        "a wrapped line longer than twenty columns\r\n",
        "\u{65e5}\u{672c} tab\there\r\n",
    );
    for (name, select) in selections {
        let terminal = terminal(1_000);
        terminal.process_output(content.as_bytes());
        // Content 0 at the top of the screen, all of it in the grid.
        terminal.scroll_to(usize::MAX);
        select(&terminal);
        terminal.end_selection();
        let in_grid = terminal.get_selected_text();
        assert!(in_grid.is_some(), "{name}: nothing selected in the grid");

        terminal.clear_selection();
        terminal.scroll_to_bottom();
        feed_lines(&terminal, 0..100);
        // The selected rows are archived now.
        assert!(grid_history(&terminal) + 10 < terminal.scroll_info().0 - ROWS);
        terminal.scroll_to(usize::MAX);
        select(&terminal);
        terminal.end_selection();
        assert_eq!(terminal.get_selected_text(), in_grid, "{name}");
    }
}

#[test]
fn select_all_covers_the_archive() {
    let terminal = filled();
    terminal.select_all();
    assert_eq!(
        terminal.get_selected_text().as_deref(),
        Some(&*format!("{}\n", lines(0..=299)))
    );
}

#[test]
fn the_history_text_runs_from_the_oldest_archived_row() {
    let terminal = filled();
    assert_eq!(terminal.history_text(), lines(0..=299));
    assert_eq!(terminal.tail_text(3), lines(297..=299));
    assert_eq!(terminal.tail_text(290), lines(10..=299));
}

#[test]
fn a_soft_wrapped_archived_line_exports_as_one_line() {
    let terminal = terminal(1_000);
    let long = "x".repeat(COLS + 5);
    terminal.process_output(format!("{long}\r\n").as_bytes());
    feed_lines(&terminal, 0..100);
    assert_eq!(
        terminal.history_text(),
        format!("{long}\n{}", lines(0..=99))
    );
}

#[test]
fn a_snapshot_replays_archived_rows_to_a_new_client() {
    let source = filled();
    let mirror = Terminal::new(
        "mirror".into(),
        TerminalSize {
            cols: COLS as u16,
            rows: ROWS as u16,
            cell_width: 8.0,
            cell_height: 16.0,
        },
        Arc::new(NullTransport),
        "/tmp".into(),
    );
    mirror.process_output(&source.render_snapshot());
    // The newest 200 history rows (content 96..=295, most of them archived)
    // and the screen.
    let text = mirror.history_text();
    assert!(text.contains(&lines(96..=299)), "{text}");
    assert!(!text.contains("line 95\n"), "{text}");
}

#[test]
fn erasing_the_saved_lines_drops_the_archive() {
    let terminal = filled();
    terminal.process_output(b"\x1b[3J");
    assert_eq!(terminal.scroll_info().0, ROWS);
    assert_eq!(terminal.history_text(), lines(296..=299));
}

#[test]
fn a_reset_drops_the_archive() {
    let terminal = filled();
    terminal.process_output(b"\x1bc");
    assert_eq!(terminal.scroll_info().0, ROWS);
    assert_eq!(terminal.history_text(), "");
}

#[test]
fn the_alternate_screen_hides_the_archive_and_keeps_it() {
    let terminal = filled();
    terminal.process_output(b"\x1b[?1049h");
    assert_eq!(terminal.scroll_info().0, ROWS);
    terminal.scroll_to(100);
    assert_eq!(terminal.display_offset(), 0);
    // An erase of the saved lines in the alternate screen leaves the primary's.
    terminal.process_output(b"\x1b[3J");
    terminal.process_output(b"\x1b[?1049l");
    assert_eq!(terminal.scroll_info().0, HISTORY + ROWS);
    terminal.scroll_to(usize::MAX);
    assert_eq!(visible_rows(&terminal)[0], "line 0");
}

#[test]
fn prompt_marks_follow_their_rows_into_the_archive() {
    let terminal = terminal(1_000);
    terminal.process_output(b"\x1b]133;A\x1b\\$ first\r\n");
    feed_lines(&terminal, 0..100);
    terminal.process_output(b"\x1b]133;A\x1b\\$ second\r\n");
    feed_lines(&terminal, 100..110);

    assert!(terminal.jump_to_prompt_above());
    assert_eq!(visible_rows(&terminal)[0], "$ second");
    assert!(terminal.jump_to_prompt_above());
    assert_eq!(visible_rows(&terminal)[0], "$ first");
    assert!(!terminal.jump_to_prompt_above());
}

#[test]
fn shrinking_the_scrollback_drops_the_oldest_archived_rows() {
    let terminal = filled();
    terminal.set_scrollback_lines(100);
    assert_eq!(terminal.scroll_info().0, 100 + ROWS);
    assert_eq!(grid_history(&terminal), LIVE);
    terminal.scroll_to(usize::MAX);
    assert_eq!(visible_rows(&terminal)[0], "line 196");

    // Below the live rows the grid alone holds the history.
    terminal.set_scrollback_lines(10);
    assert_eq!(terminal.scroll_info().0, 10 + ROWS);
    terminal.set_scrollback_lines(1_000);
    feed_lines(&terminal, 300..600);
    assert_eq!(grid_history(&terminal), LIVE);
    assert_eq!(terminal.scroll_info().0, 310 + ROWS);
}

fn resize_cols(terminal: &Terminal, cols: u16) {
    terminal.resize(TerminalSize {
        cols,
        rows: ROWS as u16,
        cell_width: 8.0,
        cell_height: 16.0,
    });
}

#[test]
fn a_narrower_screen_reflows_archived_rows() {
    let terminal = filled();
    resize_cols(&terminal, 6);
    assert_eq!(grid_history(&terminal), LIVE);
    terminal.scroll_to(usize::MAX);
    // A viewport in the archive reflows once the width has held.
    assert!(terminal.settle_scrolled_history());
    terminal.scrollback.lock().expire_reflow_wait();
    assert!(!terminal.settle_scrolled_history());
    // `line 10` and up wrap onto a second row, as the grid's rows do.
    assert_eq!(
        visible_rows(&terminal),
        ["line 0", "line 1", "line 2", "line 3", "line 4"]
    );
    terminal.scroll_to(terminal.display_offset() - 10);
    assert_eq!(visible_rows(&terminal)[..3], ["line 1", "0", "line 1"]);
    assert_eq!(terminal.history_text(), lines(0..=299));
}

#[test]
fn archived_rows_read_whole_after_narrowing_and_widening_again() {
    let terminal = filled();
    resize_cols(&terminal, 6);
    feed_lines(&terminal, 300..400);
    resize_cols(&terminal, COLS as u16);

    terminal.select_all();
    assert_eq!(
        terminal.get_selected_text().as_deref(),
        Some(&*format!("{}\n", lines(0..=399)))
    );
    assert_eq!(terminal.search_grid("line 310", true, false).len(), 1);
    assert_eq!(terminal.history_text(), lines(0..=399));
}

#[test]
fn a_resize_drag_does_not_reflow_the_archive_on_every_frame() {
    let terminal = filled();
    terminal.scroll_to(usize::MAX);
    for cols in [19, 18, 17, 6] {
        resize_cols(&terminal, cols);
        // What a frame of the drag does.
        assert!(terminal.settle_scrolled_history(), "{cols}");
        visible_rows(&terminal);
    }
    assert!(terminal.scrollback.lock().reflow_pending());
    // The grid reflowed on its own; archived `line 10`..`line 275` did not.
    let rows = terminal.scroll_info().0;
    terminal.scrollback.lock().expire_reflow_wait();
    assert!(!terminal.settle_scrolled_history());
    assert_eq!(terminal.scroll_info().0, rows + 266);
}

#[test]
fn a_line_between_archive_and_grid_reflows_whole() {
    // 50 distinct chars, so a row's text occurs once.
    let long = "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMN";
    for cols in [12, 30] {
        let terminal = terminal(1_000);
        feed_lines(&terminal, 0..100);
        terminal.process_output(format!("{long}\r\n").as_bytes());
        // 19 history rows and 4 screen rows below the line: its first two
        // rows are archived, its last is the grid's oldest.
        feed_lines(&terminal, 100..123);
        assert_eq!(
            terminal.with_content(|term| term.grid()[term.grid().topmost_line()]
                .into_iter()
                .map(|cell| cell.c)
                .collect::<String>()
                .trim_end()
                .to_owned()),
            &long[40..]
        );

        resize_cols(&terminal, cols);
        let expected = format!("{}\n{long}\n{}", lines(0..=99), lines(100..=122));
        assert_eq!(terminal.history_text(), expected, "{cols}");
        terminal.select_all();
        assert_eq!(
            terminal.get_selected_text().as_deref(),
            Some(&*format!("{expected}\n")),
            "{cols}"
        );
        // Search matches within a row; the row holding char 40, where the
        // grid's part of the line began, reads whole.
        let cols = usize::from(cols);
        let start = 40 / cols * cols;
        let row = &long[start..(start + cols).min(long.len())];
        assert_eq!(terminal.search_grid(row, true, false).len(), 1, "{cols}");
    }
}

#[test]
fn the_archive_reflows_when_a_read_needs_it() {
    let terminal = filled();
    resize_cols(&terminal, 6);
    // A resize at the bottom of the terminal leaves the archive alone.
    let rows_at_resize = terminal.scroll_info().0;
    // A read that needs every row reflows it: archived `line 10`..`line 275`
    // take two rows each at 6 columns.
    assert_eq!(terminal.history_text(), lines(0..=299));
    assert_eq!(terminal.scroll_info().0, rows_at_resize + 266);
}

#[test]
fn a_synchronized_update_drains_like_plain_output() {
    let output: String = (0..300).map(|i| format!("line {i}\r\n")).collect();
    for split in [None, Some(1_000)] {
        let terminal = terminal(1_000);
        let bytes = format!("\x1b[?2026h{output}\x1b[?2026l");
        match split {
            None => terminal.process_output(bytes.as_bytes()),
            Some(at) => {
                terminal.process_output(&bytes.as_bytes()[..at]);
                terminal.process_output(&bytes.as_bytes()[at..]);
            }
        }
        assert_eq!(grid_history(&terminal), LIVE, "{split:?}");
        assert_eq!(terminal.history_text(), lines(0..=299), "{split:?}");
    }
}

#[test]
fn a_burst_past_the_grid_limit_keeps_the_newest_rows() {
    // 30 lines of history: 20 live, room for 10 more before alacritty drops.
    let terminal = terminal(30);
    feed_lines(&terminal, 0..10);
    feed_lines(&terminal, 10..200);
    assert_eq!(terminal.scroll_info().0, 30 + ROWS);
    terminal.scroll_to(usize::MAX);
    assert_eq!(visible_rows(&terminal)[0], "line 166");
    assert_eq!(terminal.history_text(), lines(166..=199));
}

#[test]
fn an_erase_of_the_saved_lines_with_a_subparameter_drops_the_archive() {
    let terminal = filled();
    terminal.process_output(b"\x1b[3:1J");
    assert_eq!(terminal.scroll_info().0, ROWS);
}

#[test]
fn a_selection_a_scroll_region_moves_stays_alacrittys() {
    let terminal = filled();
    // Rows 1-3 scroll; row 4 stays.
    terminal.process_output(b"\x1b[1;4r\x1b[4;1H");
    terminal.start_selection(0, 1, Side::Left);
    terminal.update_selection(3, 4, Side::Right);
    terminal.end_selection();
    terminal.process_output(b"a\r\nb\r\n");
    let alacritty = terminal.with_content(|term| term.selection_to_string());
    assert!(alacritty.is_some());
    assert_eq!(terminal.get_selected_text(), alacritty);
}
