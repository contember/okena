//! Scrollback older than the live grid, kept compressed in memory.
//!
//! alacritty stores every history cell as a 24-byte `Cell`, so a full
//! 200-column row costs 4.8 KB whatever it holds. Rows that scroll past the
//! terminal's live history move here instead: encoded as text plus attribute
//! runs, then deflated in blocks. Nothing here ever touches the disk — terminal
//! output carries secrets.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::io::{Read, Write};

use alacritty_terminal::grid::{GridCell, Row};
use alacritty_terminal::index::Column;
use alacritty_terminal::term::cell::{Cell, Flags, Hyperlink};
use alacritty_terminal::vte::ansi::{Color, NamedColor, Rgb};
use flate2::Compression;
use flate2::read::DeflateDecoder;
use flate2::write::DeflateEncoder;

/// Rows per compressed block: large enough for deflate to find the repetition
/// across rows, small enough that showing one screen decompresses little.
const BLOCK_ROWS: usize = 256;

struct Block {
    /// Absolute index of the block's first row.
    first: u64,
    rows: usize,
    compressed: Box<[u8]>,
}

/// A block's rows, decompressed, with where each encoded row starts.
struct DecodedBlock {
    first: u64,
    bytes: Vec<u8>,
    offsets: Vec<usize>,
}

impl DecodedBlock {
    fn row(&self, abs: u64) -> Option<&[u8]> {
        let index = usize::try_from(abs.checked_sub(self.first)?).ok()?;
        let start = *self.offsets.get(index)?;
        let end = self
            .offsets
            .get(index + 1)
            .copied()
            .unwrap_or(self.bytes.len());
        self.bytes.get(start..end)
    }
}

/// Oldest-first store of archived rows, addressed by absolute row index: the
/// number of rows archived before it since the terminal started. Indices only
/// grow, so a dropped row's index is never reused.
pub(super) struct ScrollbackArchive {
    blocks: VecDeque<Block>,
    /// Encoded rows not yet sealed into a block.
    open: Vec<u8>,
    open_offsets: Vec<usize>,
    /// Absolute index of the oldest row still kept.
    front: u64,
    /// Absolute index the next archived row gets.
    end: u64,
    capacity: usize,
    /// The block the last lookup decompressed: a screen of rows usually sits
    /// in one or two blocks, so scrolling inflates each block once.
    cache: RefCell<Option<DecodedBlock>>,
}

impl ScrollbackArchive {
    pub(super) fn new(capacity: usize) -> Self {
        Self {
            blocks: VecDeque::new(),
            open: Vec::new(),
            open_offsets: Vec::new(),
            front: 0,
            end: 0,
            capacity,
            cache: RefCell::new(None),
        }
    }

    /// Rows held.
    pub(super) fn len(&self) -> usize {
        // `end - front` never exceeds `capacity`, a `usize`.
        usize::try_from(self.end - self.front).unwrap_or(usize::MAX)
    }

    /// Rows archived since the terminal started, including ones since dropped.
    pub(super) fn archived_total(&self) -> u64 {
        self.end
    }

    pub(super) fn capacity(&self) -> usize {
        self.capacity
    }

    /// Keep at most `capacity` rows, dropping the oldest.
    pub(super) fn set_capacity(&mut self, capacity: usize) {
        self.capacity = capacity;
        self.trim();
    }

    /// Append `row` as the newest archived row.
    pub(super) fn push(&mut self, row: &Row<Cell>) {
        self.end += 1;
        if self.capacity == 0 {
            self.front = self.end;
            return;
        }
        self.open_offsets.push(self.open.len());
        encode_row(row, &mut self.open);
        if self.open_offsets.len() == BLOCK_ROWS {
            self.seal();
        }
        self.trim();
    }

    /// Drop every row, as an erase of the saved lines does.
    pub(super) fn clear(&mut self) {
        self.blocks.clear();
        self.open.clear();
        self.open_offsets.clear();
        self.front = self.end;
        *self.cache.get_mut() = None;
    }

