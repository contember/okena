//! The terminal's history as one sequence of rows: the newest rows live in
//! alacritty's grid, older ones in the compressed [`ScrollbackArchive`].
//!
//! Line numbers follow alacritty's: `0..screen_lines` is the screen, negative
//! lines are history, `-1` the newest history row. Archived rows continue
//! below the grid's topmost line, so a row keeps its line number when it moves
//! from the grid into the archive.

use std::borrow::Cow;
use std::time::{Duration, Instant};

use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::{Dimensions, Row, Scroll};
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::cell::Cell;
use alacritty_terminal::term::{Term, TermMode};

use super::archive::ScrollbackArchive;

/// History rows kept in alacritty's grid; older rows move to the archive.
/// Every grid row costs `columns x 24 B` whatever it holds, and the grid keeps
/// up to 1 000 more allocated as spare.
pub const LIVE_HISTORY_LINES: usize = 1_000;

/// How long the columns must hold before the archive reflows to them.
const REFLOW_SETTLE: Duration = Duration::from_millis(300);

/// A selection endpoint on the history-wide line numbering.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct SelectionPoint {
    pub(super) point: Point,
    pub(super) side: Side,
}

/// The user's selection, kept on the history-wide line numbering. While both
/// ends sit in the grid, alacritty's own `term.selection` mirrors it and does
/// the work (it also drops a selection whose text gets erased); once an end
/// reaches the archive, the selection is resolved here instead.
#[derive(Clone, Copy, Debug)]
pub(super) struct HistorySelection {
    pub(super) ty: SelectionType,
    pub(super) anchor: SelectionPoint,
    pub(super) head: SelectionPoint,
}

impl HistorySelection {
    /// alacritty's selection over the same ends.
    pub(super) fn to_alacritty(self) -> Selection {
        let mut selection = Selection::new(self.ty, self.anchor.point, self.anchor.side);
        selection.update(self.head.point, self.head.side);
        selection
    }

    fn top_line(&self) -> Line {
        self.anchor.point.line.min(self.head.point.line)
    }

    fn bottom_line(&self) -> Line {
        self.anchor.point.line.max(self.head.point.line)
    }

    /// Move both ends up by `rows`, saturating.
    fn shift_up(&mut self, rows: i64) {
        let rows = rows.clamp(i64::from(i32::MIN + 1), i64::from(i32::MAX)) as i32;
        for end in [&mut self.anchor, &mut self.head] {
            end.point.line = Line(end.point.line.0.saturating_sub(rows));
        }
    }
}

pub(super) struct Scrollback {
    pub(super) archive: ScrollbackArchive,
    /// Rows the viewport sits above the grid's topmost history row. Only set
    /// while alacritty's own display offset is at the top of the grid.
    pub(super) archive_offset: usize,
    /// History rows the grid keeps before moving older ones to the archive.
    live_lines: usize,
    pub(super) selection: Option<HistorySelection>,
    /// When the columns last changed while archived rows were laid out at
    /// another width; `None` once the archive matches the grid.
    reflow_requested: Option<Instant>,
}

impl Scrollback {
    /// History of `total_lines`, the newest `live_lines` of them in the grid.
    pub(super) fn new(total_lines: usize, live_lines: usize) -> Self {
        let live_lines = live_lines.min(total_lines);
        Self {
            archive: ScrollbackArchive::new(total_lines - live_lines),
            archive_offset: 0,
            live_lines,
            selection: None,
            reflow_requested: None,
        }
    }

    pub(super) fn set_total_lines(&mut self, total_lines: usize, live_cap: usize) {
        self.live_lines = live_cap.min(total_lines);
        self.archive.set_capacity(total_lines - self.live_lines);
        self.archive_offset = self.archive_offset.min(self.archive.len());
    }

    /// The columns changed: lay the archive out at the new width once they
    /// settle. Reflowing a full archive takes tens of milliseconds, too long
    /// for every frame of a resize drag.
    pub(super) fn request_reflow(&mut self) {
        if self.archive.len() > 0 {
            self.reflow_requested = Some(Instant::now());
        }
    }

    /// Reflow the archive to the grid's width if a column change asked for it
    /// and the width has held for [`REFLOW_SETTLE`], or at once with `force`
    /// (a read that must see every row at the current width). Keeps a view
    /// scrolled into the archive at about the same place; an archived
    /// selection no longer points at its rows and is dropped.
    /// Returns whether the archive was reflowed.
    pub(super) fn settle<L>(&mut self, term: &Term<L>, force: bool) -> bool {
        let Some(requested) = self.reflow_requested else {
            return false;
        };
        if !force && requested.elapsed() < REFLOW_SETTLE {
            return false;
        }
        self.reflow_requested = None;
        let before = self.archive.len();
        self.archive.reflow(term.grid().columns());
        let after = self.archive.len();
        if before > 0 && self.archive_offset > 0 {
            let scaled = self.archive_offset as u128 * after as u128 / before as u128;
            self.archive_offset = usize::try_from(scaled).unwrap_or(after).min(after);
        }
        if self.archived_selection(term).is_some() {
            self.selection = None;
        }
        true
    }

