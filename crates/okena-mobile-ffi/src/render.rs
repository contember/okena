//! Hand-written C ABI (`okena_rv_*`) for the native terminal views.
//!
//! The Swift (iOS) and Kotlin/JNI (Android) terminal views call these
//! functions directly instead of going through the uniffi/JSI module. They run
//! in the same Rust copy as the JSI module (one static library linked once into
//! the app binary on iOS, into `libokena-mobile-ffi.so` on Android), so they
//! see the same `ConnectionManager` singleton and the connections JS opened.
//!
//! The contract is `crates/okena-mobile-ffi/include/okena_render.h`. Keep the
//! two in sync: every `#[unsafe(no_mangle)]` function and `#[repr(C)]` struct
//! here has a counterpart there, and the layout asserts below mirror the
//! header's `_Static_assert`s.
//!
//! No function here may unwind into the caller. Every entry point goes through
//! [`guard`], which turns a panic into the documented fallback value.
//!
//! ## Line addressing
//! See `RenderTrack` in `client/terminal_holder.rs` and the header. In short:
//! absolute line `n` keeps its number while output scrolls;
//! `screen_top_line` is the absolute number of the top screen row and equals
//! the count of lines that ever scrolled into history; the oldest readable
//! line is `first_line = screen_top_line - history_size`.
//!
//! ## Change detection
//! alacritty's `Term::damage()` is viewport-relative, needs `&mut Term` plus a
//! reset (one consumer only), and reports full damage whenever the screen
//! scrolls, which is every line of streaming output. It cannot tell a view
//! which absolute rows changed, so we use a per-row content hash instead
//! (computed while the row is copied anyway) next to a lock-free generation.

use std::ffi::{CStr, c_char};
use std::mem::{offset_of, size_of};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::{Term, TermMode};
use alacritty_terminal::vte::ansi::CursorShape;
use okena_core::theme::{DARK_THEME, ThemeColors};
use okena_transport::client::WsClientMessage;

use crate::client::manager::ConnectionManager;
use crate::client::terminal_holder::{
    GENERATION_DEAD, NoopEventListener, RenderTrackSnapshot, TerminalHolder, mouse_wheel_bytes,
};

/// Bumped on every incompatible change to `okena_render.h`.
pub const ABI_VERSION: u32 = 1;

/// Upper bound for `rows` in one snapshot.
pub const MAX_SNAPSHOT_ROWS: u16 = 1024;

// ── Frame layout (mirrors okena_render.h) ───────────────────────────────

pub const STATUS_OK: u32 = 0;
pub const STATUS_MISSING: u32 = 1;

pub const CELL_BOLD: u16 = 1 << 0;
pub const CELL_ITALIC: u16 = 1 << 1;
pub const CELL_UNDERLINE: u16 = 1 << 2;
pub const CELL_STRIKE: u16 = 1 << 3;
pub const CELL_INVERSE: u16 = 1 << 4;
pub const CELL_DIM: u16 = 1 << 5;
pub const CELL_WIDE: u16 = 1 << 6;
pub const CELL_WIDE_SPACER: u16 = 1 << 7;
pub const CELL_LEADING_WIDE_SPACER: u16 = 1 << 8;
pub const CELL_HIDDEN: u16 = 1 << 9;
pub const CELL_DOUBLE_UNDERLINE: u16 = 1 << 10;
pub const CELL_UNDERCURL: u16 = 1 << 11;
pub const CELL_DOTTED_UNDERLINE: u16 = 1 << 12;
pub const CELL_DASHED_UNDERLINE: u16 = 1 << 13;

pub const ROW_VOID: u32 = 1 << 0;
pub const ROW_WRAPPED: u32 = 1 << 1;

pub const MODE_MOUSE: u32 = 1 << 0;
pub const MODE_ALT_SCREEN: u32 = 1 << 1;
pub const MODE_ALTERNATE_SCROLL: u32 = 1 << 2;
pub const MODE_APP_CURSOR: u32 = 1 << 3;
pub const MODE_BRACKETED_PASTE: u32 = 1 << 4;
pub const MODE_SGR_MOUSE: u32 = 1 << 5;
pub const MODE_SHOW_CURSOR: u32 = 1 << 6;

pub const CURSOR_BLOCK: u8 = 0;
pub const CURSOR_UNDERLINE: u8 = 1;
pub const CURSOR_BEAM: u8 = 2;
pub const CURSOR_HOLLOW_BLOCK: u8 = 3;

pub const SEL_NONE: u8 = 0;
pub const SEL_STREAM: u8 = 1;
pub const SEL_BLOCK: u8 = 2;

pub const SELECT_SIMPLE: u8 = 0;
pub const SELECT_WORD: u8 = 1;
pub const SELECT_LINES: u8 = 2;
pub const SELECT_BLOCK: u8 = 3;

pub const SIDE_LEFT: u8 = 0;
pub const SIDE_RIGHT: u8 = 1;

/// One grid cell. 16 bytes, 4-byte aligned.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OkenaRvCell {
    /// Base Unicode scalar. `' '` for empty and spacer cells.
    pub codepoint: u32,
    /// Foreground ARGB (0xAARRGGBB), inverse and hidden already applied.
    pub fg: u32,
    /// Background ARGB, inverse already applied.
    pub bg: u32,
    /// `CELL_*` bits.
    pub flags: u16,
    /// 0 = none, else 1-based index into the frame's extras (the full
    /// grapheme: base char plus combining marks, UTF-8).
    pub extra: u16,
}

/// A string in the frame's text table. 8 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OkenaRvExtra {
    /// Byte offset into the text table.
    pub offset: u32,
    /// Length in bytes (UTF-8, not NUL-terminated).
    pub len: u32,
}

/// Per-row metadata. 24 bytes, 8-byte aligned.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OkenaRvRow {
    /// Absolute line number of this row.
    pub line: i64,
    /// Content hash of the row's cells and graphemes; never 0 for a real row,
    /// 0 for a void row. Same (line, hash) means same pixels, apart from the
    /// cursor and selection overlays.
    pub hash: u64,
    /// `ROW_*` bits.
    pub flags: u32,
    pub reserved: u32,
}

/// Frame header at offset 0 of the frame blob. 136 bytes, 8-byte aligned.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct OkenaRvFrame {
    pub abi_version: u32,
    pub header_size: u32,
    pub total_size: u32,
    pub status: u32,
    pub generation: u64,
    pub top_line: i64,
    pub first_line: i64,
    pub screen_top_line: i64,
    pub cursor_line: i64,
    pub sel_start_line: i64,
    pub sel_end_line: i64,
    pub history_size: u32,
    pub mode: u32,
    pub reflow_epoch: u32,
    pub default_fg: u32,
    pub default_bg: u32,
    pub cols: u16,
    pub rows: u16,
    pub screen_lines: u16,
    pub cursor_col: u16,
    pub sel_start_col: u16,
    pub sel_end_col: u16,
    pub cursor_shape: u8,
    pub cursor_visible: u8,
    pub sel_kind: u8,
    pub reserved0: u8,
    pub rows_offset: u32,
    pub cells_offset: u32,
    pub extras_offset: u32,
    pub extras_count: u32,
    pub text_offset: u32,
    pub text_len: u32,
    pub reserved1: u32,
}

