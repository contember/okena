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

  TerminalPainter({
    required this.cells,
    required this.cursor,
    required this.cols,
    required this.rows,
    required this.cellWidth,
    required this.cellHeight,
    required this.fontSize,
    required this.fontFamily,
  });

  @override
  void paint(Canvas canvas, Size size) {
    final bgPaint = Paint();

    // Pass 1: Background rectangles
    for (int i = 0; i < cells.length && i < cols * rows; i++) {
      final cell = cells[i];
      final col = i % cols;
      final row = i ~/ cols;
      final x = col * cellWidth;
      final y = row * cellHeight;

      var bgArgb = cell.bg;
      var fgArgb = cell.fg;
      if (cell.flags & _kInverse != 0) {
        final tmp = bgArgb;
        bgArgb = fgArgb;
        fgArgb = tmp;
      }

      final bgColor = argbToColor(bgArgb);
      // Only draw non-default backgrounds
      if (bgColor != TerminalTheme.bgColor && bgColor.a > 0) {
        bgPaint.color = bgColor;
        canvas.drawRect(Rect.fromLTWH(x, y, cellWidth, cellHeight), bgPaint);
      }
    }

    // Pass 2: Text characters
    for (int i = 0; i < cells.length && i < cols * rows; i++) {
      final cell = cells[i];
      if (cell.character.isEmpty || cell.character == ' ') continue;

      final col = i % cols;
      final row = i ~/ cols;
      final x = col * cellWidth;
      final y = row * cellHeight;

      var fgArgb = cell.fg;
      var bgArgb = cell.bg;
      if (cell.flags & _kInverse != 0) {
        fgArgb = bgArgb;
        // Don't need bgArgb here for text painting
      }

      var fgColor = argbToColor(fgArgb);
      if (cell.flags & _kDim != 0) {
        fgColor = fgColor.withAlpha((fgColor.a * 0.5).round());
      }

      final tp = TextPainter(
        text: TextSpan(
          text: cell.character,
          style: TextStyle(
            fontFamily: fontFamily,
            fontSize: fontSize,
            color: fgColor,
            fontWeight:
                cell.flags & _kBold != 0 ? FontWeight.bold : FontWeight.normal,
            fontStyle:
                cell.flags & _kItalic != 0 ? FontStyle.italic : FontStyle.normal,
            decoration: flagsToDecoration(cell.flags),
            decorationColor: fgColor,
          ),
        ),
        textDirection: ui.TextDirection.ltr,
      )..layout();

      // Center the character in the cell
      final dx = x + (cellWidth - tp.width) / 2;
      final dy = y + (cellHeight - tp.height) / 2;
      tp.paint(canvas, Offset(dx, dy));
    }

    // Pass 3: Cursor
    if (cursor.visible && cursor.col < cols && cursor.row < rows) {
      final cx = cursor.col * cellWidth;
      final cy = cursor.row * cellHeight;
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
  bool shouldRepaint(TerminalPainter oldDelegate) => true;
}
