import 'package:flutter/painting.dart';

// ── Color system ────────────────────────────────────────────────────────

class OkenaColors {
  OkenaColors._();

  // Backgrounds
  static const background = Color(0xFF000000);
  static const surface = Color(0xFF0A0A0A);
  static const surfaceElevated = Color(0xFF161616);
  static const surfaceOverlay = Color(0xFF1C1C1C);

  // Borders
  static const border = Color(0xFF1E1E1E);
  static const borderLight = Color(0xFF2A2A2A);

  // Accent
  static const accent = Color(0xFF7C7FFF);

  // Text
  static const textPrimary = Color(0xFFE8E8EC);
  static const textSecondary = Color(0xFF98989F);
  static const textTertiary = Color(0xFF5A5A62);

  // Semantic
  static const success = Color(0xFF4ADE80);
  static const warning = Color(0xFFFBBF24);
  static const error = Color(0xFFF87171);

  // Glass
  static const glassBg = Color(0xCC0A0A0A); // 80% opacity surface
  static const glassStroke = Color(0x18FFFFFF); // subtle white border

  // Key toolbar
  static const keyBg = Color(0xFF161616);
  static const keyBorder = Color(0xFF2A2A2A);
  static const keyText = Color(0xFFB0B0B8);
}

// ── Typography ──────────────────────────────────────────────────────────

class OkenaTypography {
  OkenaTypography._();

  static const _fontFamily = '.SF Pro Text';

  static const largeTitle = TextStyle(
    fontFamily: _fontFamily,
    fontSize: 28,
    fontWeight: FontWeight.w700,
    letterSpacing: -0.5,
    color: OkenaColors.textPrimary,
  );

  static const title = TextStyle(
    fontFamily: _fontFamily,
    fontSize: 20,
    fontWeight: FontWeight.w600,
    letterSpacing: -0.3,
    color: OkenaColors.textPrimary,
  );

  static const headline = TextStyle(
    fontFamily: _fontFamily,
    fontSize: 17,
    fontWeight: FontWeight.w600,
    color: OkenaColors.textPrimary,
  );

  static const body = TextStyle(
    fontFamily: _fontFamily,
    fontSize: 15,
    fontWeight: FontWeight.w400,
    color: OkenaColors.textPrimary,
  );

  static const callout = TextStyle(
    fontFamily: _fontFamily,
    fontSize: 14,
    fontWeight: FontWeight.w400,
    color: OkenaColors.textSecondary,
  );

  static const caption = TextStyle(
    fontFamily: _fontFamily,
    fontSize: 12,
    fontWeight: FontWeight.w500,
    color: OkenaColors.textSecondary,
  );

  static const caption2 = TextStyle(
    fontFamily: _fontFamily,
    fontSize: 11,
    fontWeight: FontWeight.w500,
    color: OkenaColors.textTertiary,
  );
}

// ── Terminal theme ──────────────────────────────────────────────────────

class TerminalTheme {
  static const fontFamily = 'JetBrainsMono';
  static const fontFamilyFallback = [
    'Menlo',
    'Consolas',
    'DejaVu Sans Mono',
    'monospace',
  ];
  static const defaultFontSize = 13.0;
  static const minFontSize = 6.0;
  static const maxFontSize = 24.0;
  static const defaultColumns = 80;
  static const lineHeightFactor = 1.2;

  static const bgColor = Color(0xFF000000);
  static const fgColor = Color(0xFFCDD6F4);
  static const cursorColor = Color(0xFFF5E0DC);
  static const selectionColor = Color(0x40585B70);
}