const _: () = {
    assert!(size_of::<OkenaRvCell>() == 16);
    assert!(size_of::<OkenaRvExtra>() == 8);
    assert!(size_of::<OkenaRvRow>() == 24);
    assert!(size_of::<OkenaRvFrame>() == 136);
    assert!(offset_of!(OkenaRvCell, flags) == 12);
    assert!(offset_of!(OkenaRvCell, extra) == 14);
    assert!(offset_of!(OkenaRvRow, hash) == 8);
    assert!(offset_of!(OkenaRvRow, flags) == 16);
    assert!(offset_of!(OkenaRvFrame, generation) == 16);
    assert!(offset_of!(OkenaRvFrame, top_line) == 24);
    assert!(offset_of!(OkenaRvFrame, first_line) == 32);
    assert!(offset_of!(OkenaRvFrame, screen_top_line) == 40);
    assert!(offset_of!(OkenaRvFrame, cursor_line) == 48);
    assert!(offset_of!(OkenaRvFrame, sel_start_line) == 56);
    assert!(offset_of!(OkenaRvFrame, sel_end_line) == 64);
    assert!(offset_of!(OkenaRvFrame, history_size) == 72);
    assert!(offset_of!(OkenaRvFrame, mode) == 76);
    assert!(offset_of!(OkenaRvFrame, reflow_epoch) == 80);
    assert!(offset_of!(OkenaRvFrame, default_fg) == 84);
    assert!(offset_of!(OkenaRvFrame, default_bg) == 88);
    assert!(offset_of!(OkenaRvFrame, cols) == 92);
    assert!(offset_of!(OkenaRvFrame, rows) == 94);
    assert!(offset_of!(OkenaRvFrame, screen_lines) == 96);
    assert!(offset_of!(OkenaRvFrame, cursor_col) == 98);
    assert!(offset_of!(OkenaRvFrame, sel_start_col) == 100);
    assert!(offset_of!(OkenaRvFrame, sel_end_col) == 102);
    assert!(offset_of!(OkenaRvFrame, cursor_shape) == 104);
    assert!(offset_of!(OkenaRvFrame, cursor_visible) == 105);
    assert!(offset_of!(OkenaRvFrame, sel_kind) == 106);
    assert!(offset_of!(OkenaRvFrame, rows_offset) == 108);
    assert!(offset_of!(OkenaRvFrame, cells_offset) == 112);
    assert!(offset_of!(OkenaRvFrame, extras_offset) == 116);
    assert!(offset_of!(OkenaRvFrame, extras_count) == 120);
    assert!(offset_of!(OkenaRvFrame, text_offset) == 124);
    assert!(offset_of!(OkenaRvFrame, text_len) == 128);
};

// ── Frame building ──────────────────────────────────────────────────────

/// Reusable, Rust-owned frame storage. The blob is a `Vec<u64>` so every
/// section starts 8-byte aligned; it only grows, so steady-state snapshots
/// allocate nothing.
#[derive(Default)]
pub struct FrameBuffers {
    blob: Vec<u64>,
    extras: Vec<OkenaRvExtra>,
    text: Vec<u8>,
}

impl FrameBuffers {
    pub fn frame_ptr(&self) -> *const OkenaRvFrame {
        self.blob.as_ptr().cast()
    }

    /// The header of the last frame built. Call only after a build.
    pub fn header(&self) -> &OkenaRvFrame {
        assert!(self.blob.len() * 8 >= size_of::<OkenaRvFrame>());
        // SAFETY: every build writes a full header at offset 0 of an 8-byte
        // aligned blob of at least `size_of::<OkenaRvFrame>()` bytes.
        unsafe { &*self.frame_ptr() }
    }

    pub fn rows(&self) -> &[OkenaRvRow] {
        let h = self.header();
        // SAFETY: the build wrote `h.rows` rows at `rows_offset`.
        unsafe {
            std::slice::from_raw_parts(self.byte_ptr(h.rows_offset).cast(), usize::from(h.rows))
        }
    }

    pub fn cells(&self) -> &[OkenaRvCell] {
        let h = self.header();
        // SAFETY: the build wrote `rows * cols` cells at `cells_offset`.
        unsafe {
            std::slice::from_raw_parts(
                self.byte_ptr(h.cells_offset).cast(),
                usize::from(h.rows) * usize::from(h.cols),
            )
        }
    }

    /// The grapheme string of a cell's `extra` index (1-based).
    pub fn extra_str(&self, extra: u16) -> Option<&str> {
        let h = self.header();
        let idx = usize::from(extra).checked_sub(1)?;
        if idx >= h.extras_count as usize {
            return None;
        }
        // SAFETY: the build wrote `extras_count` extras and `text_len` bytes.
        let (e, text) = unsafe {
            let extras: &[OkenaRvExtra] = std::slice::from_raw_parts(
                self.byte_ptr(h.extras_offset).cast(),
                h.extras_count as usize,
            );
            let text =
                std::slice::from_raw_parts(self.byte_ptr(h.text_offset), h.text_len as usize);
            (extras[idx], text)
        };
        std::str::from_utf8(text.get(e.offset as usize..(e.offset + e.len) as usize)?).ok()
    }

    fn byte_ptr(&self, offset: u32) -> *const u8 {
        // SAFETY: offsets come from the header and lie inside the blob.
        unsafe { self.blob.as_ptr().cast::<u8>().add(offset as usize) }
    }

    /// Make the blob at least `bytes` long and return its base pointer.
    fn reserve_bytes(&mut self, bytes: usize) -> *mut u8 {
        let words = bytes.div_ceil(8);
        if self.blob.len() < words {
            self.blob.resize(words, 0);
        }
        self.blob.as_mut_ptr().cast()
    }

    fn write_header(&mut self, header: OkenaRvFrame) {
        let base = self.reserve_bytes(size_of::<OkenaRvFrame>());
        // SAFETY: the blob holds at least a header and is 8-byte aligned.
        unsafe { base.cast::<OkenaRvFrame>().write(header) };
    }

    /// A frame for a terminal that does not exist (yet).
    pub fn build_missing(&mut self, top_line: i64) {
        let header_size = size_of::<OkenaRvFrame>() as u32;
        self.write_header(OkenaRvFrame {
            abi_version: ABI_VERSION,
            header_size,
            total_size: header_size,
            status: STATUS_MISSING,
            top_line,
            first_line: top_line,
            screen_top_line: top_line,
            default_fg: 0xFF00_0000 | DARK_THEME.term_foreground,
            default_bg: 0xFF00_0000 | DARK_THEME.term_background,
            rows_offset: header_size,
            cells_offset: header_size,
            extras_offset: header_size,
            text_offset: header_size,
            ..OkenaRvFrame::default()
        });
    }

