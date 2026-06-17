/**
 * theme.ts — colors and typography, ported from
 * `mobile/lib/src/theme/app_theme.dart`.
 *
 * Dart `Color(0xAARRGGBB)` values are kept verbatim as packed ARGB numbers in
 * `argb`, and also exposed as RN-friendly `#RRGGBB[AA]` hex strings (RN's
 * `color` props want `#RRGGBBAA`, NOT `#AARRGGBB`).
 */

// ── helpers ──────────────────────────────────────────────────────────────

/** `0xAARRGGBB` (Flutter order) → `#RRGGBBAA` (RN/CSS order). */
function argbToHex(argb: number): string {
  const v = argb >>> 0;
  const a = (v >>> 24) & 0xff;
  const r = (v >>> 16) & 0xff;
  const g = (v >>> 8) & 0xff;
  const b = v & 0xff;
  const h = (n: number) => n.toString(16).padStart(2, '0');
  return `#${h(r)}${h(g)}${h(b)}${h(a)}`;
}

// ── Color system (mirrors OkenaColors in app_theme.dart) ───────────────────

const OkenaColorsArgb = {
  // Backgrounds
  background: 0xff000000,
  surface: 0xff0a0a0a,
  surfaceElevated: 0xff161616,
  surfaceOverlay: 0xff1c1c1c,
  // Borders
  border: 0xff1e1e1e,
  borderLight: 0xff2a2a2a,
  // Accent
  accent: 0xff7c7fff,
  // Text
  textPrimary: 0xffe8e8ec,
  textSecondary: 0xff98989f,
  textTertiary: 0xff5a5a62,
  // Semantic
  success: 0xff4ade80,
  warning: 0xfffbbf24,
  error: 0xfff87171,
  // Glass
  glassBg: 0xcc0a0a0a,
  glassStroke: 0x18ffffff,
  // Key toolbar
  keyBg: 0xff161616,
  keyBorder: 0xff2a2a2a,
  keyText: 0xffb0b0b8,
} as const;

type ColorName = keyof typeof OkenaColorsArgb;

/** Packed ARGB form (`0xAARRGGBB`) — matches the Dart `Color(...)` literals. */
export const OkenaColorsArgbMap: Readonly<Record<ColorName, number>> = OkenaColorsArgb;

/** RN/CSS hex form (`#RRGGBBAA`). */
export const OkenaColors: Readonly<Record<ColorName, string>> = Object.fromEntries(
  (Object.keys(OkenaColorsArgb) as ColorName[]).map((k) => [k, argbToHex(OkenaColorsArgb[k])]),
) as Record<ColorName, string>;

// ── Terminal theme (mirrors TerminalTheme in app_theme.dart) ───────────────

export const TerminalTheme = {
  /** Must match the loaded font family name (see README font-linking step). */
  fontFamily: 'JetBrainsMono',
  fontFamilyFallback: ['Menlo', 'Consolas', 'DejaVu Sans Mono', 'monospace'] as const,

  defaultFontSize: 13,
  minFontSize: 6,
  maxFontSize: 24,
  defaultColumns: 80,
  lineHeightFactor: 1.2,

  // Packed ARGB (for buffer comparisons against cell bg) + hex (for paints).
  bgColorArgb: 0xff000000,
  fgColorArgb: 0xffcdd6f4,
  cursorColorArgb: 0xfff5e0dc,
  selectionColorArgb: 0x40585b70,

  bgColor: argbToHex(0xff000000),
  fgColor: argbToHex(0xffcdd6f4),
  cursorColor: argbToHex(0xfff5e0dc),
  selectionColor: argbToHex(0x40585b70),

  /** Selection highlight overlay, matches `0x40264F78` in terminal_painter.dart. */
  selectionOverlayArgb: 0x40264f78,
  selectionOverlay: argbToHex(0x40264f78),
} as const;

// ── Typography (mirrors OkenaTypography; RN uses system font for `.SF Pro`) ──

/**
 * On iOS, `System` resolves to SF Pro. On Android there is no SF Pro, so this
 * falls back to the platform default (Roboto) — acceptable for the chrome.
 */
export const OkenaTypography = {
  fontFamily: 'System',
  largeTitle: { fontSize: 28, fontWeight: '700', letterSpacing: -0.5, color: OkenaColors.textPrimary },
  title: { fontSize: 20, fontWeight: '600', letterSpacing: -0.3, color: OkenaColors.textPrimary },
  headline: { fontSize: 17, fontWeight: '600', color: OkenaColors.textPrimary },
  body: { fontSize: 15, fontWeight: '400', color: OkenaColors.textPrimary },
  callout: { fontSize: 14, fontWeight: '400', color: OkenaColors.textSecondary },
  caption: { fontSize: 12, fontWeight: '500', color: OkenaColors.textSecondary },
  caption2: { fontSize: 11, fontWeight: '500', color: OkenaColors.textTertiary },
} as const;
