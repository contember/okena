/*
 * okena_render.h — C ABI between the Rust core (crates/okena-mobile-ffi,
 * src/render.rs) and the native terminal views (Swift on iOS, C++/JNI and
 * Kotlin on Android).
 *
 * This header is the single contract. Every function is implemented in
 * render.rs with `#[unsafe(no_mangle)] extern "C"`, every struct mirrors a
 * `#[repr(C)]` struct there (Rust const-asserts the same sizes and offsets
 * as the _Static_asserts below). Keep both in sync and bump
 * OKENA_RV_ABI_VERSION on any incompatible change.
 *
 * ── Linking (one Rust copy) ───────────────────────────────────────────────
 * The functions live in the same Rust library as the uniffi/JSI module, so
 * they see the same ConnectionManager singleton and the connections that JS
 * opened. Never link a second copy of the Rust library:
 *   - iOS: the static library in OkenaMobileFfiFramework.xcframework is
 *     linked once into the app by the OkenaMobileFfi pod. A Swift pod only
 *     adds this directory to SWIFT_INCLUDE_PATHS and does `import OkenaRender`
 *     (module.modulemap next to this file). Symbols resolve at app link time.
 *   - Android: libokena_mobile_ffi.a is linked into libokena-mobile-ffi.so.
 *     The JNI shim crates/okena-mobile-ffi/jni/okena_render_jni.cpp is
 *     compiled into that same .so (added by
 *     mobile/rn/scripts/fix-android-bindings.cjs after `ubrn build android`).
 *     Kotlin talks to it through the class
 *         com.okena.terminalview.RustBridge
 *     whose `external` methods are listed in okena_render_jni.cpp.
 *
 * ── Threading and lifetime ────────────────────────────────────────────────
 * Free functions are thread-safe. A handle belongs to one thread at a time
 * (the view's UI/render thread). Pointers returned through a handle (frame,
 * selection text) stay valid until the next call of the same function on that
 * handle, or okena_rv_close. Nothing is ever freed by the caller except the
 * handle itself (okena_rv_close). No function unwinds or aborts on bad input:
 * a null handle or unknown terminal gives the documented fallback.
 *
 * Input strings are NUL-terminated UTF-8, borrowed for the call only.
 *
 * ── Line addressing ───────────────────────────────────────────────────────
 * Lines are addressed by an ABSOLUTE line number (int64_t) that stays the
 * same for a given line of output while more output scrolls in:
 *
 *   screen_top_line  absolute number of the top row of the live screen. It
 *                    equals the number of lines that have ever scrolled from
 *                    the primary screen into history (monotonic while output
 *                    streams), so it grows by one per scrolled line.
 *   history_size     lines of scrollback currently kept.
 *   first_line       = screen_top_line - history_size: the oldest line still
 *                    readable. When the scrollback limit (10 000 lines) is
 *                    reached, old lines are dropped and first_line grows.
 *   last line        = screen_top_line + screen_lines - 1 (bottom screen row).
 *
 * A view anchors a scrolled-up position by remembering the absolute line at
 * its top edge: the same number keeps showing the same text. A view at the
 * bottom ("live tail") follows screen_top_line. Content height for a scroll
 * view is (history_size + screen_lines) rows; row 0 of the content is
 * first_line. When first_line grows (limit reached), shift the scroll offset
 * by the difference to stay anchored.
 *
 * Exceptions, signalled so the view can re-anchor (e.g. keep its distance
 * from the bottom):
 *   - reflow_epoch changes on every resize. Reflow rewraps lines; the oldest
 *     line keeps its number but others may move.
 *   - Alt screen (MODE_ALT_SCREEN): the alt grid has no history, so
 *     history_size is 0 and first_line == screen_top_line. The primary
 *     screen's numbering resumes unchanged when the app leaves the alt screen.
 *   - clear-scrollback (CSI 3J) / reset (RIS) drop history: first_line jumps
 *     up to screen_top_line.
 *
 * Reading any window never moves alacritty's own display offset (used only by
 * the old uniffi API), so the native view owns its scroll position.
 *
 * ── Change detection ──────────────────────────────────────────────────────
 * Poll okena_rv_generation once per vsync (lock-free). Only if it differs
 * from the last frame's `generation` (or the view scrolled) take a snapshot.
 * Per row, (line, hash) identifies the row's pixels apart from the cursor and
 * selection overlays: redraw a cached row bitmap only when either changes.
 * (alacritty's Term::damage() is viewport-relative, single-consumer and goes
 * "full" on every scroll, so it is not used.)
 */