    /// Fill a frame with `rows` rows starting at absolute line `top_line`.
    pub(crate) fn build(
        &mut self,
        term: &Term<NoopEventListener>,
        track: RenderTrackSnapshot,
        generation: u64,
        top_line: i64,
        rows: u16,
        theme: &ThemeColors,
    ) {
        let rows = rows.min(MAX_SNAPSHOT_ROWS);
        let grid = term.grid();
        let cols = grid.columns().min(usize::from(u16::MAX));
        let screen_lines = grid.screen_lines() as i64;
        let history = grid.history_size() as i64;
        let screen_top = track.screen_top;
        let default_fg = 0xFF00_0000 | theme.term_foreground;
        let default_bg = 0xFF00_0000 | theme.term_background;

        let header_size = size_of::<OkenaRvFrame>();
        let rows_offset = header_size;
        let cells_offset = rows_offset + usize::from(rows) * size_of::<OkenaRvRow>();
        let cell_count = usize::from(rows) * cols;
        let extras_offset = cells_offset + cell_count * size_of::<OkenaRvCell>();

        self.extras.clear();
        self.text.clear();
        let base = self.reserve_bytes(extras_offset);
        // SAFETY: the blob is at least `extras_offset` bytes, 8-byte aligned,
        // and both sections start at multiples of 8 (136, 24 and 16 are).
        let (row_out, cell_out) = unsafe {
            (
                std::slice::from_raw_parts_mut(
                    base.add(rows_offset).cast::<OkenaRvRow>(),
                    usize::from(rows),
                ),
                std::slice::from_raw_parts_mut(
                    base.add(cells_offset).cast::<OkenaRvCell>(),
                    cell_count,
                ),
            )
        };
        let blank = OkenaRvCell {
            codepoint: u32::from(' '),
            fg: default_fg,
            bg: default_bg,
            flags: 0,
            extra: 0,
        };

        for (r, row_meta) in row_out.iter_mut().enumerate() {
            let line = top_line + r as i64;
            let buffer_line = line - screen_top;
            let out = &mut cell_out[r * cols..(r + 1) * cols];
            if buffer_line < -history || buffer_line >= screen_lines {
                out.fill(blank);
                *row_meta = OkenaRvRow {
                    line,
                    hash: 0,
                    flags: ROW_VOID,
                    reserved: 0,
                };
                continue;
            }
            let grid_row = &grid[Line(buffer_line as i32)];
            let mut hash = FNV_OFFSET;
            for (c, slot) in out.iter_mut().enumerate() {
                let cell = &grid_row[Column(c)];
                *slot = encode_cell(cell, theme, &mut self.extras, &mut self.text);
                hash = hash_cell(hash, slot);
                if slot.extra != 0
                    && let Some(e) = self.extras.last()
                {
                    hash = hash_bytes(
                        hash,
                        &self.text[e.offset as usize..(e.offset + e.len) as usize],
                    );
                }
            }
            let wrapped = cols > 0 && grid_row[Column(cols - 1)].flags.contains(Flags::WRAPLINE);
            *row_meta = OkenaRvRow {
                line,
                hash: if hash == 0 { 1 } else { hash },
                flags: if wrapped { ROW_WRAPPED } else { 0 },
                reserved: 0,
            };
        }

        // Extras and the text table go after the cells.
        let extras_bytes = self.extras.len() * size_of::<OkenaRvExtra>();
        let text_offset = extras_offset + extras_bytes;
        let total = text_offset + self.text.len();
        let base = self.reserve_bytes(total);
        // SAFETY: the blob is at least `total` bytes; extras start 8-aligned.
        unsafe {
            std::ptr::copy_nonoverlapping(
                self.extras.as_ptr(),
                base.add(extras_offset).cast::<OkenaRvExtra>(),
                self.extras.len(),
            );
            std::ptr::copy_nonoverlapping(
                self.text.as_ptr(),
                base.add(text_offset),
                self.text.len(),
            );
        }

        let mode = *term.mode();
        let cursor = grid.cursor.point;
        let (cursor_shape, shape_visible) = match term.cursor_style().shape {
            CursorShape::Block => (CURSOR_BLOCK, true),
            CursorShape::Underline => (CURSOR_UNDERLINE, true),
            CursorShape::Beam => (CURSOR_BEAM, true),
            CursorShape::HollowBlock => (CURSOR_HOLLOW_BLOCK, true),
            CursorShape::Hidden => (CURSOR_BLOCK, false),
        };
        let selection = term.selection.as_ref().and_then(|s| s.to_range(term));

        self.write_header(OkenaRvFrame {
            abi_version: ABI_VERSION,
            header_size: header_size as u32,
            total_size: total as u32,
            status: STATUS_OK,
            generation,
            top_line,
            first_line: screen_top - history,
            screen_top_line: screen_top,
            cursor_line: screen_top + i64::from(cursor.line.0),
            sel_start_line: selection.map_or(0, |s| screen_top + i64::from(s.start.line.0)),
            sel_end_line: selection.map_or(0, |s| screen_top + i64::from(s.end.line.0)),
            history_size: history as u32,
            mode: mode_bits(mode),
            reflow_epoch: track.reflow_epoch,
            default_fg,
            default_bg,
            cols: cols as u16,
            rows,
            screen_lines: screen_lines as u16,
            cursor_col: cursor.column.0 as u16,
            sel_start_col: selection.map_or(0, |s| s.start.column.0 as u16),
            sel_end_col: selection.map_or(0, |s| s.end.column.0 as u16),
            cursor_shape,
            cursor_visible: u8::from(shape_visible && mode.contains(TermMode::SHOW_CURSOR)),
            sel_kind: match selection {
                None => SEL_NONE,
                Some(s) if s.is_block => SEL_BLOCK,
                Some(_) => SEL_STREAM,
            },
            reserved0: 0,
            rows_offset: rows_offset as u32,
            cells_offset: cells_offset as u32,
            extras_offset: extras_offset as u32,
            extras_count: self.extras.len() as u32,
            text_offset: text_offset as u32,
            text_len: self.text.len() as u32,
            reserved1: 0,
        });
    }
}

