/// <reference types="jest" />
// tsconfig pins `types` to react/react-native, so pull jest globals in here.

import { TOUCH_SLOP_PX, accumulateScroll, exceedsTouchSlop, pointToCell } from './terminalScroll';

describe('accumulateScroll', () => {
  it('turns a downward finger drag into lines up into scrollback', () => {
    expect(accumulateScroll(0, 35, 10)).toEqual({ lines: 3, remainderPx: 5 });
  });

  it('turns an upward finger drag into lines back down', () => {
    expect(accumulateScroll(0, -25, 10)).toEqual({ lines: -2, remainderPx: -5 });
  });

  it('carries sub-line movement so slow drags still scroll', () => {
    let accum = 0;
    let lines = 0;
    for (let i = 0; i < 10; i++) {
      const step = accumulateScroll(accum, 3, 10);
      accum = step.remainderPx;
      lines += step.lines;
    }
    expect(lines).toBe(3);
    expect(accum).toBeCloseTo(0);
  });

  it('cancels out when the finger reverses within a line', () => {
    const first = accumulateScroll(0, 6, 10);
    expect(first.lines).toBe(0);
    expect(accumulateScroll(first.remainderPx, -6, 10)).toEqual({ lines: 0, remainderPx: 0 });
  });

  it('does nothing until the cell height is known', () => {
    expect(accumulateScroll(0, 100, 0)).toEqual({ lines: 0, remainderPx: 0 });
    expect(accumulateScroll(0, 100, Number.NaN)).toEqual({ lines: 0, remainderPx: 0 });
  });
});

describe('exceedsTouchSlop', () => {
  it('keeps small jitter a tap', () => {
    expect(exceedsTouchSlop(3, 4)).toBe(false);
    expect(exceedsTouchSlop(0, TOUCH_SLOP_PX)).toBe(false);
  });

  it('turns real movement into a drag', () => {
    expect(exceedsTouchSlop(0, TOUCH_SLOP_PX + 1)).toBe(true);
    expect(exceedsTouchSlop(-7, -7)).toBe(true);
  });
});

describe('pointToCell', () => {
  const grid = { cols: 40, rows: 20, cellWidth: 8, cellHeight: 16 };

  it('maps a point to the cell under it', () => {
    expect(pointToCell(17, 33, grid)).toEqual({ col: 2, row: 2 });
  });

  it('clamps points outside the grid', () => {
    expect(pointToCell(-5, -5, grid)).toEqual({ col: 0, row: 0 });
    expect(pointToCell(10_000, 10_000, grid)).toEqual({ col: 39, row: 19 });
  });

  it('falls back to the origin before metrics arrive', () => {
    expect(pointToCell(50, 50, { cols: 80, rows: 24, cellWidth: 0, cellHeight: 0 })).toEqual({
      col: 0,
      row: 0,
    });
  });
});
