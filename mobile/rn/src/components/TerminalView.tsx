/**
 * TerminalView.tsx — native GPU terminal renderer on `@shopify/react-native-skia`.
 *
 * ┌───────────────────────────────────────────────────────────────────────┐
 * │ SCAFFOLD — NOT YET BUILT/RUN. Requires the RN toolchain + the `ubrn`     │
 * │ native module. See mobile/rn/README.md. The TS is typed against the      │
 * │ public APIs of `react-native` and `@shopify/react-native-skia@^1.5`.     │
 * └───────────────────────────────────────────────────────────────────────┘
 *
 * Port of `mobile/lib/src/widgets/terminal_painter.dart` (the Flutter
 * `CustomPainter`) + the sizing/poll loop from `terminal_view.dart`.
 *
 * Strategy (RN_MIGRATION.md Decision B/C):
 *   - Cells come from the PACKED buffer (`getVisibleCellsPacked`) decoded via
 *     ../native/cells, NOT the per-cell record array — avoids marshalling
 *     thousands of JSI objects per frame.
 *   - Paint mirrors the Flutter 3-pass algorithm exactly, using
 *     Skia's imperative `createPicture((canvas) => …)` (the analog of
 *     `CustomPainter.paint(Canvas, Size)`):
 *       Pass 1: background rects (only where bg != default) + selection overlay.
 *       Pass 2: glyph runs batched by (effective fg + style flags) within a row.
 *       Pass 3: cursor (block / underline / beam, honoring visibility).
 *       Pass 4: scrollback thumb indicator.
 *   - Repaint is driven by `requestAnimationFrame` gated on `isDirty()`
 *     (Decision C), NOT a fixed 33ms timer.
 *   - Sizing: `onLayout` → cols/rows from a measured monospace glyph →
 *     `resizeLocal` immediately + debounced `resizeTerminal` (200ms), as today.
 *
 * Presentational + testable: the native surface is injected via the `native`
 * prop (typed `OkenaNative`), so this renders against a mock with no real
 * binding. Fonts are injected too, so callers control font loading.
 */

import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { View, StyleSheet, type LayoutChangeEvent } from 'react-native';
import {
  Canvas,
  Picture,
  Skia,
  createPicture,
  PaintStyle,
  type SkCanvas,
  type SkFont,
  type SkPaint,
} from '@shopify/react-native-skia';

import type {
  CursorState,
  OkenaNative,
  ScrollInfo,
  SelectionBounds,
} from '../native/okena';
import {
  FLAG_BOLD,
  FLAG_DIM,
  FLAG_INVERSE,
  FLAG_ITALIC,
  FLAG_STRIKETHROUGH,
  FLAG_STYLE_MASK,
  FLAG_UNDERLINE,
  PackedCells,
  decodeCellsView,
} from '../native/cells';
import { TerminalTheme } from '../theme';

// ── Fonts ───────────────────────────────────────────────────────────────────

/**
 * The four JetBrainsMono variants. Load with `useFont` from the bundled ttf
 * (see README) and pass them in. `regular` is required; the others are
 * optional — when a variant is missing we synthesize it on the regular font
 * via `setEmbolden` (bold) / `setSkewX` (italic), matching how Skia would
 * fake-style anyway.
 */
export interface TerminalFonts {
  regular: SkFont;
  bold?: SkFont;
  italic?: SkFont;
  boldItalic?: SkFont;
}

// ── Props ─────────────────────────────────────────────────────────────────

export interface TerminalViewProps {
  /** Injected native surface (real `ubrn` module, or a mock for tests). */
  native: OkenaNative;
  connId: string;
  terminalId: string;
  /** Loaded JetBrainsMono fonts (see {@link TerminalFonts}). */
  fonts: TerminalFonts;
  /** Font size in logical px. Defaults to {@link TerminalTheme.defaultFontSize}. */
  fontSize?: number;
  /**
   * Whether a selection is in progress (drives whether selection bounds are
   * polled each frame). The host (gesture handler) owns this.
   */
  selecting?: boolean;
  /**
   * Called whenever the computed grid size changes, AFTER `resizeLocal` has run.
   * The host typically debounces a `resizeTerminal` here, but this component
   * already does that internally; this is for the host to track cols/rows.
   */
  onGridSizeChange?: (cols: number, rows: number) => void;
}