fn encode_cell(
    cell: &Cell,
    theme: &ThemeColors,
    extras: &mut Vec<OkenaRvExtra>,
    text: &mut Vec<u8>,
) -> OkenaRvCell {
    let f = cell.flags;
    let (mut fg, mut bg) = (cell.fg, cell.bg);
    if f.contains(Flags::INVERSE) {
        std::mem::swap(&mut fg, &mut bg);
    }
    let bg_argb = theme.ansi_to_argb(&bg);
    let fg_argb = if f.contains(Flags::HIDDEN) {
        bg_argb
    } else {
        theme.ansi_to_argb(&fg)
    };

    let mut flags = 0u16;
    for (cell_flag, bit) in [
        (Flags::BOLD, CELL_BOLD),
        (Flags::ITALIC, CELL_ITALIC),
        (Flags::ALL_UNDERLINES, CELL_UNDERLINE),
        (Flags::STRIKEOUT, CELL_STRIKE),
        (Flags::INVERSE, CELL_INVERSE),
        (Flags::DIM, CELL_DIM),
        (Flags::WIDE_CHAR, CELL_WIDE),
        (Flags::WIDE_CHAR_SPACER, CELL_WIDE_SPACER),
        (Flags::LEADING_WIDE_CHAR_SPACER, CELL_LEADING_WIDE_SPACER),
        (Flags::HIDDEN, CELL_HIDDEN),
        (Flags::DOUBLE_UNDERLINE, CELL_DOUBLE_UNDERLINE),
        (Flags::UNDERCURL, CELL_UNDERCURL),
        (Flags::DOTTED_UNDERLINE, CELL_DOTTED_UNDERLINE),
        (Flags::DASHED_UNDERLINE, CELL_DASHED_UNDERLINE),
    ] {
        if f.intersects(cell_flag) {
            flags |= bit;
        }
    }

    let spacer = f.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER);
    let codepoint = if spacer || cell.c == '\0' {
        u32::from(' ')
    } else {
        u32::from(cell.c)
    };

    let mut extra = 0u16;
    if !spacer
        && let Some(marks) = cell.zerowidth()
        && !marks.is_empty()
        && extras.len() < usize::from(u16::MAX)
    {
        let offset = text.len() as u32;
        let mut tmp = [0u8; 4];
        text.extend_from_slice(cell.c.encode_utf8(&mut tmp).as_bytes());
        for m in marks {
            text.extend_from_slice(m.encode_utf8(&mut tmp).as_bytes());
        }
        extras.push(OkenaRvExtra {
            offset,
            len: text.len() as u32 - offset,
        });
        extra = extras.len() as u16;
    }

    OkenaRvCell {
        codepoint,
        fg: fg_argb,
        bg: bg_argb,
        flags,
        extra,
    }
}

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

fn hash_cell(hash: u64, cell: &OkenaRvCell) -> u64 {
    let a = u64::from(cell.codepoint) | (u64::from(cell.fg) << 32);
    let b = u64::from(cell.bg) | (u64::from(cell.flags) << 32) | (u64::from(cell.extra) << 48);
    let hash = (hash ^ a).wrapping_mul(FNV_PRIME);
    (hash ^ b).wrapping_mul(FNV_PRIME)
}

fn hash_bytes(mut hash: u64, bytes: &[u8]) -> u64 {
    for &b in bytes {
        hash = (hash ^ u64::from(b)).wrapping_mul(FNV_PRIME);
    }
    hash
}

fn mode_bits(mode: TermMode) -> u32 {
    let mut bits = 0;
    for (term_mode, bit) in [
        (TermMode::MOUSE_MODE, MODE_MOUSE),
        (TermMode::ALT_SCREEN, MODE_ALT_SCREEN),
        (TermMode::ALTERNATE_SCROLL, MODE_ALTERNATE_SCROLL),
        (TermMode::APP_CURSOR, MODE_APP_CURSOR),
        (TermMode::BRACKETED_PASTE, MODE_BRACKETED_PASTE),
        (TermMode::SGR_MOUSE, MODE_SGR_MOUSE),
        (TermMode::SHOW_CURSOR, MODE_SHOW_CURSOR),
    ] {
        if mode.intersects(term_mode) {
            bits |= bit;
        }
    }
    bits
}

// ── Holder-level operations (tested without the connection manager) ────

/// Map an absolute line / column to an alacritty point, clamped to the grid.
fn to_point(
    term: &Term<NoopEventListener>,
    track: RenderTrackSnapshot,
    line: i64,
    col: u16,
) -> Point {
    let grid = term.grid();
    let history = grid.history_size() as i64;
    let last = (grid.screen_lines() as i64 - 1).max(0);
    let buffer_line = (line - track.screen_top).clamp(-history, last);
    let max_col = grid.columns().saturating_sub(1);
    Point::new(
        Line(buffer_line as i32),
        Column(usize::from(col).min(max_col)),
    )
}

fn to_side(side: u8) -> Side {
    if side == SIDE_RIGHT {
        Side::Right
    } else {
        Side::Left
    }
}

pub(crate) fn selection_start(holder: &TerminalHolder, line: i64, col: u16, side: u8, kind: u8) {
    let ty = match kind {
        SELECT_WORD => SelectionType::Semantic,
        SELECT_LINES => SelectionType::Lines,
        SELECT_BLOCK => SelectionType::Block,
        _ => SelectionType::Simple,
    };
    holder.with_term_mut(|term, track| {
        let point = to_point(term, track, line, col);
        term.selection = Some(Selection::new(ty, point, to_side(side)));
        true
    });
}

pub(crate) fn selection_update(holder: &TerminalHolder, line: i64, col: u16, side: u8) {
    holder.with_term_mut(|term, track| {
        let point = to_point(term, track, line, col);
        match term.selection.as_mut() {
            Some(sel) => {
                sel.update(point, to_side(side));
                true
            }
            None => false,
        }
    });
}

/// Wheel bytes for `lines` notches (positive = up) at screen cell
/// `col`/`row`, or `None` when the view should scroll itself.
///
/// Mouse reporting wins (same encoding as the uniffi `scroll_wheel`); on the
/// alt screen with alternate scroll (1007) the notches become arrow keys, as
/// in xterm and alacritty.
pub(crate) fn wheel_bytes(
    holder: &TerminalHolder,
    lines: i32,
    col: u16,
    row: u16,
) -> Option<Vec<u8>> {
    if lines == 0 {
        return None;
    }
    let up = lines > 0;
    let count = lines.unsigned_abs().min(200);
    holder.with_term(|term, _| {
        let mode = *term.mode();
        if mode.intersects(TermMode::MOUSE_MODE) {
            return Some(mouse_wheel_bytes(
                up,
                col,
                row,
                count,
                mode.contains(TermMode::SGR_MOUSE),
            ));
        }
        if mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL) {
            let key: &[u8] = match (up, mode.contains(TermMode::APP_CURSOR)) {
                (true, true) => b"\x1bOA",
                (true, false) => b"\x1b[A",
                (false, true) => b"\x1bOB",
                (false, false) => b"\x1b[B",
            };
            return Some(key.repeat(count as usize));
        }
        None
    })
}

// ── C ABI ───────────────────────────────────────────────────────────────

/// Run `f`, returning `fallback` if it panics.
fn guard<R>(fallback: R, f: impl FnOnce() -> R) -> R {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or(fallback)
}

/// Borrow a NUL-terminated UTF-8 string from the caller.
///
/// # Safety
/// `ptr` must be null or point to a NUL-terminated string that outlives `'a`.
unsafe fn borrow_str<'a>(ptr: *const c_char) -> Option<&'a str> {
    if ptr.is_null() {
        return None;
    }
    // SAFETY: non-null and NUL-terminated per the caller contract.
    unsafe { CStr::from_ptr(ptr) }.to_str().ok()
}