#ifndef OKENA_RENDER_H
#define OKENA_RENDER_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define OKENA_RV_ABI_VERSION 1u

/* ── Frame layout ──────────────────────────────────────────────────────────
 * okena_rv_snapshot returns a pointer to ONE contiguous, Rust-owned blob
 * (8-byte aligned, little-endian, native ABI on all supported targets):
 *
 *   offset 0              OkenaRvFrame   header (136 bytes)
 *   rows_offset           OkenaRvRow     [rows]           24 bytes each
 *   cells_offset          OkenaRvCell    [rows * cols]    16 bytes each,
 *                                        row-major: cell (r, c) is at
 *                                        index r * cols + c
 *   extras_offset         OkenaRvExtra   [extras_count]   8 bytes each
 *   text_offset           uint8_t        [text_len]       UTF-8 text table
 *   total_size            end of blob
 *
 * All offsets are bytes from the start of the frame. rows_offset,
 * cells_offset and extras_offset are multiples of 8. Kotlin can wrap the whole
 * blob in one direct ByteBuffer (ByteOrder.LITTLE_ENDIAN) and read fields at
 * the offsets documented below; Swift/C use the structs and the inline
 * accessors at the end of this header.
 */

/* OkenaRvFrame.status */
#define OKENA_RV_STATUS_OK 0u
/* Terminal not (yet) known for this connection: rows == 0, cols == 0. */
#define OKENA_RV_STATUS_MISSING 1u

/* OkenaRvCell.flags */
#define OKENA_RV_CELL_BOLD               (1u << 0)
#define OKENA_RV_CELL_ITALIC             (1u << 1)
#define OKENA_RV_CELL_UNDERLINE          (1u << 2)  /* any underline style */
#define OKENA_RV_CELL_STRIKE             (1u << 3)
#define OKENA_RV_CELL_INVERSE            (1u << 4)  /* informative; fg/bg already swapped */
#define OKENA_RV_CELL_DIM                (1u << 5)  /* renderer applies dimming */
#define OKENA_RV_CELL_WIDE               (1u << 6)  /* glyph spans this and the next cell */
#define OKENA_RV_CELL_WIDE_SPACER        (1u << 7)  /* 2nd half of a wide glyph: draw bg only */
#define OKENA_RV_CELL_LEADING_WIDE_SPACER (1u << 8) /* last column padding before a wrapped wide glyph: bg only */
#define OKENA_RV_CELL_HIDDEN             (1u << 9)  /* informative; fg already == bg */
#define OKENA_RV_CELL_DOUBLE_UNDERLINE   (1u << 10)
#define OKENA_RV_CELL_UNDERCURL          (1u << 11)
#define OKENA_RV_CELL_DOTTED_UNDERLINE   (1u << 12)
#define OKENA_RV_CELL_DASHED_UNDERLINE   (1u << 13)

/* OkenaRvRow.flags */
#define OKENA_RV_ROW_VOID    (1u << 0) /* outside [first_line, last line]: blank cells, hash 0 */
#define OKENA_RV_ROW_WRAPPED (1u << 1) /* soft-wrapped into the next row */

/* OkenaRvFrame.mode */
#define OKENA_RV_MODE_MOUSE            (1u << 0) /* app requested mouse reporting (1000/1002/1003) */
#define OKENA_RV_MODE_ALT_SCREEN       (1u << 1)
#define OKENA_RV_MODE_ALTERNATE_SCROLL (1u << 2) /* 1007 */
#define OKENA_RV_MODE_APP_CURSOR       (1u << 3) /* DECCKM */
#define OKENA_RV_MODE_BRACKETED_PASTE  (1u << 4)
#define OKENA_RV_MODE_SGR_MOUSE        (1u << 5)
#define OKENA_RV_MODE_SHOW_CURSOR      (1u << 6)