    /// Lay every archived line out `cols` wide, as alacritty reflows its grid
    /// on a column change: soft-wrapped rows join into their line and wrap
    /// again at the new width. Rows past the capacity drop, oldest first.
    pub(super) fn reflow(&mut self, cols: usize) {
        if cols == 0 || self.len() == 0 {
            return;
        }
        let mut out = ScrollbackArchive::new(self.capacity);
        // Rows archived later keep counting from here.
        out.front = self.end;
        out.end = self.end;
        let mut rewrap = Rewrap::new(cols);
        self.for_each_row(|row| {
            let len = row.len();
            let wrapped = len > 0 && row[Column(len - 1)].flags.contains(Flags::WRAPLINE);
            let mut end = len;
            if wrapped {
                // A wide char that did not fit left a spacer in the last cell.
                if row[Column(len - 1)]
                    .flags
                    .contains(Flags::LEADING_WIDE_CHAR_SPACER)
                {
                    end -= 1;
                }
            } else {
                while end > 0 && row[Column(end - 1)].is_empty() {
                    end -= 1;
                }
            }
            for col in 0..end {
                let mut cell = row[Column(col)].clone();
                cell.flags.remove(Flags::WRAPLINE);
                rewrap.put(cell, &mut out);
            }
            if !wrapped {
                rewrap.end_line(&mut out);
            }
        });
        // The newest archived line may go on in the grid.
        rewrap.end_wrapped(&mut out);
        *self = out;
    }

    /// The row `back` rows above the newest archived row (0 = newest), laid
    /// out `cols` wide.
    pub(super) fn row_from_newest(&self, back: usize, cols: usize) -> Option<Row<Cell>> {
        let abs = self.end.checked_sub(1)?.checked_sub(back as u64)?;
        if abs < self.front {
            return None;
        }
        self.with_encoded(abs, |bytes| decode_row(bytes, Some(cols)))
    }

    /// The newest archived row soft-wraps into the next one.
    pub(super) fn newest_wraps(&self) -> bool {
        let Some(newest) = self.end.checked_sub(1).filter(|abs| *abs >= self.front) else {
            return false;
        };
        self.with_encoded(newest, |bytes| {
            let row = decode_row(bytes, None);
            let len = row.len();
            len > 0 && row[Column(len - 1)].flags.contains(Flags::WRAPLINE)
        })
        .unwrap_or(false)
    }

    /// Every row oldest first, each at the width it was archived with.
    pub(super) fn for_each_row(&self, mut f: impl FnMut(&Row<Cell>)) {
        for block in &self.blocks {
            let decoded = inflate(block);
            for abs in block.first.max(self.front)..block.first + block.rows as u64 {
                if let Some(row) = decoded.row(abs).map(|bytes| decode_row(bytes, None)) {
                    f(&row);
                }
            }
        }
        let open_first = self.end - self.open_offsets.len() as u64;
        for (index, start) in self.open_offsets.iter().enumerate() {
            let abs = open_first + index as u64;
            if abs < self.front {
                continue;
            }
            let end = self
                .open_offsets
                .get(index + 1)
                .copied()
                .unwrap_or(self.open.len());
            f(&decode_row(&self.open[*start..end], None));
        }
    }

    /// Bytes this archive holds on the heap.
    #[cfg(test)]
    pub(super) fn heap_bytes(&self) -> usize {
        let blocks: usize = self.blocks.iter().map(|b| b.compressed.len()).sum();
        let cache = self.cache.borrow().as_ref().map_or(0, |c| {
            c.bytes.capacity() + c.offsets.capacity() * size_of::<usize>()
        });
        blocks + self.open.capacity() + self.open_offsets.capacity() * size_of::<usize>() + cache
    }

    fn with_encoded<R>(&self, abs: u64, f: impl FnOnce(&[u8]) -> R) -> Option<R> {
        let open_first = self.end - self.open_offsets.len() as u64;
        if abs >= open_first {
            let index = usize::try_from(abs - open_first).ok()?;
            let start = *self.open_offsets.get(index)?;
            let end = self
                .open_offsets
                .get(index + 1)
                .copied()
                .unwrap_or(self.open.len());
            return Some(f(&self.open[start..end]));
        }
        let position = self
            .blocks
            .partition_point(|block| block.first + block.rows as u64 <= abs);
        let block = self.blocks.get(position)?;
        let mut cache = self.cache.borrow_mut();
        if cache
            .as_ref()
            .is_none_or(|cached| cached.first != block.first)
        {
            *cache = Some(inflate(block));
        }
        cache.as_ref()?.row(abs).map(f)
    }

