/**
 * terminalScroll.ts — pure touch → terminal-scroll math for {@link TerminalPane}.
 *
 * Kept free of React / native so jest can pin the gesture semantics:
 *   - a drag only becomes a scroll once it leaves the touch slop (otherwise it
 *     stays a tap / long-press candidate),
 *   - finger travel is accumulated in px and released as whole lines, carrying
 *     the remainder so slow drags still scroll,
 *   - finger moving DOWN reveals older output (positive lines = up into
 *     scrollback), the same sign the FFI `scroll` / `scroll_wheel` take and the
 *     natural-scrolling direction iOS users expect.
 */

/** Movement (px) under which a touch is still a tap / long-press candidate. */
export const TOUCH_SLOP_PX = 8;

/** True once a touch has travelled far enough to count as a drag. */
export function exceedsTouchSlop(dx: number, dy: number, slop = TOUCH_SLOP_PX): boolean {
  return Math.hypot(dx, dy) > slop;
}

export interface ScrollStep {
  /** Whole lines to scroll: positive = up (into scrollback), negative = down. */
  lines: number;
  /** Sub-line px carried into the next move event. */
  remainderPx: number;
}

/**
 * Fold a vertical finger movement (`dyPx`, positive = finger moved down) into
 * the running px accumulator and split off whole cell-height lines.
 */
export function accumulateScroll(accumPx: number, dyPx: number, cellHeight: number): ScrollStep {
  const total = accumPx + dyPx;
  if (!(cellHeight > 0) || !Number.isFinite(total)) {
    return { lines: 0, remainderPx: 0 };
  }
  const lines = Math.trunc(total / cellHeight);
  // `|| 0` folds -0 into 0 so callers can compare with `!== 0` / toEqual.
  return { lines: lines || 0, remainderPx: total - lines * cellHeight };
}

/** Map a point inside the pane to a clamped grid cell. */
export function pointToCell(
  x: number,
  y: number,
  grid: { cols: number; rows: number; cellWidth: number; cellHeight: number },
): { col: number; row: number } {
  const clamp = (v: number, max: number) => Math.min(Math.max(v, 0), Math.max(max - 1, 0));
  const col = grid.cellWidth > 0 ? clamp(Math.floor(x / grid.cellWidth), grid.cols) : 0;
  const row = grid.cellHeight > 0 ? clamp(Math.floor(y / grid.cellHeight), grid.rows) : 0;
  return { col, row };
}
