/**
 * Smoke test for the packed visible-cell decoder — the contract the Rust
 * `get_visible_cells_packed` encoder (crates/okena-mobile-ffi) must satisfy.
 */
import {
  decodeCells,
  decodeCellsView,
  argbToChannels,
  argbToCss,
  FLAG_BOLD,
  HEADER_BYTES,
  CELL_BYTES,
} from '../src/native/cells';

interface Cell {
  codepoint: number;
  fg: number;
  bg: number;
  flags: number;
}

/** Build a packed buffer exactly as the Rust encoder does (LE throughout). */
function packGrid(cols: number, rows: number, cells: Cell[]): ArrayBuffer {
  const buf = new ArrayBuffer(HEADER_BYTES + cols * rows * CELL_BYTES);
  const v = new DataView(buf);
  v.setUint16(0, cols, true);
  v.setUint16(2, rows, true);
  let off = HEADER_BYTES;
  for (const c of cells) {
    v.setUint32(off + 0, c.codepoint, true);
    v.setUint32(off + 4, c.fg, true);
    v.setUint32(off + 8, c.bg, true);
    v.setUint8(off + 12, c.flags);
    off += CELL_BYTES;
  }
  return buf;
}

const SAMPLE: Cell[] = [
  {codepoint: 0x41 /* 'A' */, fg: 0xff112233, bg: 0xff000000, flags: FLAG_BOLD},
  {codepoint: 0x20 /* ' ' */, fg: 0, bg: 0, flags: 0},
];

describe('decodeCells', () => {
  it('decodes header + cells round-trip from the packed format', () => {
    const grid = decodeCells(packGrid(2, 1, SAMPLE));
    expect(grid.cols).toBe(2);
    expect(grid.rows).toBe(1);
    expect(grid.cells).toHaveLength(2);
    expect(grid.cells[0]).toEqual({
      codepoint: 0x41,
      fg: 0xff112233,
      bg: 0xff000000,
      flags: FLAG_BOLD,
    });
    expect(grid.cells[1].codepoint).toBe(0x20);
  });

  it('throws on a truncated buffer', () => {
    const short = packGrid(2, 1, SAMPLE).slice(0, 10);
    expect(() => decodeCells(short)).toThrow(RangeError);
  });
});

describe('PackedCells (zero-alloc view)', () => {
  it('exposes per-cell getters matching decodeCells', () => {
    const view = decodeCellsView(packGrid(2, 1, SAMPLE));
    expect(view.count).toBe(2);
    expect(view.codepoint(0)).toBe(0x41);
    expect(view.char(0)).toBe('A');
    expect(view.flags(0) & FLAG_BOLD).toBe(FLAG_BOLD);
    expect(view.isBlank(1)).toBe(true);
  });
});

describe('ARGB helpers', () => {
  it('unpacks channels from 0xAARRGGBB', () => {
    expect(argbToChannels(0xff112233)).toEqual({a: 255, r: 0x11, g: 0x22, b: 0x33});
  });

  it('formats an rgba() string', () => {
    expect(argbToCss(0xff112233)).toBe('rgba(17, 34, 51, 1.0000)');
  });
});