    fn seal(&mut self) {
        let rows = self.open_offsets.len();
        if rows == 0 {
            return;
        }
        let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
        let mut header = Vec::with_capacity(rows * 2);
        for pair in self.open_offsets.windows(2) {
            write_varint(&mut header, (pair[1] - pair[0]) as u64);
        }
        write_varint(
            &mut header,
            (self.open.len() - self.open_offsets[rows - 1]) as u64,
        );
        // Writing into a `Vec` cannot fail.
        let compressed = encoder
            .write_all(&header)
            .and_then(|()| encoder.write_all(&self.open))
            .and_then(|()| encoder.finish())
            .unwrap_or_default();
        self.blocks.push_back(Block {
            first: self.end - rows as u64,
            rows,
            compressed: compressed.into_boxed_slice(),
        });
        self.open.clear();
        self.open_offsets.clear();
    }

    fn trim(&mut self) {
        let len = self.end - self.front;
        if len > self.capacity as u64 {
            self.front = self.end - self.capacity as u64;
        }
        while self
            .blocks
            .front()
            .is_some_and(|block| block.first + block.rows as u64 <= self.front)
        {
            let dropped = self.blocks.pop_front();
            let cache = self.cache.get_mut();
            if cache.as_ref().map(|c| c.first) == dropped.map(|b| b.first) {
                *cache = None;
            }
        }
        // The open rows only go once all of them are out; until then the
        // dropped ones are skipped by `front`.
        if self.blocks.is_empty() && self.end == self.front {
            self.open.clear();
            self.open_offsets.clear();
        }
    }
}

/// Lays cells out in rows of a fixed width, wrapping like alacritty.
struct Rewrap {
    row: Row<Cell>,
    col: usize,
    cols: usize,
}

impl Rewrap {
    fn new(cols: usize) -> Self {
        Self {
            row: Row::new(cols),
            col: 0,
            cols,
        }
    }

    fn put(&mut self, cell: Cell, out: &mut ScrollbackArchive) {
        let wide = cell.flags.contains(Flags::WIDE_CHAR);
        let wide_misfit = wide && self.cols > 1 && self.col == self.cols - 1;
        if self.col == self.cols || wide_misfit {
            if wide_misfit {
                let spacer = &mut self.row[Column(self.col)];
                spacer.c = ' ';
                spacer.flags = Flags::LEADING_WIDE_CHAR_SPACER;
            }
            self.row[Column(self.cols - 1)]
                .flags
                .insert(Flags::WRAPLINE);
            self.flush(out);
        }
        self.row[Column(self.col)] = cell;
        self.col += 1;
    }

    fn end_line(&mut self, out: &mut ScrollbackArchive) {
        self.flush(out);
    }

    fn end_wrapped(&mut self, out: &mut ScrollbackArchive) {
        if self.col > 0 {
            self.row[Column(self.cols - 1)]
                .flags
                .insert(Flags::WRAPLINE);
            self.flush(out);
        }
    }

    fn flush(&mut self, out: &mut ScrollbackArchive) {
        out.push(&self.row);
        self.row = Row::new(self.cols);
        self.col = 0;
    }
}

fn inflate(block: &Block) -> DecodedBlock {
    let mut bytes = Vec::new();
    // The block was deflated by `seal` from memory; inflating it cannot fail
    // short of a bug, and a short read leaves rows that decode blank.
    let _ = DeflateDecoder::new(&block.compressed[..]).read_to_end(&mut bytes);
    let mut pos = 0;
    let mut lengths = Vec::with_capacity(block.rows);
    for _ in 0..block.rows {
        lengths.push(read_varint(&bytes, &mut pos).unwrap_or(0) as usize);
    }
    let mut offsets = Vec::with_capacity(block.rows);
    for length in lengths {
        offsets.push(pos);
        pos += length;
    }
    let body = bytes.split_off(
        offsets
            .first()
            .copied()
            .unwrap_or(bytes.len())
            .min(bytes.len()),
    );
    let shift = bytes.len();
    DecodedBlock {
        first: block.first,
        bytes: body,
        offsets: offsets.into_iter().map(|offset| offset - shift).collect(),
    }
}

