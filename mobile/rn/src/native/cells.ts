/**
 * cells.ts — decoder for the packed visible-cell buffer.
 *
 * SCAFFOLD NOTE: this matches a binary format produced by a Rust FFI function
 * (`getVisibleCellsPacked`) that is being ADDED to `crates/okena-mobile-ffi`
 * as part of the migration (it does not exist in `mobile/native` yet). This
 * file is the authoritative spec of that wire format; the Rust encoder must
 * produce exactly these bytes.
 *
 * Format (little-endian throughout):
 *
 *   ┌─ header (4 bytes) ─────────────────────────────────────────────┐
 *   │ cols : u16                                                      │
 *   │ rows : u16                                                      │
 *   ├─ then cols*rows cells, row-major, 13 bytes each ───────────────┤
 *   │ codepoint : u32   Unicode scalar value (0x20 == space)         │
 *   │ fg        : u32   ARGB packed: 0xAARRGGBB                       │
 *   │ bg        : u32   ARGB packed: 0xAARRGGBB                       │
 *   │ flags     : u8    bitmask (see FLAG_* below)                    │
 *   └────────────────────────────────────────────────────────────────┘
 *
 * Total length = 4 + cols*rows*13 bytes.
 */

// ── Style flag bits (must match Rust `CellData.flags` in api/terminal.rs) ──

export const FLAG_BOLD = 1;
export const FLAG_ITALIC = 2;
export const FLAG_UNDERLINE = 4;
export const FLAG_STRIKETHROUGH = 8;
export const FLAG_INVERSE = 16;
export const FLAG_DIM = 32;

/**
 * Flags that affect glyph styling (everything except INVERSE, which is handled
 * separately by swapping fg/bg). Mirrors `_kStyleMask` in terminal_painter.dart.
 */
export const FLAG_STYLE_MASK =
  FLAG_BOLD | FLAG_ITALIC | FLAG_UNDERLINE | FLAG_STRIKETHROUGH | FLAG_DIM;

// ── Byte-layout constants ──────────────────────────────────────────────────

/** Header size in bytes (cols:u16 + rows:u16). */
export const HEADER_BYTES = 4;
/** Size of one packed cell in bytes (u32 + u32 + u32 + u8). */
export const CELL_BYTES = 13;

const OFFSET_CODEPOINT = 0;
const OFFSET_FG = 4;
const OFFSET_BG = 8;
const OFFSET_FLAGS = 12;

// ── ARGB helpers ────────────────────────────────────────────────────────────

export interface Argb {
  a: number;
  r: number;
  g: number;
  b: number;
}

/** Unpack a `0xAARRGGBB` u32 into channel components (0–255 each). */
export function argbToChannels(argb: number): Argb {
  // `>>> 0` keeps it an unsigned 32-bit value.
  const v = argb >>> 0;
  return {
    a: (v >>> 24) & 0xff,
    r: (v >>> 16) & 0xff,
    g: (v >>> 8) & 0xff,
    b: v & 0xff,
  };
}

/**
 * Convert a `0xAARRGGBB` u32 to a CSS `rgba()` string. Handy for non-Skia
 * consumers / tests; the Skia renderer builds `Float32Array` colors directly.
 */
export function argbToCss(argb: number): string {
  const { a, r, g, b } = argbToChannels(argb);
  return `rgba(${r}, ${g}, ${b}, ${(a / 255).toFixed(4)})`;
}

// ── Decoded representations ──────────────────────────────────────────────────

/** A single decoded cell. */
export interface DecodedCell {
  /** Unicode scalar value; `0x20` is a space / blank cell. */
  codepoint: number;
  /** Foreground, ARGB packed `0xAARRGGBB`. */
  fg: number;
  /** Background, ARGB packed `0xAARRGGBB`. */
  bg: number;
  /** Style flags bitmask. */
  flags: number;
}

/** Object form of a decoded grid (convenient; allocates per cell). */
export interface DecodedGrid {
  cols: number;
  rows: number;
  cells: DecodedCell[];
}

/**
 * Decode the packed buffer into a `{ cols, rows, cells }` object.
 *
 * Allocates one `DecodedCell` object per cell — fine for tests and incidental
 * use. For the per-frame render hot path prefer `decodeCellsView`, which wraps
 * the same bytes in typed-array views with zero per-cell allocation.
 */
export function decodeCells(buf: ArrayBuffer): DecodedGrid {
  const view = new DataView(buf);
  const cols = view.getUint16(0, /* littleEndian */ true);
  const rows = view.getUint16(2, true);
  const count = cols * rows;

  const expected = HEADER_BYTES + count * CELL_BYTES;
  if (buf.byteLength < expected) {
    throw new RangeError(
      `packed cell buffer too short: have ${buf.byteLength} bytes, ` +
        `need ${expected} for ${cols}x${rows}`,
    );
  }

  const cells: DecodedCell[] = new Array(count);
  let off = HEADER_BYTES;
  for (let i = 0; i < count; i++) {
    cells[i] = {
      codepoint: view.getUint32(off + OFFSET_CODEPOINT, true),
      fg: view.getUint32(off + OFFSET_FG, true),
      bg: view.getUint32(off + OFFSET_BG, true),
      flags: view.getUint8(off + OFFSET_FLAGS),
    };
    off += CELL_BYTES;
  }

  return { cols, rows, cells };
}

/**
 * Zero-allocation accessor over the packed buffer for the render hot path.
 *
 * Holds a single `DataView` and exposes per-cell field getters indexed by
 * `i = row * cols + col`. No `DecodedCell` objects are created, so painting a
 * full frame allocates nothing beyond this wrapper.
 */
export class PackedCells {
  readonly cols: number;
  readonly rows: number;
  readonly count: number;
  private readonly view: DataView;

  constructor(buf: ArrayBuffer) {
    const view = new DataView(buf);
    this.cols = view.getUint16(0, true);
    this.rows = view.getUint16(2, true);
    this.count = this.cols * this.rows;
    const expected = HEADER_BYTES + this.count * CELL_BYTES;
    if (buf.byteLength < expected) {
      throw new RangeError(
        `packed cell buffer too short: have ${buf.byteLength} bytes, ` +
          `need ${expected} for ${this.cols}x${this.rows}`,
      );
    }
    this.view = view;
  }

  private cellOffset(i: number): number {
    return HEADER_BYTES + i * CELL_BYTES;
  }

  codepoint(i: number): number {
    return this.view.getUint32(this.cellOffset(i) + OFFSET_CODEPOINT, true);
  }

  fg(i: number): number {
    return this.view.getUint32(this.cellOffset(i) + OFFSET_FG, true);
  }

  bg(i: number): number {
    return this.view.getUint32(this.cellOffset(i) + OFFSET_BG, true);
  }

  flags(i: number): number {
    return this.view.getUint8(this.cellOffset(i) + OFFSET_FLAGS);
  }

  /** The character at cell `i` as a JS string (handles astral codepoints). */
  char(i: number): string {
    return String.fromCodePoint(this.codepoint(i));
  }

  /** True if the cell is empty (space). */
  isBlank(i: number): boolean {
    const cp = this.codepoint(i);
    return cp === 0x20 || cp === 0;
  }
}

/**
 * Wrap a packed buffer in a {@link PackedCells} view (zero per-cell alloc).
 * The render path should use this; `decodeCells` is the convenience form.
 */
export function decodeCellsView(buf: ArrayBuffer): PackedCells {
  return new PackedCells(buf);
}