    /// History rows seen by the viewport: none in the alternate screen, whose
    /// grid has no history and which hides the primary screen's.
    pub(super) fn history_size<L>(&self, term: &Term<L>) -> usize {
        if term.mode().contains(TermMode::ALT_SCREEN) {
            return 0;
        }
        term.grid().history_size() + self.archive.len()
    }

    /// How far the viewport is scrolled up from the bottom, archive included.
    pub(super) fn display_offset<L>(&self, term: &Term<L>) -> usize {
        let grid = term.grid();
        if self.archive_offset > 0
            && !term.mode().contains(TermMode::ALT_SCREEN)
            && grid.display_offset() == grid.history_size()
        {
            grid.display_offset() + self.archive_offset
        } else {
            grid.display_offset()
        }
    }

    /// Scroll so the viewport sits `offset` rows above the bottom.
    pub(super) fn set_display_offset<L: EventListener>(
        &mut self,
        term: &mut Term<L>,
        offset: usize,
    ) {
        let offset = offset.min(self.history_size(term));
        let grid_history = term.grid().history_size();
        let grid_offset = offset.min(grid_history);
        let delta = grid_offset as i64 - term.grid().display_offset() as i64;
        if delta != 0 {
            term.scroll_display(Scroll::Delta(delta as i32));
        }
        self.archive_offset = offset - grid_offset;
    }

    /// Move history rows past the live lines into the archive, keeping the
    /// viewport on the rows it showed. Called after every bit of parsed output
    /// and every resize, so the grid never holds more than the live lines plus
    /// what one step pushed.
    pub(super) fn drain<L: EventListener>(&mut self, term: &mut Term<L>) {
        self.settle(term, false);
        if term.mode().contains(TermMode::ALT_SCREEN) {
            return;
        }
        // alacritty moved the view on its own (a clear, a reset): the archive
        // part of the old offset no longer applies.
        if term.grid().display_offset() < term.grid().history_size() {
            self.archive_offset = 0;
        }
        let history = term.grid().history_size();
        if history <= self.live_lines {
            return;
        }
        let offset = self.display_offset(term);
        {
            let grid = term.grid_mut();
            for back in (self.live_lines + 1..=history).rev() {
                self.archive.push(&grid[Line(-(back as i32))]);
            }
            let limit = self.live_lines + self.archive.capacity();
            grid.update_history(self.live_lines);
            grid.update_history(limit);
        }
        self.set_display_offset(term, offset);
        if self
            .selection
            .is_some_and(|selection| !in_grid(&selection, term))
        {
            // An end moved into the archive, where alacritty cannot follow.
            term.selection = None;
        }
    }

    /// Drop the archived rows, as an erase of the saved lines does, and a
    /// selection reaching into them.
    pub(super) fn clear_archive<L>(&mut self, term: &Term<L>) {
        self.archive.clear();
        self.archive_offset = 0;
        self.reflow_requested = None;
        if self
            .selection
            .is_some_and(|selection| !in_grid(&selection, term))
        {
            self.selection = None;
        }
    }

    /// Replace the selection, mirroring it into alacritty while both ends sit
    /// in the grid.
    pub(super) fn set_selection<L>(
        &mut self,
        term: &mut Term<L>,
        selection: Option<HistorySelection>,
    ) {
        term.selection = selection
            .filter(|selection| in_grid(selection, term))
            .map(HistorySelection::to_alacritty);
        self.selection = selection;
    }

    /// Rows that entered the history since `before`, a
    /// [`Scrollback::history_mark`] taken earlier.
    pub(super) fn rows_entered<L>(&self, term: &Term<L>, before: HistoryMark) -> i64 {
        let now = self.history_mark(term);
        if before.alt_screen || now.alt_screen {
            return 0;
        }
        (now.grid_history + now.archived) as i64 - (before.grid_history + before.archived) as i64
    }

    pub(super) fn history_mark<L>(&self, term: &Term<L>) -> HistoryMark {
        HistoryMark {
            grid_history: term.grid().history_size() as u64,
            archived: self.archive.archived_total(),
            alt_screen: term.mode().contains(TermMode::ALT_SCREEN),
        }
    }

    /// The selection while an end sits in the archive, where alacritty's
    /// mirror cannot cover it.
    pub(super) fn archived_selection<L>(&self, term: &Term<L>) -> Option<HistorySelection> {
        self.selection.filter(|selection| !in_grid(selection, term))
    }