/// Everything about a cell but its text.
#[derive(Clone, PartialEq)]
struct Attrs {
    flags: Flags,
    fg: Color,
    bg: Color,
    underline: Option<Color>,
    hyperlink: Option<Hyperlink>,
}

impl Attrs {
    fn of(cell: &Cell) -> Self {
        Self {
            flags: cell.flags,
            fg: cell.fg,
            bg: cell.bg,
            underline: cell.underline_color(),
            hyperlink: cell.hyperlink(),
        }
    }

    fn apply(&self, cell: &mut Cell) {
        cell.flags = self.flags;
        cell.fg = self.fg;
        cell.bg = self.bg;
        cell.set_underline_color(self.underline);
        cell.set_hyperlink(self.hyperlink.clone());
    }

    fn encode(&self, out: &mut Vec<u8>) {
        write_varint(out, u64::from(self.flags.bits()));
        encode_color(self.fg, out);
        encode_color(self.bg, out);
        match self.underline {
            Some(color) => encode_color(color, out),
            None => out.push(COLOR_NONE),
        }
        match &self.hyperlink {
            Some(link) => {
                out.push(1);
                write_str(out, link.id());
                write_str(out, link.uri());
            }
            None => out.push(0),
        }
    }

    fn decode(bytes: &[u8], pos: &mut usize) -> Option<Self> {
        let flags = Flags::from_bits_truncate(u16::try_from(read_varint(bytes, pos)?).ok()?);
        let fg = decode_color(bytes, pos)??;
        let bg = decode_color(bytes, pos)??;
        let underline = decode_color(bytes, pos)?;
        let hyperlink = match *bytes.get(*pos)? {
            0 => {
                *pos += 1;
                None
            }
            _ => {
                *pos += 1;
                let id = read_str(bytes, pos)?;
                let uri = read_str(bytes, pos)?;
                Some(Hyperlink::new(Some(id), uri))
            }
        };
        Some(Self {
            flags,
            fg,
            bg,
            underline,
            hyperlink,
        })
    }
}

/// Row layout: width, cells in use, attribute runs, one char per cell, then
/// the cells carrying zero-width marks. Trailing default cells are not stored.
fn encode_row(row: &Row<Cell>, out: &mut Vec<u8>) {
    let cols = row.len();
    let default = Cell::default();
    let used = (0..cols)
        .rposition(|col| row[Column(col)] != default)
        .map_or(0, |col| col + 1);
    write_varint(out, cols as u64);
    write_varint(out, used as u64);

    let mut col = 0;
    while col < used {
        let attrs = Attrs::of(&row[Column(col)]);
        let start = col;
        col += 1;
        while col < used && Attrs::of(&row[Column(col)]) == attrs {
            col += 1;
        }
        write_varint(out, (col - start) as u64);
        attrs.encode(out);
    }

    for col in 0..used {
        write_varint(out, u64::from(u32::from(row[Column(col)].c)));
    }

    let marked: Vec<usize> = (0..used)
        .filter(|col| {
            row[Column(*col)]
                .zerowidth()
                .is_some_and(|marks| !marks.is_empty())
        })
        .collect();
    write_varint(out, marked.len() as u64);
    for col in marked {
        let marks = row[Column(col)].zerowidth().unwrap_or_default();
        write_varint(out, col as u64);
        write_varint(out, marks.len() as u64);
        for mark in marks {
            write_varint(out, u64::from(u32::from(*mark)));
        }
    }
}