/// Copy `text` into a caller buffer of `cap` bytes, NUL-terminated and
/// truncated if needed. Returns `text.len()`, so a caller can retry with a
/// larger buffer.
///
/// # Safety
/// `buf` must be null or valid for `cap` bytes of writes.
unsafe fn write_c_buf(text: &str, buf: *mut c_char, cap: usize) -> usize {
    if !buf.is_null() && cap > 0 {
        let n = text.len().min(cap - 1);
        // SAFETY: `buf` is valid for `cap` bytes and `n + 1 <= cap`.
        unsafe {
            std::ptr::copy_nonoverlapping(text.as_ptr(), buf.cast::<u8>(), n);
            *buf.add(n) = 0;
        }
    }
    text.len()
}

/// A native view's connection to one terminal. Owns the reused frame and
/// the selection-text buffer. Not thread-safe: one thread at a time.
pub struct OkenaRvHandle {
    conn_id: String,
    terminal_id: String,
    counter: Option<Arc<AtomicU64>>,
    frame: FrameBuffers,
    selection_text: Vec<u8>,
}

impl OkenaRvHandle {
    fn with_holder<R>(&self, f: impl FnOnce(&TerminalHolder) -> R) -> Option<R> {
        ConnectionManager::try_get()?.with_terminal(&self.conn_id, &self.terminal_id, f)
    }
}

/// # Safety
/// `handle` must be null or a live pointer from `okena_rv_open`.
unsafe fn handle_mut<'a>(handle: *mut OkenaRvHandle) -> Option<&'a mut OkenaRvHandle> {
    // SAFETY: caller contract.
    unsafe { handle.as_mut() }
}

/// ABI version of this library; native code compares it with
/// `OKENA_RV_ABI_VERSION` from the header it was compiled against.
#[unsafe(no_mangle)]
pub extern "C" fn okena_rv_abi_version() -> u32 {
    ABI_VERSION
}

/// Number of terminals the connection `conn_id` mirrors, or -1 if the
/// connection does not exist (or `init_app` has not run yet).
///
/// # Safety
/// `conn_id` must be null or a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn okena_rv_terminal_count(conn_id: *const c_char) -> i32 {
    guard(-1, || {
        // SAFETY: forwarded caller contract.
        let Some(conn_id) = (unsafe { borrow_str(conn_id) }) else {
            return -1;
        };
        ConnectionManager::try_get()
            .and_then(|mgr| mgr.terminal_ids(conn_id))
            .map_or(-1, |ids| i32::try_from(ids.len()).unwrap_or(i32::MAX))
    })
}

/// Diagnostic listing of the live connections, one line each:
/// `<conn_id>\t<terminal_count>\t<term_id>,<term_id>,...\n`.
///
/// # Safety
/// `buf` must be null or valid for `cap` bytes of writes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn okena_rv_debug_connections(buf: *mut c_char, cap: usize) -> usize {
    guard(0, || {
        let text = connections_listing();
        // SAFETY: forwarded caller contract.
        unsafe { write_c_buf(&text, buf, cap) }
    })
}

fn connections_listing() -> String {
    let Some(mgr) = ConnectionManager::try_get() else {
        return String::new();
    };
    let mut ids = mgr.connection_ids();
    ids.sort();
    let mut out = String::new();
    for conn_id in ids {
        let terms = mgr.terminal_ids(&conn_id).unwrap_or_default();
        out.push_str(&format!(
            "{conn_id}\t{}\t{}\n",
            terms.len(),
            terms.join(",")
        ));
    }
    out
}

/// Open a view handle. The terminal may appear later (snapshots report
/// `STATUS_MISSING` until it does). Returns null only for null / non-UTF-8
/// ids. Free with `okena_rv_close`.
///
/// # Safety
/// Both arguments must be null or NUL-terminated strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn okena_rv_open(
    conn_id: *const c_char,
    terminal_id: *const c_char,
) -> *mut OkenaRvHandle {
    guard(std::ptr::null_mut(), || {
        // SAFETY: forwarded caller contract.
        let (Some(conn_id), Some(terminal_id)) = (unsafe { borrow_str(conn_id) }, unsafe {
            borrow_str(terminal_id)
        }) else {
            return std::ptr::null_mut();
        };
        Box::into_raw(Box::new(OkenaRvHandle {
            conn_id: conn_id.to_owned(),
            terminal_id: terminal_id.to_owned(),
            counter: None,
            frame: FrameBuffers::default(),
            selection_text: Vec::new(),
        }))
    })
}

/// Free a handle and its frame. Null is a no-op.
///
/// # Safety
/// `handle` must be null or a pointer from `okena_rv_open` not yet closed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn okena_rv_close(handle: *mut OkenaRvHandle) {
    if handle.is_null() {
        return;
    }
    // SAFETY: caller contract; the box came from `okena_rv_open`.
    let boxed = unsafe { Box::from_raw(handle) };
    guard((), move || drop(boxed));
}

/// Current render generation; changes whenever anything a view draws may
/// have changed. Lock-free while the terminal lives. 0 = terminal missing.
///
/// # Safety
/// `handle` must be null or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn okena_rv_generation(handle: *mut OkenaRvHandle) -> u64 {
    guard(0, || {
        // SAFETY: caller contract.
        let Some(h) = (unsafe { handle_mut(handle) }) else {
            return 0;
        };
        if let Some(counter) = &h.counter {
            let value = counter.load(Ordering::Acquire);
            if value & GENERATION_DEAD == 0 {
                return value;
            }
        }
        h.counter = h.with_holder(TerminalHolder::generation_counter);
        h.counter
            .as_ref()
            .map_or(0, |c| c.load(Ordering::Acquire) & !GENERATION_DEAD)
    })
}

/// Fill the handle's frame with `rows` rows from absolute line `top_line`
/// and return it. The pointer stays valid until the next `okena_rv_snapshot`
/// or `okena_rv_close` on this handle. Null only for a null handle.
///
/// # Safety
/// `handle` must be null or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn okena_rv_snapshot(
    handle: *mut OkenaRvHandle,
    top_line: i64,
    rows: u16,
) -> *const OkenaRvFrame {
    // SAFETY: caller contract.
    let Some(h) = (unsafe { handle_mut(handle) }) else {
        return std::ptr::null();
    };
    let built = guard(false, || {
        let OkenaRvHandle {
            conn_id,
            terminal_id,
            counter,
            frame,
            ..
        } = &mut *h;
        let Some(mgr) = ConnectionManager::try_get() else {
            return false;
        };
        mgr.with_terminal(conn_id, terminal_id, |holder| {
            let gen_counter = holder.generation_counter();
            holder.with_term(|term, track| {
                let generation = gen_counter.load(Ordering::Acquire) & !GENERATION_DEAD;
                frame.build(term, track, generation, top_line, rows, &DARK_THEME);
            });
            *counter = Some(gen_counter);
        })
        .is_some()
    });
    if !built {
        h.frame.build_missing(top_line);
    }
    h.frame.frame_ptr()
}

