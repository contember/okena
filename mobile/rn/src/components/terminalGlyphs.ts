/**
 * terminalGlyphs.ts — per-codepoint font fallback for the Skia terminal renderer.
 *
 * The primary terminal font (JetBrains Mono) has no glyphs for many symbols
 * that TUIs print: Claude Code's `⏺ ✻ ✽ ✳ ✢ ✔ ⏵ ⎿`, braille spinners, emoji,
 * CJK. Skia draws glyph id 0 (the crossed "tofu" box) for those. This module
 * decides, once per codepoint, where each glyph comes from:
 *
 *   1. the primary font, when it has the glyph (the common case);
 *   2. a primary-font stand-in from {@link GLYPH_ALIASES} (e.g. `⎿` → `└`),
 *      for symbols no reliable font on iOS/Android carries;
 *   3. the bundled symbol font (a Noto Sans Symbols 2 subset), skipped for
 *      emoji-presentation codepoints so those stay colour emoji;
 *   4. the platform font fallback (emoji, CJK, everything else).
 *
 * Results are cached, so the per-frame cost of a non-ASCII cell is one `Map`
 * lookup. Nothing here imports Skia at runtime: the Skia-backed dependencies
 * are injected (see `terminalFonts.ts`), which keeps the logic unit-testable.
 */

// ── Codepoint classification ────────────────────────────────────────────────

/**
 * Inclusive `[start, end]` ranges of East Asian Wide/Fullwidth codepoints,
 * sorted ascending. A condensed form of the `unicode-width` tables the
 * terminal emulator (alacritty) uses: those cells occupy two columns, with a
 * blank spacer cell after them in the packed buffer.
 */
const WIDE_RANGES: readonly (readonly [number, number])[] = [
  [0x1100, 0x115f],
  [0x231a, 0x231b],
  [0x2329, 0x232a],
  [0x23e9, 0x23ec],
  [0x23f0, 0x23f0],
  [0x23f3, 0x23f3],
  [0x25fd, 0x25fe],
  [0x2614, 0x2615],
  [0x2648, 0x2653],
  [0x267f, 0x267f],
  [0x2693, 0x2693],
  [0x26a1, 0x26a1],
  [0x26aa, 0x26ab],
  [0x26bd, 0x26be],
  [0x26c4, 0x26c5],
  [0x26ce, 0x26ce],
  [0x26d4, 0x26d4],
  [0x26ea, 0x26ea],
  [0x26f2, 0x26f3],
  [0x26f5, 0x26f5],
  [0x26fa, 0x26fa],
  [0x26fd, 0x26fd],
  [0x2705, 0x2705],
  [0x270a, 0x270b],
  [0x2728, 0x2728],
  [0x274c, 0x274c],
  [0x274e, 0x274e],
  [0x2753, 0x2755],
  [0x2757, 0x2757],
  [0x2795, 0x2797],
  [0x27b0, 0x27b0],
  [0x27bf, 0x27bf],
  [0x2b1b, 0x2b1c],
  [0x2b50, 0x2b50],
  [0x2b55, 0x2b55],
  [0x2e80, 0x303e],
  [0x3041, 0x33ff],
  [0x3400, 0x4dbf],
  [0x4e00, 0x9fff],
  [0xa000, 0xa4cf],
  [0xa960, 0xa97f],
  [0xac00, 0xd7a3],
  [0xf900, 0xfaff],
  [0xfe10, 0xfe19],
  [0xfe30, 0xfe6f],
  [0xff00, 0xff60],
  [0xffe0, 0xffe6],
  [0x16fe0, 0x16fe4],
  [0x17000, 0x18aff],
  [0x1b000, 0x1b2ff],
  [0x1f004, 0x1f004],
  [0x1f0cf, 0x1f0cf],
  [0x1f18e, 0x1f18e],
  [0x1f191, 0x1f19a],
  [0x1f200, 0x1f202],
  [0x1f210, 0x1f23b],
  [0x1f240, 0x1f248],
  [0x1f250, 0x1f251],
  [0x1f260, 0x1f265],
  [0x1f300, 0x1f320],
  [0x1f32d, 0x1f335],
  [0x1f337, 0x1f37c],
  [0x1f37e, 0x1f393],
  [0x1f3a0, 0x1f3ca],
  [0x1f3cf, 0x1f3d3],
  [0x1f3e0, 0x1f3f0],
  [0x1f3f4, 0x1f3f4],
  [0x1f3f8, 0x1f43e],
  [0x1f440, 0x1f440],
  [0x1f442, 0x1f4fc],
  [0x1f4ff, 0x1f53d],
  [0x1f54b, 0x1f54e],
  [0x1f550, 0x1f567],
  [0x1f57a, 0x1f57a],
  [0x1f595, 0x1f596],
  [0x1f5a4, 0x1f5a4],
  [0x1f5fb, 0x1f64f],
  [0x1f680, 0x1f6c5],
  [0x1f6cc, 0x1f6cc],
  [0x1f6d0, 0x1f6d2],
  [0x1f6d5, 0x1f6d7],
  [0x1f6dc, 0x1f6df],
  [0x1f6eb, 0x1f6ec],
  [0x1f6f4, 0x1f6fc],
  [0x1f7e0, 0x1f7eb],
  [0x1f7f0, 0x1f7f0],
  [0x1f90c, 0x1f93a],
  [0x1f93c, 0x1f945],
  [0x1f947, 0x1f9ff],
  [0x1fa70, 0x1faff],
  [0x20000, 0x2fffd],
  [0x30000, 0x3fffd],
];