/// Decode a row laid out `cols` wide (its archived width when `None`). A
/// narrower layout clips the row; a soft wrap moves to the new last column so
/// the row still joins the next one as text.
fn decode_row(bytes: &[u8], cols: Option<usize>) -> Row<Cell> {
    let mut pos = 0;
    let decoded = (|| {
        let stored = usize::try_from(read_varint(bytes, &mut pos)?).ok()?;
        let used = usize::try_from(read_varint(bytes, &mut pos)?).ok()?;
        let width = cols.unwrap_or(stored);
        let mut row = Row::new(width);
        let visible = used.min(width);

        let mut runs = Vec::new();
        let mut covered = 0;
        while covered < used {
            let len = usize::try_from(read_varint(bytes, &mut pos)?).ok()?;
            runs.push((len, Attrs::decode(bytes, &mut pos)?));
            covered += len;
        }
        let mut col = 0;
        for (len, attrs) in &runs {
            for _ in 0..*len {
                if col < visible {
                    attrs.apply(&mut row[Column(col)]);
                }
                col += 1;
            }
        }
        for col in 0..used {
            let c = char::from_u32(u32::try_from(read_varint(bytes, &mut pos)?).ok()?)?;
            if col < visible {
                row[Column(col)].c = c;
            }
        }
        let marked = read_varint(bytes, &mut pos)?;
        for _ in 0..marked {
            let col = usize::try_from(read_varint(bytes, &mut pos)?).ok()?;
            let count = read_varint(bytes, &mut pos)?;
            for _ in 0..count {
                let mark = char::from_u32(u32::try_from(read_varint(bytes, &mut pos)?).ok()?)?;
                if col < visible {
                    row[Column(col)].push_zerowidth(mark);
                }
            }
        }

        let wrapped = used == stored
            && runs
                .last()
                .is_some_and(|(_, attrs)| attrs.flags.contains(Flags::WRAPLINE));
        if width != stored && wrapped && width > 0 {
            if stored <= width {
                row[Column(stored - 1)].flags.remove(Flags::WRAPLINE);
            }
            row[Column(width - 1)].flags.insert(Flags::WRAPLINE);
        }
        Some(row)
    })();
    decoded.unwrap_or_else(|| Row::new(cols.unwrap_or(0)))
}

const COLOR_NAMED: u8 = 0;
const COLOR_SPEC: u8 = 1;
const COLOR_INDEXED: u8 = 2;
const COLOR_NONE: u8 = 3;

fn encode_color(color: Color, out: &mut Vec<u8>) {
    match color {
        Color::Named(named) => {
            out.push(COLOR_NAMED);
            write_varint(out, named as u64);
        }
        Color::Spec(rgb) => out.extend_from_slice(&[COLOR_SPEC, rgb.r, rgb.g, rgb.b]),
        Color::Indexed(index) => out.extend_from_slice(&[COLOR_INDEXED, index]),
    }
}

/// `Some(None)` is a stored "no colour"; `None` is malformed input.
fn decode_color(bytes: &[u8], pos: &mut usize) -> Option<Option<Color>> {
    let tag = *bytes.get(*pos)?;
    *pos += 1;
    Some(match tag {
        COLOR_NAMED => Some(Color::Named(named_color(read_varint(bytes, pos)?)?)),
        COLOR_SPEC => {
            let rgb = bytes.get(*pos..*pos + 3)?;
            *pos += 3;
            Some(Color::Spec(Rgb {
                r: rgb[0],
                g: rgb[1],
                b: rgb[2],
            }))
        }
        COLOR_INDEXED => {
            let index = *bytes.get(*pos)?;
            *pos += 1;
            Some(Color::Indexed(index))
        }
        COLOR_NONE => None,
        _ => return None,
    })
}

fn named_color(value: u64) -> Option<NamedColor> {
    use NamedColor::*;
    const ALL: [NamedColor; 29] = [
        Black,
        Red,
        Green,
        Yellow,
        Blue,
        Magenta,
        Cyan,
        White,
        BrightBlack,
        BrightRed,
        BrightGreen,
        BrightYellow,
        BrightBlue,
        BrightMagenta,
        BrightCyan,
        BrightWhite,
        Foreground,
        Background,
        Cursor,
        DimBlack,
        DimRed,
        DimGreen,
        DimYellow,
        DimBlue,
        DimMagenta,
        DimCyan,
        DimWhite,
        BrightForeground,
        DimForeground,
    ];
    ALL.into_iter().find(|named| *named as u64 == value)
}

fn write_varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push((value as u8) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn read_varint(bytes: &[u8], pos: &mut usize) -> Option<u64> {
    let mut value = 0u64;
    for shift in (0..64).step_by(7) {
        let byte = *bytes.get(*pos)?;
        *pos += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Some(value);
        }
    }
    None
}

fn write_str(out: &mut Vec<u8>, s: &str) {
    write_varint(out, s.len() as u64);
    out.extend_from_slice(s.as_bytes());
}