/// Start a selection at absolute `line` / `col`. `side` is `SIDE_LEFT` or
/// `SIDE_RIGHT` (which half of the cell was touched), `kind` a `SELECT_*`.
///
/// # Safety
/// `handle` must be null or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn okena_rv_selection_start(
    handle: *mut OkenaRvHandle,
    line: i64,
    col: u16,
    side: u8,
    kind: u8,
) {
    guard((), || {
        // SAFETY: caller contract.
        if let Some(h) = unsafe { handle_mut(handle) } {
            h.with_holder(|holder| selection_start(holder, line, col, side, kind));
        }
    })
}

/// Extend the current selection to absolute `line` / `col`.
///
/// # Safety
/// `handle` must be null or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn okena_rv_selection_update(
    handle: *mut OkenaRvHandle,
    line: i64,
    col: u16,
    side: u8,
) {
    guard((), || {
        // SAFETY: caller contract.
        if let Some(h) = unsafe { handle_mut(handle) } {
            h.with_holder(|holder| selection_update(holder, line, col, side));
        }
    })
}

/// Select the word at absolute `line` / `col` (double tap).
///
/// # Safety
/// `handle` must be null or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn okena_rv_select_word(handle: *mut OkenaRvHandle, line: i64, col: u16) {
    // SAFETY: forwarded caller contract.
    unsafe { okena_rv_selection_start(handle, line, col, SIDE_LEFT, SELECT_WORD) }
}

/// The selected text as UTF-8 (NUL-terminated; length without NUL in
/// `*len_out` if non-null), or null when nothing is selected. Owned by the
/// handle, valid until the next `okena_rv_selection_text` or `okena_rv_close`.
///
/// # Safety
/// `handle` must be null or a live handle; `len_out` null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn okena_rv_selection_text(
    handle: *mut OkenaRvHandle,
    len_out: *mut usize,
) -> *const c_char {
    guard(std::ptr::null(), || {
        // SAFETY: caller contract.
        let Some(h) = (unsafe { handle_mut(handle) }) else {
            return std::ptr::null();
        };
        let Some(text) = h.with_holder(TerminalHolder::get_selected_text).flatten() else {
            return std::ptr::null();
        };
        h.selection_text.clear();
        h.selection_text.extend_from_slice(text.as_bytes());
        h.selection_text.push(0);
        if !len_out.is_null() {
            // SAFETY: caller contract.
            unsafe { *len_out = text.len() };
        }
        h.selection_text.as_ptr().cast()
    })
}

/// Clear the selection.
///
/// # Safety
/// `handle` must be null or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn okena_rv_selection_clear(handle: *mut OkenaRvHandle) {
    guard((), || {
        // SAFETY: caller contract.
        if let Some(h) = unsafe { handle_mut(handle) } {
            h.with_holder(TerminalHolder::clear_selection);
        }
    })
}

/// Scroll gesture of `lines` notches (positive = up) at screen cell
/// `col`/`row`. Returns 1 when the notches went to the app (mouse reporting,
/// or alt screen with alternate scroll), 0 when the view should scroll its
/// own position. Never moves alacritty's display offset.
///
/// # Safety
/// `handle` must be null or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn okena_rv_wheel(
    handle: *mut OkenaRvHandle,
    lines: i32,
    col: u16,
    row: u16,
) -> i32 {
    guard(0, || {
        // SAFETY: caller contract.
        let Some(h) = (unsafe { handle_mut(handle) }) else {
            return 0;
        };
        let Some(data) = h
            .with_holder(|holder| wheel_bytes(holder, lines, col, row))
            .flatten()
        else {
            return 0;
        };
        if let Some(mgr) = ConnectionManager::try_get() {
            mgr.send_ws_message(
                &h.conn_id,
                WsClientMessage::SendInput {
                    terminal_id: h.terminal_id.clone(),
                    data,
                },
            );
        }
        1
    })
}

/// Resize only the local grid (no message to the server).
///
/// # Safety
/// `handle` must be null or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn okena_rv_resize_local(handle: *mut OkenaRvHandle, cols: u16, rows: u16) {
    guard((), || {
        // SAFETY: caller contract.
        if let Some(h) = unsafe { handle_mut(handle) }
            && cols > 0
            && rows > 0
        {
            h.with_holder(|holder| holder.resize(cols, rows));
        }
    })
}