/* OkenaRvFrame.cursor_shape */
#define OKENA_RV_CURSOR_BLOCK        0u
#define OKENA_RV_CURSOR_UNDERLINE    1u
#define OKENA_RV_CURSOR_BEAM         2u
#define OKENA_RV_CURSOR_HOLLOW_BLOCK 3u

/* OkenaRvFrame.sel_kind */
#define OKENA_RV_SEL_NONE   0u
#define OKENA_RV_SEL_STREAM 1u /* from (start_line,start_col) to (end_line,end_col), inclusive, wrapping rows */
#define OKENA_RV_SEL_BLOCK  2u /* rectangle: rows start..end, columns start_col..end_col inclusive */

/* okena_rv_selection_start kind */
#define OKENA_RV_SELECT_SIMPLE 0u
#define OKENA_RV_SELECT_WORD   1u
#define OKENA_RV_SELECT_LINES  2u
#define OKENA_RV_SELECT_BLOCK  3u

/* side: which half of the touched cell (affects whether it is included) */
#define OKENA_RV_SIDE_LEFT  0u
#define OKENA_RV_SIDE_RIGHT 1u

/* One cell. 16 bytes, align 4. */
typedef struct OkenaRvCell {
    uint32_t codepoint; /*  0  base Unicode scalar; ' ' for empty and spacer cells */
    uint32_t fg;        /*  4  ARGB 0xAARRGGBB, inverse + hidden resolved, theme applied */
    uint32_t bg;        /*  8  ARGB, inverse resolved */
    uint16_t flags;     /* 12  OKENA_RV_CELL_* */
    uint16_t extra;     /* 14  0 = none; else 1-based index into extras: the
                               full grapheme (base + combining marks). Draw
                               that string instead of `codepoint`. */
} OkenaRvCell;

/* A string in the text table. 8 bytes, align 4. */
typedef struct OkenaRvExtra {
    uint32_t offset; /* 0  byte offset into the text table */
    uint32_t len;    /* 4  byte length, UTF-8, not NUL-terminated */
} OkenaRvExtra;

/* Row metadata. 24 bytes, align 8. */
typedef struct OkenaRvRow {
    int64_t line;      /*  0  absolute line number (top_line + row index) */
    uint64_t hash;     /*  8  content hash; 0 iff ROW_VOID */
    uint32_t flags;    /* 16  OKENA_RV_ROW_* */
    uint32_t reserved; /* 20 */
} OkenaRvRow;

/* Frame header. 136 bytes, align 8. */
typedef struct OkenaRvFrame {
    uint32_t abi_version;     /*   0  == OKENA_RV_ABI_VERSION */
    uint32_t header_size;     /*   4  sizeof(OkenaRvFrame) */
    uint32_t total_size;      /*   8  bytes in the whole blob */
    uint32_t status;          /*  12  OKENA_RV_STATUS_* */
    uint64_t generation;      /*  16  generation this frame reflects */
    int64_t top_line;         /*  24  absolute line of row 0 (as requested) */
    int64_t first_line;       /*  32  oldest readable absolute line */
    int64_t screen_top_line;  /*  40  absolute line of the top screen row */
    int64_t cursor_line;      /*  48  absolute line of the cursor */
    int64_t sel_start_line;   /*  56  absolute; valid if sel_kind != NONE */
    int64_t sel_end_line;     /*  64  absolute, inclusive */
    uint32_t history_size;    /*  72  scrollback lines kept */
    uint32_t mode;            /*  76  OKENA_RV_MODE_* */
    uint32_t reflow_epoch;    /*  80  bumped by every resize */
    uint32_t default_fg;      /*  84  theme foreground, ARGB */
    uint32_t default_bg;      /*  88  theme background, ARGB (also void rows) */
    uint16_t cols;            /*  92  cells per row */
    uint16_t rows;            /*  94  rows in this frame */
    uint16_t screen_lines;    /*  96  rows of the live screen */
    uint16_t cursor_col;      /*  98 */
    uint16_t sel_start_col;   /* 100 */
    uint16_t sel_end_col;     /* 102  inclusive */
    uint8_t cursor_shape;     /* 104  OKENA_RV_CURSOR_* */
    uint8_t cursor_visible;   /* 105  0/1: SHOW_CURSOR and not hidden style */
    uint8_t sel_kind;         /* 106  OKENA_RV_SEL_* */
    uint8_t reserved0;        /* 107 */
    uint32_t rows_offset;     /* 108 */
    uint32_t cells_offset;    /* 112 */
    uint32_t extras_offset;   /* 116 */
    uint32_t extras_count;    /* 120 */
    uint32_t text_offset;     /* 124 */
    uint32_t text_len;        /* 128 */
    uint32_t reserved1;       /* 132 */
} OkenaRvFrame;