// ── Geometry from font metrics ────────────────────────────────────────────────

interface CellMetrics {
  cellWidth: number;
  cellHeight: number;
  /** Baseline offset from the top of the cell (ascent), for glyph placement. */
  baseline: number;
}

/**
 * Compute monospace cell metrics from the regular font, mirroring
 * `_computeCellSize()` in terminal_view.dart (`width` of 'M', height * lineHeightFactor).
 */
function measureCell(font: SkFont): CellMetrics {
  const advance = font.measureText('M').width;
  const m = font.getMetrics();
  // ascent is negative (above baseline), descent positive (below).
  const textHeight = -m.ascent + m.descent;
  const cellHeight = textHeight * TerminalTheme.lineHeightFactor;
  // Center the glyph box vertically within the (taller) cell, then add ascent.
  const baseline = (cellHeight - textHeight) / 2 + -m.ascent;
  return { cellWidth: advance, cellHeight, baseline };
}

// ── Color helpers ─────────────────────────────────────────────────────────────

/** Cache Skia colors keyed by packed ARGB to avoid re-allocating each frame. */
const colorCache = new Map<number, ReturnType<typeof Skia.Color>>();
function skColor(argb: number) {
  const key = argb >>> 0;
  let c = colorCache.get(key);
  if (!c) {
    // Skia.Color accepts CSS-ish input; pass an `#RRGGBBAA` string built from ARGB.
    const v = key;
    const a = (v >>> 24) & 0xff;
    const r = (v >>> 16) & 0xff;
    const g = (v >>> 8) & 0xff;
    const b = v & 0xff;
    const hex = (n: number) => n.toString(16).padStart(2, '0');
    c = Skia.Color(`#${hex(r)}${hex(g)}${hex(b)}${hex(a)}`);
    colorCache.set(key, c);
  }
  return c;
}

function alpha(argb: number): number {
  return (argb >>> 24) & 0xff;
}

// ── Paint passes (the Flutter port) ────────────────────────────────────────────

function selectCellInSelection(
  col: number,
  row: number,
  sel: SelectionBounds,
  displayOffset: number,
): boolean {
  // Buffer row = visual row - display offset (matches _isCellInSelection in Dart).
  const bufferRow = row - displayOffset;
  const { startRow: sr, endRow: er, startCol: sc, endCol: ec } = sel;
  if (bufferRow < sr || bufferRow > er) return false;
  if (sr === er) return col >= sc && col <= ec;
  if (bufferRow === sr) return col >= sc;
  if (bufferRow === er) return col <= ec;
  return true;
}

interface PaintArgs {
  canvas: SkCanvas;
  cells: PackedCells;
  cursor: CursorState;
  scroll: ScrollInfo;
  selection?: SelectionBounds;
  metrics: CellMetrics;
  fonts: TerminalFonts;
  fontSize: number;
  width: number;
  height: number;
}

/** Pick the font variant for a style, faking bold/italic on `regular` if absent. */
function fontFor(fonts: TerminalFonts, bold: boolean, italic: boolean): SkFont {
  if (bold && italic && fonts.boldItalic) return fonts.boldItalic;
  if (bold && !italic && fonts.bold) return fonts.bold;
  if (!bold && italic && fonts.italic) return fonts.italic;
  if (!bold && !italic) return fonts.regular;
  // Variant missing → synthesize on regular. (Mutates a shared font; acceptable
  // here because painting is single-threaded and we reset below.)
  const f = fonts.regular;
  // NOTE: synthetic *bold* via `setEmbolden` is intentionally omitted. All four
  // JetBrainsMono variants are bundled, so this fallback only runs if a variant
  // fails to load — and `setEmbolden` is broken in react-native-skia 1.12.4: its
  // native binding rejects the (typed `boolean`) arg with "Value is false,
  // expected a number". Italic is still synthesized via `setSkewX` (typed
  // `number`, works fine).
  f.setSkewX(italic ? -0.25 : 0);
  return f;
}

