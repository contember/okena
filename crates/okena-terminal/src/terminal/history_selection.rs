//! A selection resolved over the whole history, for one reaching into archived
//! rows where alacritty's own cannot follow. Ports alacritty 0.25's range and
//! text rules (`Selection::to_range`, `Term::selection_to_string`) onto
//! [`HistoryView`] rows. Two differences: a semantic selection does not jump to
//! a matching bracket, and tab stops are taken as every 8 columns.

use std::borrow::Cow;
use std::ops::Range;

use alacritty_terminal::grid::Row;
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{SelectionRange, SelectionType};
use alacritty_terminal::term::cell::{Cell, Flags, LineLength};

use super::scrollback::{HistorySelection, HistoryView, SelectionPoint};

/// alacritty's initial tab stops.
const TAB_WIDTH: usize = 8;

fn is_wide_spacer(cell: &Cell) -> bool {
    cell.flags
        .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
}

/// Reads cells of one view, decoding each archived row once while the reads
/// stay on it.
struct Rows<'v, 'a, L> {
    view: &'v HistoryView<'a, L>,
    current: Option<(Line, Cow<'a, Row<Cell>>)>,
}

impl<'v, 'a, L> Rows<'v, 'a, L> {
    fn new(view: &'v HistoryView<'a, L>) -> Self {
        Self {
            view,
            current: None,
        }
    }

    fn row(&mut self, line: Line) -> &Row<Cell> {
        if self.current.as_ref().is_some_and(|(at, _)| *at != line) {
            self.current = None;
        }
        let view = self.view;
        &self.current.get_or_insert_with(|| (line, view.row(line))).1
    }

    fn cell(&mut self, point: Point) -> &Cell {
        &self.row(point.line)[point.column]
    }
}

impl<L> HistoryView<'_, L> {
    fn last_column(&self) -> Column {
        Column(self.columns() - 1)
    }

    /// The cell after `point`, row by row; `None` past the bottom right.
    fn next_point(&self, point: Point) -> Option<Point> {
        let last_column = self.last_column();
        if point >= Point::new(self.bottommost_line(), last_column) {
            return None;
        }
        Some(if point.column >= last_column {
            Point::new(point.line + 1, Column(0))
        } else {
            Point::new(point.line, point.column + 1)
        })
    }

    /// The cell before `point`; `None` at the top left of the history.
    fn prev_point(&self, point: Point) -> Option<Point> {
        if point <= Point::new(self.topmost_line(), Column(0)) {
            return None;
        }
        Some(if point.column == Column(0) {
            Point::new(point.line - 1, self.last_column())
        } else {
            Point::new(point.line, point.column - 1)
        })
    }

    fn clamp(&self, point: Point) -> Point {
        let last_column = self.last_column();
        if point.line < self.topmost_line() {
            Point::new(self.topmost_line(), Column(0))
        } else if point.line > self.bottommost_line() {
            Point::new(self.bottommost_line(), last_column)
        } else {
            Point::new(point.line, point.column.min(last_column))
        }
    }

    /// The selected cells: start and end inclusive, `None` for an empty or
    /// scrolled-off selection.
    pub(super) fn selection_range(&self, selection: &HistorySelection) -> Option<SelectionRange> {
        let (mut start, mut end) = (selection.anchor, selection.head);
        if start.point > end.point {
            std::mem::swap(&mut start, &mut end);
        }
        if end.point.line < self.topmost_line() {
            return None;
        }
        start.point = self.clamp(start.point);
        end.point = self.clamp(end.point);

        match selection.ty {
            SelectionType::Simple => self.range_simple(selection, start, end),
            SelectionType::Block => range_block(selection, start, end),
            SelectionType::Semantic => Some(SelectionRange {
                start: self.semantic_search_left(start.point),
                end: self.semantic_search_right(end.point),
                is_block: false,
            }),
            SelectionType::Lines => Some(SelectionRange {
                start: self.line_search_left(start.point),
                end: self.line_search_right(end.point),
                is_block: false,
            }),
        }
    }

    fn range_simple(
        &self,
        selection: &HistorySelection,
        mut start: SelectionPoint,
        mut end: SelectionPoint,
    ) -> Option<SelectionRange> {
        if is_empty(selection) {
            return None;
        }
        // Drop the last cell when the selection ends left of it.
        if end.side == Side::Left && start.point != end.point {
            if end.point.column == Column(0) {
                end.point.column = self.last_column();
                end.point.line -= 1;
            } else {
                end.point.column -= 1;
            }
        }
        // Drop the first cell when the selection starts right of it.
        if start.side == Side::Right && start.point != end.point {
            start.point.column += 1;
            if start.point.column == Column(self.columns()) {
                start.point.column = Column(0);
                start.point.line += 1;
            }
        }
        Some(SelectionRange {
            start: start.point,
            end: end.point,
            is_block: false,
        })
    }

    fn semantic_search_left(&self, point: Point) -> Point {
        match self.inline_search_left(point) {
            // Step back right of the separator, over wide char spacers.
            Ok(found) => {
                let mut rows = Rows::new(self);
                let mut at = found;
                while let Some(next) = self.next_point(at) {
                    at = next;
                    if !is_wide_spacer(rows.cell(next)) {
                        return next;
                    }
                }
                found
            }
            Err(point) => point,
        }
    }