fn read_str(bytes: &[u8], pos: &mut usize) -> Option<String> {
    let len = usize::try_from(read_varint(bytes, pos)?).ok()?;
    let s = std::str::from_utf8(bytes.get(*pos..*pos + len)?).ok()?;
    *pos += len;
    Some(s.to_owned())
}

#[cfg(test)]
mod tests {
    use super::{BLOCK_ROWS, ScrollbackArchive};
    use alacritty_terminal::event::{Event, EventListener};
    use alacritty_terminal::grid::{Dimensions, Row};
    use alacritty_terminal::index::{Column, Line};
    use alacritty_terminal::term::cell::{Cell, Flags};
    use alacritty_terminal::term::test::TermSize;
    use alacritty_terminal::term::{Config, Term};
    use alacritty_terminal::vte::ansi::Processor;

    struct NoopListener;

    impl EventListener for NoopListener {
        fn send_event(&self, _event: Event) {}
    }

    /// Every screen row of a 12-column terminal fed `output`.
    fn rows_of(output: &str) -> Vec<Row<Cell>> {
        let mut term = Term::new(Config::default(), &TermSize::new(12, 6), NoopListener);
        let mut processor: Processor = Processor::new();
        processor.advance(&mut term, output.as_bytes());
        let grid = term.grid();
        (0..grid.screen_lines() as i32)
            .map(|line| grid[Line(line)].clone())
            .collect()
    }

    fn archived(rows: &[Row<Cell>]) -> ScrollbackArchive {
        let mut archive = ScrollbackArchive::new(10_000);
        for row in rows {
            archive.push(row);
        }
        archive
    }

    /// Rows as `row_from_newest` returns them at `cols`, oldest first.
    fn read_back(archive: &ScrollbackArchive, cols: usize) -> Vec<Row<Cell>> {
        (0..archive.len())
            .rev()
            .map(|back| {
                archive
                    .row_from_newest(back, cols)
                    .unwrap_or_else(|| Row::new(0))
            })
            .collect()
    }

    const STYLED: &str = concat!(
        "\x1b[1;3;31mbold\x1b[0m \x1b[4:3;58;2;1;2;3;38;5;200;48;2;9;8;7mx\x1b[0m\r\n",
        "\x1b[7;9;2mrev\x1b[0m e\u{0301}\u{0308}z \u{65e5}\u{672c}\r\n",
        "\x1b]8;id=a;https://example.com\x1b\\link\x1b]8;;\x1b\\ \x1b]8;;file:///x\x1b\\f\x1b]8;;\x1b\\\r\n",
        "wraps-past-twelve-columns\u{65e5}x",
    );

    #[test]
    fn archived_rows_read_back_identical() {
        let rows = rows_of(STYLED);
        let cells = || {
            rows.iter()
                .flat_map(|row| (0..12).map(move |col| &row[Column(col)]))
        };
        assert!(cells().any(|cell| cell.hyperlink().is_some()));
        assert!(cells().any(|cell| cell.zerowidth().is_some_and(|marks| marks.len() == 2)));
        assert!(cells().any(|cell| cell.underline_color().is_some()));
        assert!(cells().any(|cell| cell.flags.contains(Flags::WIDE_CHAR)));
        let archive = archived(&rows);
        assert_eq!(read_back(&archive, 12), rows);
        let mut full = Vec::new();
        archive.for_each_row(|row| full.push(row.clone()));
        assert_eq!(full, rows);
    }

    #[test]
    fn rows_read_back_identical_across_sealed_blocks() {
        let rows: Vec<Row<Cell>> = (0..BLOCK_ROWS * 2 + 7)
            .flat_map(|i| {
                rows_of(&format!("\x1b[3{}mrow {i}", i % 8))
                    .into_iter()
                    .take(1)
            })
            .collect();
        let archive = archived(&rows);
        assert_eq!(read_back(&archive, 12), rows);
    }

