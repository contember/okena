import 'dart:ui' as ui;

import 'package:flutter/material.dart';

import '../../src/rust/api/terminal.dart';
import '../theme/app_theme.dart';

// Flag bitmask constants matching Rust CellData.flags
const _kBold = 1;
const _kItalic = 2;
const _kUnderline = 4;
const _kStrikethrough = 8;
const _kInverse = 16;
const _kDim = 32;

// Mask for flags that affect text style (excludes _kInverse which is handled
// separately when computing effective fg/bg).
const _kStyleMask = _kBold | _kItalic | _kUnderline | _kStrikethrough | _kDim;

TextDecoration flagsToDecoration(int flags) {
  final decorations = <TextDecoration>[];
  if (flags & _kUnderline != 0) decorations.add(TextDecoration.underline);
  if (flags & _kStrikethrough != 0) {
    decorations.add(TextDecoration.lineThrough);
  }
  if (decorations.isEmpty) return TextDecoration.none;
  return TextDecoration.combine(decorations);
}

Color argbToColor(int argb) {
  // Rust sends ARGB as u32: 0xAARRGGBB
  return Color(argb);
}

class TerminalPainter extends CustomPainter {
  final List<CellData> cells;
  final CursorState cursor;
  final int cols;
  final int rows;
  final double cellWidth;
  final double cellHeight;
  final double fontSize;
  final String fontFamily;
  final double devicePixelRatio;

  TerminalPainter({
    required this.cells,
    required this.cursor,
    required this.cols,
    required this.rows,
    required this.cellWidth,
    required this.cellHeight,
    required this.fontSize,
    required this.fontFamily,
    required this.devicePixelRatio,
  });

  /// Snap a logical coordinate to device pixel boundaries.
  double _snap(double v) =>
      (v * devicePixelRatio).roundToDouble() / devicePixelRatio;

  @override
  void paint(Canvas canvas, Size size) {
    final bgPaint = Paint();

    // Pass 1: Background rectangles
    for (int i = 0; i < cells.length && i < cols * rows; i++) {
      final cell = cells[i];
      final col = i % cols;
      final row = i ~/ cols;
      final x = _snap(col * cellWidth);
      final y = _snap(row * cellHeight);

      var bgArgb = cell.bg;
      var fgArgb = cell.fg;
      if (cell.flags & _kInverse != 0) {
        final tmp = bgArgb;
        bgArgb = fgArgb;
        fgArgb = tmp;
      }

      final bgColor = argbToColor(bgArgb);
      // Only draw non-default backgrounds
      if (bgColor != OkenaColors.background && bgColor.a > 0) {
        bgPaint.color = bgColor;
        canvas.drawRect(Rect.fromLTWH(x, y, cellWidth, cellHeight), bgPaint);
      }
    }

    // Pass 2: Text characters — batched by style runs within each row.
    // Consecutive non-space cells with the same effective fg + style flags
    // are concatenated into a single TextPainter call, reducing allocations
    // from ~cols*rows down to the number of distinct style runs.
    for (int row = 0; row < rows; row++) {
      int col = 0;
      while (col < cols) {
        final idx = row * cols + col;
        if (idx >= cells.length) break;

        final cell = cells[idx];
        if (cell.character.isEmpty || cell.character == ' ') {
          col++;
          continue;
        }

        // Determine effective fg for the first cell of the run.
        var fgArgb = cell.fg;
        if (cell.flags & _kInverse != 0) fgArgb = cell.bg;
        final styleFlags = cell.flags & _kStyleMask;

        final startCol = col;
        final buffer = StringBuffer();
        buffer.write(cell.character);
        col++;

        // Extend run with consecutive cells sharing the same style.
        while (col < cols) {
          final ci = row * cols + col;
          if (ci >= cells.length) break;
          final c = cells[ci];
          if (c.character.isEmpty || c.character == ' ') break;

          var cFg = c.fg;
          if (c.flags & _kInverse != 0) cFg = c.bg;
          final cStyleFlags = c.flags & _kStyleMask;

          if (cFg != fgArgb || cStyleFlags != styleFlags) break;

          buffer.write(c.character);
          col++;
        }

        var fgColor = argbToColor(fgArgb);
        if (styleFlags & _kDim != 0) {
          fgColor = fgColor.withAlpha((fgColor.a * 0.5).round());
        }

        final tp = TextPainter(
          text: TextSpan(
            text: buffer.toString(),
            style: TextStyle(
              fontFamily: fontFamily,
              fontFamilyFallback: TerminalTheme.fontFamilyFallback,
              fontSize: fontSize,
              color: fgColor,
              fontWeight:
                  styleFlags & _kBold != 0 ? FontWeight.bold : FontWeight.normal,
              fontStyle:
                  styleFlags & _kItalic != 0 ? FontStyle.italic : FontStyle.normal,
              decoration: flagsToDecoration(styleFlags),
              decorationColor: fgColor,
            ),
          ),
          textDirection: ui.TextDirection.ltr,
        )..layout();

        final x = _snap(startCol * cellWidth);
        final y = _snap(row * cellHeight);
        final dy = y + (cellHeight - tp.height) / 2;
        tp.paint(canvas, Offset(x, dy));
      }
    }

    // Pass 3: Cursor
    if (cursor.visible && cursor.col < cols && cursor.row < rows) {
      final cx = _snap(cursor.col * cellWidth);
      final cy = _snap(cursor.row * cellHeight);
      final cursorPaint = Paint()..color = TerminalTheme.cursorColor;

      switch (cursor.shape) {
        case CursorShape.block:
          cursorPaint.color = TerminalTheme.cursorColor.withAlpha(128);
          canvas.drawRect(
            Rect.fromLTWH(cx, cy, cellWidth, cellHeight),
            cursorPaint,
          );
        case CursorShape.beam:
          cursorPaint.strokeWidth = 2;
          canvas.drawLine(
            Offset(cx, cy),
            Offset(cx, cy + cellHeight),
            cursorPaint,
          );
        case CursorShape.underline:
          cursorPaint.strokeWidth = 2;
          canvas.drawLine(
            Offset(cx, cy + cellHeight - 1),
            Offset(cx + cellWidth, cy + cellHeight - 1),
            cursorPaint,
          );
      }
    }
  }

  @override
  bool shouldRepaint(TerminalPainter oldDelegate) {
    return !identical(cells, oldDelegate.cells) ||
        cursor.col != oldDelegate.cursor.col ||
        cursor.row != oldDelegate.cursor.row ||
        cursor.shape != oldDelegate.cursor.shape ||
        cursor.visible != oldDelegate.cursor.visible ||
        cols != oldDelegate.cols ||
        rows != oldDelegate.rows ||
        fontSize != oldDelegate.fontSize;
  }
}