function resetSynthetic(font: SkFont) {
  font.setSkewX(0);
}

function paintTerminal(args: PaintArgs): void {
  const { canvas, cells, cursor, scroll, selection, metrics, fonts, width, height } = args;
  const { cellWidth, cellHeight, baseline } = metrics;
  const cols = cells.cols;
  const rows = cells.rows;

  const bgPaint = Skia.Paint();
  bgPaint.setAntiAlias(false);

  const defaultBg = TerminalTheme.bgColorArgb >>> 0;
  const selOverlay = skColor(TerminalTheme.selectionOverlayArgb);
  const displayOffset = scroll.displayOffset;

  // ── Pass 1: background rects + selection highlight ──────────────────────
  for (let i = 0; i < cells.count; i++) {
    const col = i % cols;
    const row = (i / cols) | 0;
    const x = col * cellWidth;
    const y = row * cellHeight;

    let bgArgb = cells.bg(i);
    let fgArgb = cells.fg(i);
    const flags = cells.flags(i);
    if (flags & FLAG_INVERSE) {
      const tmp = bgArgb;
      bgArgb = fgArgb;
      fgArgb = tmp;
    }

    if ((bgArgb >>> 0) !== defaultBg && alpha(bgArgb) > 0) {
      bgPaint.setColor(skColor(bgArgb));
      canvas.drawRect(Skia.XYWHRect(x, y, cellWidth, cellHeight), bgPaint);
    }

    if (selection && selectCellInSelection(col, row, selection, displayOffset)) {
      bgPaint.setColor(selOverlay);
      canvas.drawRect(Skia.XYWHRect(x, y, cellWidth, cellHeight), bgPaint);
    }
  }

  // ── Pass 2: text — batched by style runs within each row ─────────────────
  const textPaint = Skia.Paint();
  textPaint.setAntiAlias(true);

  for (let row = 0; row < rows; row++) {
    let col = 0;
    while (col < cols) {
      const idx = row * cols + col;
      if (cells.isBlank(idx)) {
        col++;
        continue;
      }

      const flags0 = cells.flags(idx);
      let fg0 = flags0 & FLAG_INVERSE ? cells.bg(idx) : cells.fg(idx);
      const style0 = flags0 & FLAG_STYLE_MASK;

      const startCol = col;
      let run = cells.char(idx);
      col++;

      while (col < cols) {
        const ci = row * cols + col;
        if (cells.isBlank(ci)) break;
        const f = cells.flags(ci);
        const cFg = f & FLAG_INVERSE ? cells.bg(ci) : cells.fg(ci);
        const cStyle = f & FLAG_STYLE_MASK;
        if ((cFg >>> 0) !== (fg0 >>> 0) || cStyle !== style0) break;
        run += cells.char(ci);
        col++;
      }

      // Effective fg (dim halves alpha, matching the Dart painter).
      let fgEff = fg0 >>> 0;
      if (style0 & FLAG_DIM) {
        const a = Math.round(alpha(fgEff) * 0.5);
        fgEff = ((a << 24) | (fgEff & 0x00ffffff)) >>> 0;
      }
      textPaint.setColor(skColor(fgEff));

      const bold = (style0 & FLAG_BOLD) !== 0;
      const italic = (style0 & FLAG_ITALIC) !== 0;
      const font = fontFor(fonts, bold, italic);

      const x = startCol * cellWidth;
      const y = row * cellHeight + baseline;
      canvas.drawText(run, x, y, textPaint, font);

      // Underline / strikethrough as drawn lines (decoration in Dart).
      if (style0 & (FLAG_UNDERLINE | FLAG_STRIKETHROUGH)) {
        const linePaint = Skia.Paint();
        linePaint.setColor(skColor(fgEff));
        linePaint.setStrokeWidth(Math.max(1, args.fontSize / 14));
        const runW = run.length * cellWidth;
        if (style0 & FLAG_UNDERLINE) {
          const uy = row * cellHeight + cellHeight - 1;
          canvas.drawLine(x, uy, x + runW, uy, linePaint);
        }
        if (style0 & FLAG_STRIKETHROUGH) {
          const sy = row * cellHeight + cellHeight / 2;
          canvas.drawLine(x, sy, x + runW, sy, linePaint);
        }
      }

      resetSynthetic(fonts.regular);
    }
  }

  // ── Pass 3: cursor ───────────────────────────────────────────────────────
  if (cursor.visible && cursor.col < cols && cursor.row < rows) {
    const cx = cursor.col * cellWidth;
    const cy = cursor.row * cellHeight;
    const cursorPaint = Skia.Paint();
    switch (cursor.shape) {
      case 'block': {
        // Half-alpha block, matching withAlpha(128) in Dart.
        const c = (0x80000000 | (TerminalTheme.cursorColorArgb & 0x00ffffff)) >>> 0;
        cursorPaint.setColor(skColor(c));
        cursorPaint.setStyle(PaintStyle.Fill);
        canvas.drawRect(Skia.XYWHRect(cx, cy, cellWidth, cellHeight), cursorPaint);
        break;
      }
      case 'beam': {
        cursorPaint.setColor(skColor(TerminalTheme.cursorColorArgb));
        cursorPaint.setStrokeWidth(2);
        canvas.drawLine(cx, cy, cx, cy + cellHeight, cursorPaint);
        break;
      }
      case 'underline': {
        cursorPaint.setColor(skColor(TerminalTheme.cursorColorArgb));
        cursorPaint.setStrokeWidth(2);
        canvas.drawLine(cx, cy + cellHeight - 1, cx + cellWidth, cy + cellHeight - 1, cursorPaint);
        break;
      }
    }
  }

  // ── Pass 4: scrollback thumb ───────────────────────────────────────────────
  if (scroll.totalLines > scroll.visibleLines && scroll.totalLines > 0 && scroll.visibleLines > 0) {
    const trackHeight = height;
    const thumbHeight = Math.min(
      Math.max((scroll.visibleLines / scroll.totalLines) * trackHeight, 20),
      trackHeight,
    );
    const maxOffset = scroll.totalLines - scroll.visibleLines;
    const thumbTop =
      maxOffset > 0 ? (1 - scroll.displayOffset / maxOffset) * (trackHeight - thumbHeight) : 0;
    const scrollPaint: SkPaint = Skia.Paint();
    scrollPaint.setColor(skColor(0x40ffffff));
    scrollPaint.setStyle(PaintStyle.Fill);
    canvas.drawRRect(
      Skia.RRectXY(Skia.XYWHRect(width - 4, thumbTop, 3, thumbHeight), 1.5, 1.5),
      scrollPaint,
    );
  }
}