    fn semantic_search_right(&self, point: Point) -> Point {
        match self.inline_search_right(point) {
            Ok(found) => self.prev_point(found).unwrap_or(found),
            Err(point) => point,
        }
    }

    /// To the left of `point` within its wrapped line, the nearest semantic
    /// separator (`Ok`), or the line's first cell (`Err`).
    fn inline_search_left(&self, mut point: Point) -> Result<Point, Point> {
        point.line = point.line.max(self.topmost_line());
        let separators = self.term().semantic_escape_chars();
        let last_column = self.last_column();
        let mut rows = Rows::new(self);
        let mut at = point;
        while let Some(prev) = self.prev_point(at) {
            at = prev;
            let cell = rows.cell(prev);
            if prev.column == last_column && !cell.flags.contains(Flags::WRAPLINE) {
                break;
            }
            point = prev;
            if !is_wide_spacer(cell) && separators.contains(cell.c) {
                return Ok(point);
            }
        }
        Err(point)
    }

    /// To the right of `point` within its wrapped line, the nearest semantic
    /// separator (`Ok`), or the line's last cell (`Err`).
    fn inline_search_right(&self, mut point: Point) -> Result<Point, Point> {
        point.line = point.line.max(self.topmost_line());
        let separators = self.term().semantic_escape_chars();
        let last_column = self.last_column();
        let mut rows = Rows::new(self);
        if point.column == last_column && !rows.cell(point).flags.contains(Flags::WRAPLINE) {
            return Err(point);
        }
        let mut at = point;
        while let Some(next) = self.next_point(at) {
            at = next;
            point = next;
            let cell = rows.cell(next);
            if !is_wide_spacer(cell) && separators.contains(cell.c) {
                return Ok(point);
            }
            if point.column == last_column && !cell.flags.contains(Flags::WRAPLINE) {
                break;
            }
        }
        Err(point)
    }

    /// The first cell of `point`'s line, soft wraps followed upwards.
    fn line_search_left(&self, mut point: Point) -> Point {
        let last_column = self.last_column();
        let mut rows = Rows::new(self);
        while point.line > self.topmost_line()
            && rows
                .cell(Point::new(point.line - 1, last_column))
                .flags
                .contains(Flags::WRAPLINE)
        {
            point.line -= 1;
        }
        point.column = Column(0);
        point
    }

    /// The last cell of `point`'s line, soft wraps followed downwards.
    fn line_search_right(&self, mut point: Point) -> Point {
        let last_column = self.last_column();
        let mut rows = Rows::new(self);
        while point.line + 1 < self.screen_lines() as i32
            && rows
                .cell(Point::new(point.line, last_column))
                .flags
                .contains(Flags::WRAPLINE)
        {
            point.line += 1;
        }
        point.column = last_column;
        point
    }

    /// The selected text, as alacritty's `selection_to_string` builds it.
    pub(super) fn selection_text(&self, selection: &HistorySelection) -> Option<String> {
        let SelectionRange { start, end, .. } = self.selection_range(selection)?;
        let text = match selection.ty {
            SelectionType::Block => {
                let mut text = String::new();
                for line in start.line.0..end.line.0 {
                    let cols = start.column..end.column;
                    text += self
                        .line_to_string(Line(line), cols, start.column.0 != 0)
                        .trim_end();
                    text += "\n";
                }
                text += self
                    .line_to_string(end.line, start.column..end.column, true)
                    .trim_end();
                text
            }
            SelectionType::Lines => self.bounds_to_string(start, end) + "\n",
            SelectionType::Simple | SelectionType::Semantic => self.bounds_to_string(start, end),
        };
        Some(text)
    }

    fn bounds_to_string(&self, start: Point, end: Point) -> String {
        let mut text = String::new();
        for line in (start.line.0..=end.line.0).map(Line) {
            let start_col = if line == start.line {
                start.column
            } else {
                Column(0)
            };
            let end_col = if line == end.line {
                end.column
            } else {
                self.last_column()
            };
            text += &self.line_to_string(line, start_col..end_col, line == end.line);
        }
        text.strip_suffix('\n').map(str::to_owned).unwrap_or(text)
    }