#ifdef __cplusplus
#define OKENA_RV_STATIC_ASSERT(c, m) static_assert(c, m)
#else
#define OKENA_RV_STATIC_ASSERT(c, m) _Static_assert(c, m)
#endif
OKENA_RV_STATIC_ASSERT(sizeof(OkenaRvCell) == 16, "OkenaRvCell layout");
OKENA_RV_STATIC_ASSERT(sizeof(OkenaRvExtra) == 8, "OkenaRvExtra layout");
OKENA_RV_STATIC_ASSERT(sizeof(OkenaRvRow) == 24, "OkenaRvRow layout");
OKENA_RV_STATIC_ASSERT(sizeof(OkenaRvFrame) == 136, "OkenaRvFrame layout");
OKENA_RV_STATIC_ASSERT(offsetof(OkenaRvFrame, cols) == 92, "OkenaRvFrame layout");
OKENA_RV_STATIC_ASSERT(offsetof(OkenaRvFrame, rows_offset) == 108, "OkenaRvFrame layout");
OKENA_RV_STATIC_ASSERT(offsetof(OkenaRvFrame, text_len) == 128, "OkenaRvFrame layout");

/* Opaque per-view handle. */
typedef struct OkenaRvHandle OkenaRvHandle;

/* ── Diagnostics ─────────────────────────────────────────────────────────── */

/* ABI version of the linked library. Compare with OKENA_RV_ABI_VERSION. */
uint32_t okena_rv_abi_version(void);

/*
 * Number of terminals the connection `conn_id` currently mirrors, or -1 when
 * the connection does not exist (or init_app has not run yet).
 */
int32_t okena_rv_terminal_count(const char *conn_id);

/*
 * Diagnostic listing of the live connections, one line each:
 *   "<conn_id>\t<terminal_count>\t<term_id>,<term_id>,...\n"
 * Writes at most cap-1 bytes plus a NUL into buf (buf may be NULL when cap is
 * 0) and returns the full length without the NUL. If the return value is >=
 * cap, the text was truncated; call again with a larger buffer.
 */
size_t okena_rv_debug_connections(char *buf, size_t cap);

/* ── View handle ─────────────────────────────────────────────────────────── */

/*
 * Open a handle for terminal `terminal_id` (unprefixed, as JS uses it) of
 * connection `conn_id`. Succeeds even if the terminal does not exist yet;
 * snapshots then report OKENA_RV_STATUS_MISSING. NULL only for NULL or
 * invalid UTF-8 ids. Close with okena_rv_close.
 */
OkenaRvHandle *okena_rv_open(const char *conn_id, const char *terminal_id);

/* Free the handle and its frame. NULL is a no-op. */
void okena_rv_close(OkenaRvHandle *handle);

/*
 * Render generation: changes whenever output, resize or selection may have
 * changed what the view draws. Lock-free in the steady state; call once per
 * vsync. Returns 0 while the terminal is missing. Values are globally unique,
 * so a terminal replaced under the same id never repeats an old value.
 */
uint64_t okena_rv_generation(OkenaRvHandle *handle);