// ── Component ─────────────────────────────────────────────────────────────────

export const TerminalView: React.FC<TerminalViewProps> = ({
  native,
  connId,
  terminalId,
  fonts,
  fontSize = TerminalTheme.defaultFontSize,
  selecting = false,
  onGridSizeChange,
}) => {
  const [size, setSize] = useState<{ w: number; h: number } | null>(null);
  // Bumped to force the picture to recompute (the actual cell data lives in
  // native; this is just a repaint trigger gated on isDirty()).
  const [frame, setFrame] = useState(0);

  const colsRef = useRef(0);
  const rowsRef = useRef(0);
  const resizeTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const initialResizeSent = useRef(false);

  // Resize the fonts in-place so metrics match the requested size, then measure
  // a cell. Kept as one memo so `measureCell` always runs after `setSize` and
  // `fontSize` is a real (visible-to-eslint) dependency — the Skia font is
  // mutated in place, so the `fonts` reference alone wouldn't track size changes.
  const metrics = useMemo(() => {
    fonts.regular.setSize(fontSize);
    fonts.bold?.setSize(fontSize);
    fonts.italic?.setSize(fontSize);
    fonts.boldItalic?.setSize(fontSize);
    return measureCell(fonts.regular);
  }, [fonts, fontSize]);

  // ── Layout → cols/rows → resizeLocal + debounced resizeTerminal ──────────
  const onLayout = useCallback(
    (e: LayoutChangeEvent) => {
      const { width, height } = e.nativeEvent.layout;
      if (width <= 0 || height <= 0) return;
      setSize({ w: width, h: height });

      const { cellWidth, cellHeight } = metrics;
      if (cellWidth <= 0 || cellHeight <= 0) return;

      const newCols = Math.min(Math.max(Math.floor(width / cellWidth), 1), 500);
      const newRows = Math.min(Math.max(Math.floor(height / cellHeight), 1), 200);

      if (newCols !== colsRef.current || newRows !== rowsRef.current) {
        colsRef.current = newCols;
        rowsRef.current = newRows;

        // Immediate local resize for responsive rendering (no WS round-trip).
        native.resizeLocal(connId, terminalId, newCols, newRows);
        onGridSizeChange?.(newCols, newRows);

        if (!initialResizeSent.current) {
          // First resize fires immediately — avoids a flash of garbled content.
          initialResizeSent.current = true;
          if (resizeTimer.current) clearTimeout(resizeTimer.current);
          native.resizeTerminal(connId, terminalId, newCols, newRows);
        } else {
          // Debounce subsequent resizes (200ms), as in terminal_view.dart.
          if (resizeTimer.current) clearTimeout(resizeTimer.current);
          resizeTimer.current = setTimeout(() => {
            native.resizeTerminal(connId, terminalId, newCols, newRows);
          }, 200);
        }
      }
    },
    [native, connId, terminalId, metrics, onGridSizeChange],
  );

  // Reset resize/init state when the target terminal changes.
  useEffect(() => {
    initialResizeSent.current = false;
    colsRef.current = 0;
    rowsRef.current = 0;
  }, [connId, terminalId]);

  // ── Repaint loop: rAF gated on isDirty() (Decision C) ────────────────────
  useEffect(() => {
    let raf = 0;
    let mounted = true;
    const tick = () => {
      if (!mounted) return;
      // Repaint when native reports new output. We always repaint on the very
      // first tick (frame===0 picture) so initial content shows.
      if (native.isDirty(connId, terminalId)) {
        setFrame((f) => f + 1);
      }
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => {
      mounted = false;
      cancelAnimationFrame(raf);
    };
  }, [native, connId, terminalId]);

  useEffect(() => {
    return () => {
      if (resizeTimer.current) clearTimeout(resizeTimer.current);
    };
  }, []);

  // ── Build the SkPicture from the packed buffer ───────────────────────────
  const picture = useMemo(() => {
    if (!size) return null;
    // Read fresh native state for this frame.
    let cells: PackedCells;
    try {
      cells = decodeCellsView(native.getVisibleCellsPacked(connId, terminalId));
    } catch {
      // Native not ready / empty buffer — draw nothing this frame.
      return null;
    }
    const cursor = native.getCursor(connId, terminalId);
    const scroll = native.getScrollInfo(connId, terminalId);
    const selection = selecting
      ? native.getSelectionBounds(connId, terminalId)
      : undefined;

    return createPicture(
      (canvas) =>
        paintTerminal({
          canvas,
          cells,
          cursor,
          scroll,
          selection,
          metrics,
          fonts,
          fontSize,
          width: size.w,
          height: size.h,
        }),
      Skia.XYWHRect(0, 0, size.w, size.h),
    );
    // `frame` participates so the picture recomputes each dirty tick.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [size, frame, native, connId, terminalId, selecting, metrics, fonts, fontSize]);

  return (
    <View style={styles.root} onLayout={onLayout}>
      {picture ? (
        <Canvas style={StyleSheet.absoluteFill}>
          <Picture picture={picture} />
        </Canvas>
      ) : null}
    </View>
  );
};

const styles = StyleSheet.create({
  root: {
    flex: 1,
    backgroundColor: TerminalTheme.bgColor,
  },
});

export default TerminalView;