    #[test]
    fn a_narrower_read_clips_and_keeps_the_soft_wrap_on_its_last_column() {
        let rows = rows_of("abcdefghijklmnop");
        assert!(rows[0][Column(11)].flags.contains(Flags::WRAPLINE));
        let archive = archived(&rows[..1]);

        let narrow = archive.row_from_newest(0, 5).unwrap_or_else(|| Row::new(0));
        let text: String = (0..5).map(|col| narrow[Column(col)].c).collect();
        assert_eq!(text, "abcde");
        assert!(narrow[Column(4)].flags.contains(Flags::WRAPLINE));

        let wide = archive
            .row_from_newest(0, 20)
            .unwrap_or_else(|| Row::new(0));
        assert!(!wide[Column(11)].flags.contains(Flags::WRAPLINE));
        assert!(wide[Column(19)].flags.contains(Flags::WRAPLINE));
        assert_eq!(wide[Column(12)], Cell::default());
    }

    fn row_texts(archive: &ScrollbackArchive) -> Vec<String> {
        let mut texts = Vec::new();
        archive.for_each_row(|row| {
            let text: String = (0..row.len())
                .map(|col| &row[Column(col)])
                .filter(|cell| !cell.flags.contains(Flags::WIDE_CHAR_SPACER))
                .map(|cell| cell.c)
                .collect();
            texts.push(text.trim_end().to_owned());
        });
        texts
    }

    #[test]
    fn a_reflow_rewraps_lines_like_alacritty() {
        // 12 columns: a wrapped line, a wide char at the wrap, a blank line.
        let rows = rows_of("abcdefghijklmnop\r\nxy\u{65e5}\u{672c}z\r\n\r\nend");
        let mut archive = archived(&rows[..5]);

        archive.reflow(3);
        assert_eq!(
            row_texts(&archive),
            [
                "abc",
                "def",
                "ghi",
                "jkl",
                "mno",
                "p",
                "xy",
                "\u{65e5}",
                "\u{672c}z",
                "",
                "end"
            ]
        );
        // The wide char that did not fit left a leading spacer and wrapped.
        let mut third_col = Vec::new();
        archive.for_each_row(|row| third_col.push(row[Column(2)].flags));
        assert!(third_col[6].contains(Flags::LEADING_WIDE_CHAR_SPACER | Flags::WRAPLINE));

        // Back at the first width the rows are what they were.
        archive.reflow(12);
        assert_eq!(read_back(&archive, 12), rows[..5]);
    }

    #[test]
    fn a_reflow_keeps_the_last_line_open_into_the_grid() {
        // The newest archived row wraps on into a row still in the grid.
        let rows = rows_of("abcdefghijklmnop");
        let mut archive = archived(&rows[..1]);
        archive.reflow(5);
        assert_eq!(row_texts(&archive), ["abcde", "fghij", "kl"]);
        let mut last = None;
        archive.for_each_row(|row| last = Some(row[Column(4)].flags));
        assert_eq!(
            last.map(|flags| flags.contains(Flags::WRAPLINE)),
            Some(true)
        );
    }

    #[test]
    fn the_capacity_drops_the_oldest_rows() {
        let rows: Vec<Row<Cell>> = (0..BLOCK_ROWS + 50)
            .flat_map(|i| rows_of(&format!("r{i}")).into_iter().take(1))
            .collect();
        let mut archive = archived(&rows);
        archive.set_capacity(100);
        assert_eq!(archive.len(), 100);
        assert_eq!(read_back(&archive, 12), rows[rows.len() - 100..]);
        assert_eq!(archive.archived_total(), rows.len() as u64);

        archive.clear();
        assert_eq!(archive.len(), 0);
        assert!(archive.row_from_newest(0, 12).is_none());
    }

    #[test]
    fn full_width_text_compresses_far_below_the_grid_cost() {
        let line: String = (0..200)
            .map(|i| char::from(b'a' + (i % 26) as u8))
            .collect();
        let mut term = Term::new(Config::default(), &TermSize::new(200, 1), NoopListener);
        let mut processor: Processor = Processor::new();
        let mut archive = ScrollbackArchive::new(30_000);
        for i in 0..30_000 {
            processor.advance(&mut term, format!("{i:>6} {}", &line[7..]).as_bytes());
            archive.push(&term.grid()[Line(0)]);
            processor.advance(&mut term, b"\r");
        }
        let grid_cost = 30_000 * 200 * size_of::<Cell>();
        assert!(
            archive.heap_bytes() * 20 < grid_cost,
            "{} archived bytes for {grid_cost} grid bytes",
            archive.heap_bytes()
        );
    }
}