    /// Move the selection with its text after `rows` entered the history (a
    /// negative count when a taller screen pulled rows back out of it), as
    /// alacritty moved its mirror. Drops it once it scrolls off the top, or
    /// when alacritty dropped the mirror because its text was erased. A mirror
    /// alacritty moved differently (a scroll region not starting at the top)
    /// stays alacritty's alone.
    pub(super) fn follow_output<L>(&mut self, term: &Term<L>, rows: i64) {
        let Some(mut selection) = self.selection else {
            return;
        };
        selection.shift_up(rows);
        let topmost = -(self.history_size(term) as i64);
        if i64::from(selection.bottom_line().0) < topmost {
            self.selection = None;
            return;
        }
        let mirrored = in_grid(&selection, term);
        let mirror_lost = mirrored && term.selection.as_ref() != Some(&selection.to_alacritty());
        // The alternate screen hides the history the selection reaches into.
        let history_hidden = !mirrored && term.mode().contains(TermMode::ALT_SCREEN);
        if mirror_lost || history_hidden {
            self.selection = None;
            return;
        }
        self.selection = Some(selection);
    }
}

/// Both ends of `selection` sit in alacritty's grid.
fn in_grid<L>(selection: &HistorySelection, term: &Term<L>) -> bool {
    selection.top_line() >= term.grid().topmost_line()
}

#[derive(Clone, Copy)]
pub(super) struct HistoryMark {
    grid_history: u64,
    archived: u64,
    alt_screen: bool,
}

/// Read access to the whole history, grid and archive, for one locked moment.
pub struct HistoryView<'a, L> {
    term: &'a Term<L>,
    scrollback: &'a Scrollback,
}

impl<'a, L> HistoryView<'a, L> {
    pub(super) fn new(term: &'a Term<L>, scrollback: &'a Scrollback) -> Self {
        Self { term, scrollback }
    }

    pub fn term(&self) -> &'a Term<L> {
        self.term
    }

    pub fn screen_lines(&self) -> usize {
        self.term.grid().screen_lines()
    }

    pub fn columns(&self) -> usize {
        self.term.grid().columns()
    }

    /// History rows above the screen, grid and archive together.
    pub fn history_size(&self) -> usize {
        self.scrollback.history_size(self.term)
    }

    /// How far the viewport is scrolled up from the bottom.
    pub fn display_offset(&self) -> usize {
        self.scrollback.display_offset(self.term)
    }

    pub fn topmost_line(&self) -> Line {
        Line(-(self.history_size() as i32))
    }

    pub fn bottommost_line(&self) -> Line {
        Line(self.screen_lines() as i32 - 1)
    }

    /// The row at `line`, `columns()` wide. An archived row is decoded; a
    /// line outside the history reads as a blank row.
    pub fn row(&self, line: Line) -> Cow<'a, Row<Cell>> {
        let grid = self.term.grid();
        if line >= grid.topmost_line() && line <= grid.bottommost_line() {
            return Cow::Borrowed(&grid[line]);
        }
        let cols = grid.columns();
        let in_archive =
            line < grid.topmost_line() && !self.term.mode().contains(TermMode::ALT_SCREEN);
        let back = (-(line.0 as i64) - grid.history_size() as i64 - 1).max(0) as usize;
        let row = in_archive
            .then(|| self.scrollback.archive.row_from_newest(back, cols))
            .flatten();
        Cow::Owned(row.unwrap_or_else(|| Row::new(cols)))
    }

    /// The row shown at `visual_row` of the viewport, 0 at its top.
    pub fn visible_row(&self, visual_row: usize) -> Cow<'a, Row<Cell>> {
        self.row(Line(visual_row as i32 - self.display_offset() as i32))
    }

    /// The cell at `point`, blank outside the history.
    pub fn cell(&self, point: Point) -> Cell {
        let row = self.row(point.line);
        if point.column.0 < row.len() {
            row[point.column].clone()
        } else {
            Cell::default()
        }
    }

    /// Every archived row oldest first, at the width it was archived with,
    /// then the grid's history and screen rows.
    pub fn for_each_full_row(&self, mut f: impl FnMut(&Row<Cell>)) {
        if !self.term.mode().contains(TermMode::ALT_SCREEN) {
            self.scrollback.archive.for_each_row(&mut f);
        }
        let grid = self.term.grid();
        for line in grid.topmost_line().0..=grid.bottommost_line().0 {
            f(&grid[Line(line)]);
        }
    }

    /// The selection while an end sits in the archive, where alacritty's
    /// mirror cannot cover it.
    pub(super) fn archived_selection(&self) -> Option<HistorySelection> {
        self.scrollback.archived_selection(self.term)
    }
}

/// `true` when a soft wrap continues `row` onto the next line.
pub(super) fn wraps(row: &Row<Cell>) -> bool {
    row.len() > 0
        && row[Column(row.len() - 1)]
            .flags
            .contains(alacritty_terminal::term::cell::Flags::WRAPLINE)
}