/// Resize the local grid and ask the server to resize the PTY.
///
/// # Safety
/// `handle` must be null or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn okena_rv_resize_terminal(
    handle: *mut OkenaRvHandle,
    cols: u16,
    rows: u16,
) {
    guard((), || {
        // SAFETY: caller contract.
        if let Some(h) = unsafe { handle_mut(handle) }
            && cols > 0
            && rows > 0
            && let Some(mgr) = ConnectionManager::try_get()
        {
            mgr.resize_terminal(&h.conn_id, &h.terminal_id, cols, rows);
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(holder: &TerminalHolder, bufs: &mut FrameBuffers, top: i64, rows: u16) {
        let generation = holder.generation_counter().load(Ordering::Acquire);
        holder.with_term(|term, track| bufs.build(term, track, generation, top, rows, &DARK_THEME));
    }

    fn row_text(bufs: &FrameBuffers, r: usize) -> String {
        let cols = usize::from(bufs.header().cols);
        bufs.cells()[r * cols..(r + 1) * cols]
            .iter()
            .filter(|c| c.flags & CELL_WIDE_SPACER == 0)
            .map(|c| char::from_u32(c.codepoint).unwrap_or('?'))
            .collect::<String>()
            .trim_end()
            .to_owned()
    }

    fn feed_lines(holder: &TerminalHolder, range: std::ops::Range<usize>) {
        for i in range {
            holder.process_output(format!("L{i}\r\n").as_bytes());
        }
    }

    #[test]
    fn write_c_buf_truncates_and_terminates() {
        let mut buf = [0x7f as c_char; 4];
        let n = unsafe { write_c_buf("hello", buf.as_mut_ptr(), buf.len()) };
        assert_eq!(n, 5);
        assert_eq!(unsafe { CStr::from_ptr(buf.as_ptr()) }.to_str(), Ok("hel"));
    }

    #[test]
    fn unknown_connection_counts_minus_one() {
        ConnectionManager::init();
        let id = c"no-such-connection";
        assert_eq!(unsafe { okena_rv_terminal_count(id.as_ptr()) }, -1);
        assert_eq!(unsafe { okena_rv_terminal_count(std::ptr::null()) }, -1);
    }

    #[test]
    fn missing_terminal_gives_missing_frame_and_null_handle_is_safe() {
        ConnectionManager::init();
        let h = unsafe { okena_rv_open(c"nope".as_ptr(), c"t".as_ptr()) };
        assert!(!h.is_null());
        let frame = unsafe { &*okena_rv_snapshot(h, 5, 10) };
        assert_eq!(frame.status, STATUS_MISSING);
        assert_eq!(frame.rows, 0);
        assert_eq!(unsafe { okena_rv_generation(h) }, 0);
        assert_eq!(unsafe { okena_rv_wheel(h, 1, 0, 0) }, 0);
        assert!(unsafe { okena_rv_selection_text(h, std::ptr::null_mut()) }.is_null());
        unsafe { okena_rv_close(h) };
        unsafe { okena_rv_close(std::ptr::null_mut()) };
        assert!(unsafe { okena_rv_snapshot(std::ptr::null_mut(), 0, 1) }.is_null());
    }

    #[test]
    fn window_at_an_offset_into_scrollback() {
        let holder = TerminalHolder::new(10, 4);
        feed_lines(&holder, 0..30);
        let mut bufs = FrameBuffers::default();
        snapshot(&holder, &mut bufs, 0, 4);
        let h = *bufs.header();
        // 30 line feeds on a 4-row screen: 27 lines scrolled off.
        assert_eq!(h.screen_top_line, 27);
        assert_eq!(h.history_size, 27);
        assert_eq!(h.first_line, 0);
        assert_eq!(h.screen_lines, 4);
        assert_eq!(h.cursor_line, 30);

        // Read 3 rows from absolute line 10, deep in history.
        snapshot(&holder, &mut bufs, 10, 3);
        assert_eq!(row_text(&bufs, 0), "L10");
        assert_eq!(row_text(&bufs, 2), "L12");
        assert_eq!(bufs.rows()[0].line, 10);
        assert_eq!(bufs.rows()[0].flags & ROW_VOID, 0);
        // Reading does not move the old API's display offset.
        assert_eq!(holder.scroll_info().2, 0);

        // Rows outside the buffer are void.
        snapshot(&holder, &mut bufs, -2, 3);
        assert_eq!(bufs.rows()[0].flags, ROW_VOID);
        assert_eq!(bufs.rows()[1].flags, ROW_VOID);
        assert_eq!(row_text(&bufs, 2), "L0");
    }

    #[test]
    fn absolute_lines_stay_anchored_while_history_grows_and_drops() {
        let holder = TerminalHolder::with_history(10, 4, 5);
        feed_lines(&holder, 0..20);
        let mut bufs = FrameBuffers::default();
        snapshot(&holder, &mut bufs, 0, 1);
        assert_eq!(bufs.header().screen_top_line, 17);
        assert_eq!(bufs.header().first_line, 12);
        snapshot(&holder, &mut bufs, 14, 1);
        assert_eq!(row_text(&bufs, 0), "L14");

        // History is full: lines keep scrolling off, numbers keep counting.
        feed_lines(&holder, 20..23);
        snapshot(&holder, &mut bufs, 14, 1);
        assert_eq!(bufs.header().screen_top_line, 20);
        assert_eq!(bufs.header().first_line, 15);
        assert_eq!(
            bufs.rows()[0].flags,
            ROW_VOID,
            "L14 was dropped by the limit"
        );
        snapshot(&holder, &mut bufs, 18, 1);
        assert_eq!(row_text(&bufs, 0), "L18");

        // One huge chunk (more lines than the limit) is still counted.
        let big: String = (23..5023).map(|i| format!("L{i}\r\n")).collect();
        holder.process_output(big.as_bytes());
        snapshot(&holder, &mut bufs, 5018, 1);
        assert_eq!(bufs.header().screen_top_line, 5020);
        assert_eq!(row_text(&bufs, 0), "L5018");
    }

    #[test]
    fn old_api_display_offset_behaves_as_before() {
        let holder = TerminalHolder::new(10, 4);
        feed_lines(&holder, 0..20);
        holder.scroll(5);
        feed_lines(&holder, 20..23);
        // A scrolled-up view moves with its content (alacritty's behaviour).
        assert_eq!(holder.scroll_info().2, 8);
        holder.scroll_to_bottom();
        feed_lines(&holder, 23..26);
        assert_eq!(holder.scroll_info().2, 0);
    }

    #[test]
    fn alt_screen_round_trip_keeps_counting_and_restores_offset() {
        let holder = TerminalHolder::new(10, 4);
        feed_lines(&holder, 0..10);
        let mut bufs = FrameBuffers::default();
        snapshot(&holder, &mut bufs, 0, 1);
        assert_eq!(bufs.header().screen_top_line, 7);
        // Two more lines, then enter the alt screen in the same chunk.
        holder.process_output(b"L10\r\nL11\r\n\x1b[?1049h");
        snapshot(&holder, &mut bufs, 0, 4);
        assert_ne!(bufs.header().mode & MODE_ALT_SCREEN, 0);
        assert_eq!(bufs.header().history_size, 0);
        holder.process_output(b"alt1\r\nalt2\r\nalt3\r\nalt4\r\nalt5\r\n");
        holder.process_output(b"\x1b[?1049lL12\r\n");
        assert_eq!(holder.scroll_info().2, 0, "old API stays live");
        snapshot(&holder, &mut bufs, 0, 1);
        assert_eq!(bufs.header().mode & MODE_ALT_SCREEN, 0);
        let top = bufs.header().screen_top_line;
        snapshot(&holder, &mut bufs, 11, 1);
        assert_eq!(row_text(&bufs, 0), "L11", "screen_top={top}");
    }

    #[test]
    fn wide_and_combining_characters() {
        let holder = TerminalHolder::new(10, 2);
        holder.process_output("a中b e\u{0301}".as_bytes());
        let mut bufs = FrameBuffers::default();
        snapshot(&holder, &mut bufs, 0, 1);
        let cells = bufs.cells();
        assert_eq!(cells[0].codepoint, 'a' as u32);
        assert_eq!(cells[1].codepoint, '中' as u32);
        assert_ne!(cells[1].flags & CELL_WIDE, 0);
        assert_ne!(cells[2].flags & CELL_WIDE_SPACER, 0);
        assert_eq!(cells[2].codepoint, ' ' as u32);
        assert_eq!(cells[3].codepoint, 'b' as u32);
        assert_eq!(cells[5].codepoint, 'e' as u32);
        assert_eq!(cells[5].extra, 1);
        assert_eq!(bufs.extra_str(cells[5].extra), Some("e\u{0301}"));
        assert_eq!(cells[3].extra, 0);
        assert_eq!(bufs.header().extras_count, 1);
    }

    #[test]
    fn inverse_and_hidden_are_resolved() {
        let holder = TerminalHolder::new(10, 2);
        holder.process_output(b"\x1b[7mX\x1b[0m\x1b[8mY\x1b[0mZ");
        let mut bufs = FrameBuffers::default();
        snapshot(&holder, &mut bufs, 0, 1);
        let c = bufs.cells();
        let h = *bufs.header();
        assert_eq!((c[0].fg, c[0].bg), (h.default_bg, h.default_fg));
        assert_ne!(c[0].flags & CELL_INVERSE, 0);
        assert_eq!(c[1].fg, c[1].bg);
        assert_ne!(c[1].flags & CELL_HIDDEN, 0);
        assert_eq!((c[2].fg, c[2].bg), (h.default_fg, h.default_bg));
    }

    #[test]
    fn generation_bumps_on_output_resize_and_selection() {
        let holder = TerminalHolder::new(10, 4);
        let counter = holder.generation_counter();
        let g0 = counter.load(Ordering::Acquire);
        holder.process_output(b"x");
        let g1 = counter.load(Ordering::Acquire);
        assert_ne!(g0, g1);
        holder.resize(12, 5);
        let g2 = counter.load(Ordering::Acquire);
        assert_ne!(g1, g2);
        selection_start(&holder, 0, 0, SIDE_LEFT, SELECT_SIMPLE);
        assert_ne!(g2, counter.load(Ordering::Acquire));
        drop(holder);
        assert_ne!(counter.load(Ordering::Acquire) & GENERATION_DEAD, 0);
    }

    #[test]
    fn changed_rows_are_detected_by_hash() {
        let holder = TerminalHolder::new(10, 4);
        holder.process_output(b"aaa\r\nbbb\r\nccc");
        let mut bufs = FrameBuffers::default();
        snapshot(&holder, &mut bufs, 0, 4);
        let before: Vec<u64> = bufs.rows().iter().map(|r| r.hash).collect();
        assert!(before.iter().all(|&h| h != 0));
        // Rewrite row 1 only (CUP to row 2, col 1).
        holder.process_output(b"\x1b[2;1HBBB");
        snapshot(&holder, &mut bufs, 0, 4);
        let after: Vec<u64> = bufs.rows().iter().map(|r| r.hash).collect();
        assert_eq!(before[0], after[0]);
        assert_ne!(before[1], after[1]);
        assert_eq!(before[2], after[2]);
        assert_eq!(before[3], after[3]);
        // Colour alone changes the hash.
        holder.process_output(b"\x1b[1;1H\x1b[31ma\x1b[0m");
        snapshot(&holder, &mut bufs, 0, 4);
        assert_ne!(after[0], bufs.rows()[0].hash);
    }

    #[test]
    fn selection_text_across_rows_in_absolute_coordinates() {
        let holder = TerminalHolder::new(10, 4);
        feed_lines(&holder, 0..12);
        let mut bufs = FrameBuffers::default();
        snapshot(&holder, &mut bufs, 0, 1);
        assert_eq!(bufs.header().screen_top_line, 9);
        // From history line 3 col 1 through line 5 col 1.
        selection_start(&holder, 3, 1, SIDE_LEFT, SELECT_SIMPLE);
        selection_update(&holder, 5, 1, SIDE_RIGHT);
        assert_eq!(holder.get_selected_text().as_deref(), Some("3\nL4\nL5"));
        snapshot(&holder, &mut bufs, 0, 1);
        let h = *bufs.header();
        assert_eq!(h.sel_kind, SEL_STREAM);
        assert_eq!((h.sel_start_line, h.sel_start_col), (3, 1));
        assert_eq!((h.sel_end_line, h.sel_end_col), (5, 1));
        // The selection stays on its content while output scrolls.
        feed_lines(&holder, 12..14);
        snapshot(&holder, &mut bufs, 0, 1);
        assert_eq!(bufs.header().sel_start_line, 3);
        assert_eq!(holder.get_selected_text().as_deref(), Some("3\nL4\nL5"));

        selection_start(&holder, 4, 0, SIDE_LEFT, SELECT_WORD);
        assert_eq!(holder.get_selected_text().as_deref(), Some("L4"));
        holder.clear_selection();
        snapshot(&holder, &mut bufs, 0, 1);
        assert_eq!(bufs.header().sel_kind, SEL_NONE);
    }

    #[test]
    fn mode_bits_follow_the_app() {
        let holder = TerminalHolder::new(10, 4);
        let mut bufs = FrameBuffers::default();
        snapshot(&holder, &mut bufs, 0, 1);
        assert_eq!(bufs.header().mode & MODE_MOUSE, 0);
        holder.process_output(b"\x1b[?1000h");
        snapshot(&holder, &mut bufs, 0, 1);
        assert_ne!(bufs.header().mode & MODE_MOUSE, 0);
        holder.process_output(b"\x1b[?1000l\x1b[?1049h\x1b[?1007h\x1b[?1h");
        snapshot(&holder, &mut bufs, 0, 1);
        let mode = bufs.header().mode;
        assert_eq!(mode & MODE_MOUSE, 0);
        assert_ne!(mode & MODE_ALT_SCREEN, 0);
        assert_ne!(mode & MODE_ALTERNATE_SCROLL, 0);
        assert_ne!(mode & MODE_APP_CURSOR, 0);
    }

    #[test]
    fn wheel_goes_to_the_app_only_when_it_asked() {
        let holder = TerminalHolder::new(10, 4);
        assert_eq!(wheel_bytes(&holder, 2, 0, 0), None);
        holder.process_output(b"\x1b[?1049h\x1b[?1007h");
        assert_eq!(
            wheel_bytes(&holder, -2, 0, 0),
            Some(b"\x1b[B\x1b[B".to_vec())
        );
        holder.process_output(b"\x1b[?1h");
        assert_eq!(wheel_bytes(&holder, 1, 0, 0), Some(b"\x1bOA".to_vec()));
        holder.process_output(b"\x1b[?1000h\x1b[?1006h");
        assert_eq!(
            wheel_bytes(&holder, 1, 4, 2),
            Some(b"\x1b[<64;5;3M".to_vec())
        );
        assert_eq!(holder.scroll_info().2, 0);
    }

    #[test]
    fn frame_offsets_are_consistent() {
        let holder = TerminalHolder::new(7, 3);
        holder.process_output("e\u{0301}".as_bytes());
        let mut bufs = FrameBuffers::default();
        snapshot(&holder, &mut bufs, 0, 3);
        let h = *bufs.header();
        assert_eq!(h.abi_version, ABI_VERSION);
        assert_eq!(h.header_size, 136);
        assert_eq!(h.rows_offset, 136);
        assert_eq!(h.cells_offset, 136 + 3 * 24);
        assert_eq!(h.extras_offset, h.cells_offset + 3 * 7 * 16);
        assert_eq!(h.text_offset, h.extras_offset + 8);
        assert_eq!(h.total_size, h.text_offset + h.text_len);
        assert_eq!(h.text_len, 3);
        assert_eq!(h.extras_offset % 8, 0);
    }

    /// `cargo test -p okena-mobile-ffi --release -- --ignored snapshot_bench --nocapture`
    #[test]
    #[ignore]
    fn snapshot_bench() {
        let holder = TerminalHolder::new(60, 50);
        let mut out = String::new();
        for i in 0..2000 {
            out.push_str(&format!(
                "\x1b[3{}m{:>4} the quick brown fox \x1b[1mjumps\x1b[0m over e\u{0301} 中文 {}\r\n",
                i % 8,
                i,
                "x".repeat(i % 20)
            ));
        }
        holder.process_output(out.as_bytes());
        let mut bufs = FrameBuffers::default();
        let iters = 5000;
        let start = std::time::Instant::now();
        for i in 0..iters {
            snapshot(&holder, &mut bufs, 1000 + (i % 500) as i64, 50);
        }
        let per = start.elapsed() / iters;
        println!("snapshot 60x50: {per:?} per frame");
        assert_eq!(bufs.header().cols, 60);
    }
}
