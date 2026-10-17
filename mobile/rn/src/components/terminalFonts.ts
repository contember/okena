/**
 * terminalFonts.ts — Skia glue for the terminal's per-glyph font fallback.
 *
 * The resolution logic lives in `terminalGlyphs.ts` (pure, unit-tested). This
 * module binds it to Skia:
 *   - the primary font's coverage via `SkFont.getGlyphIDs`;
 *   - the bundled `NotoSansSymbols2-Subset.ttf` (OFL, see
 *     `assets/NotoSansSymbols2-OFL.txt`), loaded once per process;
 *   - the platform fallback via SkParagraph, whose font collection runs the
 *     system's per-character matching (`matchFamilyStyleCharacter`), which
 *     the JS `SkFontMgr` binding does not expose. `extendedVisit` reports the
 *     resolved typeface and glyph id, which are cached and then drawn with
 *     plain `drawGlyphs`. A paragraph is built only once per missing codepoint.
 */

import { useEffect, useMemo, useState } from 'react';
import { Platform } from 'react-native';
import {
  Skia,
  loadData,
  type SkCanvas,
  type SkFont,
  type SkPaint,
  type SkPoint,
  type SkTypeface,
} from '@shopify/react-native-skia';

import {
  GlyphFallback,
  fitGlyph,
  type FallbackGlyph,
  type TypefaceGlyph,
} from './terminalGlyphs';

export type TerminalGlyphFallback = GlyphFallback<SkTypeface, SkFont>;
export type TerminalFallbackGlyph = FallbackGlyph<SkTypeface>;

// ── Bundled symbol font ─────────────────────────────────────────────────────

const SYMBOL_FONT_SOURCE = require('../../assets/NotoSansSymbols2-Subset.ttf');

/** `undefined` until the load settles; `null` when it failed. */
let symbolTypeface: SkTypeface | null | undefined;
let symbolLoad: Promise<SkTypeface | null> | null = null;

function loadSymbolTypeface(): Promise<SkTypeface | null> {
  if (!symbolLoad) {
    symbolLoad = loadData(SYMBOL_FONT_SOURCE, (data) =>
      Skia.Typeface.MakeFreeTypeFaceFromData(data),
    )
      .catch(() => null)
      .then((tf) => {
        symbolTypeface = tf;
        return tf;
      });
  }
  return symbolLoad;
}

// ── System fallback (SkParagraph) ───────────────────────────────────────────

/** Families tried before the per-character system match. Unknown names are skipped. */
const TEXT_FAMILIES = Platform.select({
  ios: ['Menlo', 'STIX Two Math'],
  default: ['monospace', 'sans-serif'],
});
const EMOJI_FAMILIES = Platform.select({
  ios: ['Apple Color Emoji'],
  default: ['Noto Color Emoji'],
});

function systemGlyph(ch: string, emoji: boolean): TypefaceGlyph<SkTypeface> | null {
  try {
    const builder = Skia.ParagraphBuilder.Make();
    builder.pushStyle({
      fontSize: 16,
      fontFamilies: emoji ? EMOJI_FAMILIES : TEXT_FAMILIES,
      color: Skia.Color('white'),
    });
    builder.addText(ch);
    const paragraph = builder.build();
    paragraph.layout(1000);
    let found: TypefaceGlyph<SkTypeface> | null = null;
    paragraph.extendedVisit((_line, info) => {
      if (found || !info || info.glyphs.length === 0 || info.glyphs[0] === 0) return;
      const typeface = info.font.getTypeface();
      if (typeface) found = { typeface, glyph: info.glyphs[0] };
    });
    return found;
  } catch {
    return null;
  }
}

// ── Resolver instances ──────────────────────────────────────────────────────

let cached: { primary: SkFont; bundled: SkTypeface | null; fallback: TerminalGlyphFallback } | null =
  null;

/** Resolver for a primary font + bundled typeface; shared by all panes. */
export function getGlyphFallback(
  primary: SkFont,
  bundled: SkTypeface | null,
): TerminalGlyphFallback {
  if (cached && cached.primary === primary && cached.bundled === bundled) return cached.fallback;
  const fallback: TerminalGlyphFallback = new GlyphFallback<SkTypeface, SkFont>({
    primaryGlyphId: (ch) => primary.getGlyphIDs(ch, 1)[0] ?? 0,
    bundled: bundled
      ? { typeface: bundled, glyphId: (ch) => bundled.getGlyphIDs(ch, 1)[0] ?? 0 }
      : null,
    system: systemGlyph,
    makeFont: (typeface, size) => Skia.Font(typeface, size),
  });
  cached = { primary, bundled, fallback };
  return fallback;
}

/**
 * The glyph resolver for `primary`. Starts without the bundled symbol font
 * and swaps to a resolver that has it once it loads, so the caller repaints.
 */
export function useGlyphFallback(primary: SkFont): TerminalGlyphFallback {
  const [bundled, setBundled] = useState<SkTypeface | null>(symbolTypeface ?? null);
  useEffect(() => {
    if (symbolTypeface !== undefined) return;
    let live = true;
    loadSymbolTypeface().then((tf) => {
      if (live) setBundled(tf);
    });
    return () => {
      live = false;
    };
  }, []);
  return useMemo(() => getGlyphFallback(primary, bundled), [primary, bundled]);
}

// ── Drawing ─────────────────────────────────────────────────────────────────

let origin: SkPoint[] | null = null;

export interface FallbackCellBox {
  /** Left edge of the first cell. */
  x: number;
  /** Top edge of the row. */
  top: number;
  /** Width available to the glyph (one or two cells). */
  width: number;
  cellHeight: number;
  /** Primary-font baseline offset from the row top. */
  baseline: number;
}

/**
 * Draw a fallback glyph fitted into its cell(s): shrunk when wider than the
 * box (scaled around the row's vertical center), centered horizontally, on
 * the primary font's baseline otherwise.
 */
export function drawFallbackGlyph(
  canvas: SkCanvas,
  fallback: TerminalGlyphFallback,
  glyph: TerminalFallbackGlyph,
  box: FallbackCellBox,
  fontSize: number,
  paint: SkPaint,
): void {
  if (!origin) origin = [Skia.Point(0, 0)];
  const { font, advance, glyphs } = fallback.sized(glyph, fontSize);
  const { scale, dx } = fitGlyph(advance, box.width);
  if (scale === 1) {
    canvas.drawGlyphs(glyphs, origin, box.x + dx, box.top + box.baseline, font, paint);
    return;
  }
  const centerY = box.top + box.cellHeight / 2;
  canvas.save();
  canvas.translate(box.x + dx, centerY);
  canvas.scale(scale, scale);
  canvas.drawGlyphs(glyphs, origin, 0, box.baseline - box.cellHeight / 2, font, paint);
  canvas.restore();
}