/*
 * Fill the handle's frame with `rows` rows (at most 1024) starting at
 * absolute line `top_line`, and return it. Rows outside the readable range
 * come back as OKENA_RV_ROW_VOID. The returned frame is owned by the handle
 * and stays valid (and unchanged) until the next okena_rv_snapshot or
 * okena_rv_close on this handle. Never NULL for a non-NULL handle.
 * Cost: ~one memcpy of rows*cols*16 bytes plus colour resolution; the cells
 * are not copied again on the native side.
 */
const OkenaRvFrame *okena_rv_snapshot(OkenaRvHandle *handle, int64_t top_line, uint16_t rows);

/* ── Selection (absolute lines, buffer columns) ──────────────────────────── */

/* Start a selection at (line, col); `side` OKENA_RV_SIDE_*, `kind`
 * OKENA_RV_SELECT_*. Positions are clamped to the readable range. */
void okena_rv_selection_start(OkenaRvHandle *handle, int64_t line, uint16_t col,
                              uint8_t side, uint8_t kind);

/* Extend the current selection to (line, col). No-op without a selection. */
void okena_rv_selection_update(OkenaRvHandle *handle, int64_t line, uint16_t col,
                               uint8_t side);

/* Select the word at (line, col) (double tap). */
void okena_rv_select_word(OkenaRvHandle *handle, int64_t line, uint16_t col);

/*
 * The selected text, UTF-8 and NUL-terminated (length without NUL written to
 * *len_out when len_out is non-NULL), or NULL when nothing is selected. Owned
 * by the handle; valid until the next okena_rv_selection_text or close.
 */
const char *okena_rv_selection_text(OkenaRvHandle *handle, size_t *len_out);

/* Clear the selection. */
void okena_rv_selection_clear(OkenaRvHandle *handle);

/* The selection follows its text while output scrolls (alacritty rotates it),
 * and is cleared by alt-screen switches. Read its current range from the
 * frame (sel_kind, sel_*). */

/* ── Input and sizing ────────────────────────────────────────────────────── */

/*
 * Scroll gesture of `lines` notches (positive = up/back in history) at the
 * screen cell (col, row) (row 0 = top screen row). Returns 1 when the notches
 * were sent to the app (MODE_MOUSE: wheel reports; MODE_ALT_SCREEN with
 * MODE_ALTERNATE_SCROLL: arrow keys), 0 when the view should move its own
 * scroll position instead. Never changes alacritty's display offset.
 */
int32_t okena_rv_wheel(OkenaRvHandle *handle, int32_t lines, uint16_t col, uint16_t row);

/* Resize only the local grid (no message to the server). 0 sizes ignored. */
void okena_rv_resize_local(OkenaRvHandle *handle, uint16_t cols, uint16_t rows);

/* Resize the local grid and send a resize to the server PTY. Debounce it
 * (the Skia view uses 200 ms) and use resize_local for live feedback. */
void okena_rv_resize_terminal(OkenaRvHandle *handle, uint16_t cols, uint16_t rows);

/* ── Inline accessors (Swift imports these) ──────────────────────────────── */

static inline const OkenaRvRow *okena_rv_frame_rows(const OkenaRvFrame *f) {
    return (const OkenaRvRow *)((const uint8_t *)f + f->rows_offset);
}

static inline const OkenaRvCell *okena_rv_frame_cells(const OkenaRvFrame *f) {
    return (const OkenaRvCell *)((const uint8_t *)f + f->cells_offset);
}

static inline const OkenaRvCell *okena_rv_frame_row_cells(const OkenaRvFrame *f, uint16_t row) {
    return okena_rv_frame_cells(f) + (size_t)row * f->cols;
}

/* Grapheme of a cell with extra != 0: pointer to UTF-8 bytes and *len.
 * NULL for extra == 0 or an out-of-range index. */
static inline const char *okena_rv_frame_extra(const OkenaRvFrame *f, uint16_t extra,
                                               uint32_t *len) {
    if (extra == 0 || extra > f->extras_count) return 0;
    const OkenaRvExtra *e =
        (const OkenaRvExtra *)((const uint8_t *)f + f->extras_offset) + (extra - 1);
    if (len) *len = e->len;
    return (const char *)f + f->text_offset + e->offset;
}

#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* OKENA_RENDER_H */