    fn line_to_string(
        &self,
        line: Line,
        mut cols: Range<Column>,
        include_wrapped_wide: bool,
    ) -> String {
        let row = self.row(line);
        let mut text = String::new();
        let line_length = row.line_length().min(cols.end + 1);

        // Include a wide char whose trailing spacer is selected.
        if cols.start.0 > 0 && row[cols.start].flags.contains(Flags::WIDE_CHAR_SPACER) {
            cols.start -= 1;
        }

        let mut tab_mode = false;
        for column in (cols.start.0..line_length.0).map(Column) {
            let cell = &row[column];
            // Skip the blanks a tab left up to the next tab stop.
            if tab_mode {
                if column.0 % TAB_WIDTH == 0 || cell.c != ' ' {
                    tab_mode = false;
                } else {
                    continue;
                }
            }
            if cell.c == '\t' {
                tab_mode = true;
            }
            if !is_wide_spacer(cell) {
                text.push(cell.c);
                text.extend(cell.zerowidth().into_iter().flatten());
            }
        }

        if cols.end >= self.last_column()
            && (line_length.0 == 0 || !row[line_length - 1].flags.contains(Flags::WRAPLINE))
        {
            text.push('\n');
        }

        // A wide char that wrapped to the next line, with its leading spacer
        // selected (alacritty reads it from the line above).
        if line_length == Column(self.columns())
            && line_length.0 >= 2
            && row[line_length - 1]
                .flags
                .contains(Flags::LEADING_WIDE_CHAR_SPACER)
            && include_wrapped_wide
        {
            text.push(self.row(line - 1)[Column(0)].c);
        }

        text
    }
}

/// alacritty's `Selection::is_empty`, over the unordered ends.
fn is_empty(selection: &HistorySelection) -> bool {
    let (start, end) = (selection.anchor, selection.head);
    match selection.ty {
        SelectionType::Simple => {
            let (start, end) = if start.point > end.point {
                (end, start)
            } else {
                (start, end)
            };
            start == end
                || (start.side == Side::Right
                    && end.side == Side::Left
                    && start.point.line == end.point.line
                    && start.point.column + 1 == end.point.column)
        }
        SelectionType::Block => {
            (start.point.column == end.point.column && start.side == end.side)
                || (start.point.column + 1 == end.point.column
                    && start.side == Side::Right
                    && end.side == Side::Left)
                || (end.point.column + 1 == start.point.column
                    && start.side == Side::Left
                    && end.side == Side::Right)
        }
        SelectionType::Semantic | SelectionType::Lines => false,
    }
}

fn range_block(
    selection: &HistorySelection,
    mut start: SelectionPoint,
    mut end: SelectionPoint,
) -> Option<SelectionRange> {
    if is_empty(selection) {
        return None;
    }
    // Always top left to bottom right.
    if start.point.column > end.point.column {
        std::mem::swap(&mut start.side, &mut end.side);
        std::mem::swap(&mut start.point.column, &mut end.point.column);
    }
    if end.side == Side::Left && start.point != end.point && end.point.column.0 > 0 {
        end.point.column -= 1;
    }
    if start.side == Side::Right && start.point != end.point {
        start.point.column += 1;
    }
    Some(SelectionRange {
        start: start.point,
        end: end.point,
        is_block: true,
    })
}

#[cfg(test)]
mod tests {
    use super::super::scrollback::{HistorySelection, HistoryView, Scrollback, SelectionPoint};
    use alacritty_terminal::event::{Event, EventListener};
    use alacritty_terminal::index::{Column, Line, Point, Side};
    use alacritty_terminal::selection::SelectionType;
    use alacritty_terminal::term::test::TermSize;
    use alacritty_terminal::term::{Config, Term};
    use alacritty_terminal::vte::ansi::Processor;

    struct NoopListener;

    impl EventListener for NoopListener {
        fn send_event(&self, _event: Event) {}
    }

    /// The port gives alacritty's range and text for every kind of selection
    /// between many pairs of points, over rows with wraps, wide chars, tabs,
    /// marks and separators.
    #[test]
    fn ranges_and_text_match_alacrittys() {
        let mut term = Term::new(Config::default(), &TermSize::new(10, 6), NoopListener);
        let mut processor: Processor = Processor::new();
        processor.advance(
            &mut term,
            "ab,cd ef\r\na wrapped (line) here\r\n\u{65e5}\u{672c}x\tz e\u{0301}\r\nlast"
                .as_bytes(),
        );
        let scrollback = Scrollback::new(100, 100);
        let points: Vec<SelectionPoint> = [(0, 0), (0, 3), (1, 9), (2, 4), (3, 1), (3, 5), (4, 2)]
            .into_iter()
            .flat_map(|(line, col)| {
                [Side::Left, Side::Right].map(|side| SelectionPoint {
                    point: Point::new(Line(line), Column(col)),
                    side,
                })
            })
            .collect();
        let kinds = [
            SelectionType::Simple,
            SelectionType::Block,
            SelectionType::Semantic,
            SelectionType::Lines,
        ];
        for ty in kinds {
            for &anchor in &points {
                for &head in &points {
                    let selection = HistorySelection { ty, anchor, head };
                    term.selection = Some(selection.to_alacritty());
                    let view = HistoryView::new(&term, &scrollback);
                    let expected_range = term.selection.as_ref().and_then(|s| s.to_range(&term));
                    let range = view.selection_range(&selection);
                    assert_eq!(
                        range.map(|r| (r.start, r.end, r.is_block)),
                        expected_range.map(|r| (r.start, r.end, r.is_block)),
                        "{ty:?} {anchor:?} {head:?}"
                    );
                    assert_eq!(
                        view.selection_text(&selection),
                        term.selection_to_string(),
                        "{ty:?} {anchor:?} {head:?}"
                    );
                }
            }
        }
    }
}