function inRanges(cp: number, ranges: readonly (readonly [number, number])[]): boolean {
  let lo = 0;
  let hi = ranges.length - 1;
  while (lo <= hi) {
    const mid = (lo + hi) >> 1;
    const [start, end] = ranges[mid];
    if (cp < start) hi = mid - 1;
    else if (cp > end) lo = mid + 1;
    else return true;
  }
  return false;
}

/** True when the codepoint occupies two terminal columns. */
export function isWideCodepoint(cp: number): boolean {
  if (cp < 0x1100) return false;
  return inRanges(cp, WIDE_RANGES);
}

/**
 * True for codepoints that render as colour emoji by default (wide symbols in
 * the emoji blocks, e.g. `✅ ⚡ ✨ 🤖`). Text-presentation symbols such as
 * `⏺ ✔ ⚠` return false and are tinted with the cell colour.
 */
export function isEmojiPresentation(cp: number): boolean {
  if (!isWideCodepoint(cp)) return false;
  return (cp >= 0x231a && cp <= 0x2b55) || (cp >= 0x1f000 && cp < 0x1fb00);
}

/**
 * Stand-ins drawn with the primary font for symbols that neither the bundled
 * symbol font nor stock iOS/Android fonts reliably carry. `⎿` (U+23BF, Claude
 * Code's tool-output connector) is visually the light up-and-right corner.
 */
export const GLYPH_ALIASES: ReadonlyMap<number, string> = new Map([[0x23bf, '└']]);

// ── Fallback resolution ─────────────────────────────────────────────────────

/** Minimal font surface the resolver needs (satisfied by Skia's `SkFont`). */
export interface GlyphFont {
  getGlyphWidths(glyphs: number[]): number[];
}

/** A glyph in a specific typeface, as returned by the system fallback. */
export interface TypefaceGlyph<TTypeface> {
  typeface: TTypeface;
  glyph: number;
}

/** Glyph source with a fixed typeface (the bundled symbol font). */
export interface BundledGlyphSource<TTypeface> {
  typeface: TTypeface;
  /** Glyph id for a single-codepoint string; 0 when missing. */
  glyphId(ch: string): number;
}

export interface GlyphFallbackDeps<TTypeface, TFont extends GlyphFont> {
  /** Glyph id of `ch` in the primary font; 0 when missing. */
  primaryGlyphId(ch: string): number;
  /** Bundled symbol font, or null while it loads / when unavailable. */
  bundled: BundledGlyphSource<TTypeface> | null;
  /** Platform font fallback for `ch`; null when no font has it. */
  system(ch: string, emoji: boolean): TypefaceGlyph<TTypeface> | null;
  /** Builds a font for a typeface at a size. */
  makeFont(typeface: TTypeface, size: number): TFont;
}

/** Draw `text` with the primary font (also used for aliases and true misses). */
export interface PrimaryGlyph {
  readonly kind: 'primary';
  readonly text: string;
}

/** Draw `glyph` from `typeface`, fitted into one cell or two (`wide`). */
export interface FallbackGlyph<TTypeface> {
  readonly kind: 'fallback';
  readonly text: string;
  readonly typeface: TTypeface;
  readonly glyph: number;
  readonly wide: boolean;
}

export type GlyphResolution<TTypeface> = PrimaryGlyph | FallbackGlyph<TTypeface>;

/** A fallback glyph's font at the current size, plus its advance. */
export interface SizedFallbackFont<TFont> {
  font: TFont;
  advance: number;
  /** `[glyph]`, preallocated for `drawGlyphs`. */
  glyphs: number[];
}

/**
 * Codepoint → glyph source resolver with caching. One instance per
 * (primary font, bundled font) pair; sizes are handled by {@link sized}.
 */
export class GlyphFallback<TTypeface, TFont extends GlyphFont> {
  private readonly resolutions = new Map<number, GlyphResolution<TTypeface>>();
  private readonly sizedFonts = new Map<FallbackGlyph<TTypeface>, SizedFallbackFont<TFont>>();
  private sizedFor = -1;

  constructor(private readonly deps: GlyphFallbackDeps<TTypeface, TFont>) {}

  /** Resolve a codepoint; cached after the first call. */
  resolve(cp: number): GlyphResolution<TTypeface> {
    const cached = this.resolutions.get(cp);
    if (cached) return cached;
    const resolved = this.compute(cp);
    this.resolutions.set(cp, resolved);
    return resolved;
  }

  /** The fallback glyph's font at `size` (cached until the size changes). */
  sized(fb: FallbackGlyph<TTypeface>, size: number): SizedFallbackFont<TFont> {
    if (size !== this.sizedFor) {
      this.sizedFonts.clear();
      this.sizedFor = size;
    }
    let entry = this.sizedFonts.get(fb);
    if (!entry) {
      const font = this.deps.makeFont(fb.typeface, size);
      const glyphs = [fb.glyph];
      const advance = font.getGlyphWidths(glyphs)[0] ?? 0;
      entry = { font, advance, glyphs };
      this.sizedFonts.set(fb, entry);
    }
    return entry;
  }

  private compute(cp: number): GlyphResolution<TTypeface> {
    const text = String.fromCodePoint(cp);
    if (this.deps.primaryGlyphId(text) !== 0) return { kind: 'primary', text };

    const alias = GLYPH_ALIASES.get(cp);
    if (alias !== undefined && this.deps.primaryGlyphId(alias) !== 0) {
      return { kind: 'primary', text: alias };
    }

    const wide = isWideCodepoint(cp);
    const emoji = isEmojiPresentation(cp);
    const bundled = this.deps.bundled;
    if (bundled && !emoji) {
      const glyph = bundled.glyphId(text);
      if (glyph !== 0) return { kind: 'fallback', text, typeface: bundled.typeface, glyph, wide };
    }

    const system = this.deps.system(text, emoji);
    if (system && system.glyph !== 0) {
      return { kind: 'fallback', text, typeface: system.typeface, glyph: system.glyph, wide };
    }

    // Nothing has it: let the primary font draw its missing-glyph box.
    return { kind: 'primary', text };
  }
}

// ── Cell fitting ────────────────────────────────────────────────────────────

/**
 * Scale and horizontal offset that fit a glyph of `advance` into `target`
 * pixels: shrink when too wide, never enlarge, and center the result.
 */
export function fitGlyph(advance: number, target: number): { scale: number; dx: number } {
  if (advance <= 0 || target <= 0) return { scale: 1, dx: 0 };
  const scale = advance > target ? target / advance : 1;
  return { scale, dx: (target - advance * scale) / 2 };
}
