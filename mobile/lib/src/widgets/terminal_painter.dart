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
  final SelectionBounds? selection;
  final ScrollInfo? scrollInfo;

  TerminalPainter({
    required this.cells,
    required this.cursor,
    required this.cols,
    required this.rows,
    required this.cellWidth,
    required this.cellHeight,
    required this.fontSize,
    required this.fontFamily,
    this.selection,
    this.scrollInfo,
  });

  bool _isCellInSelection(int col, int row, SelectionBounds sel) {
    // Selection bounds are in buffer coordinates; convert visual row
    // to buffer row for comparison: buffer_row = visual_row - display_offset
    final offset = scrollInfo?.displayOffset.toInt() ?? 0;
    final bufferRow = row - offset;

    final sr = sel.startRow;
    final er = sel.endRow;
    final sc = sel.startCol;
    final ec = sel.endCol;

    if (bufferRow < sr || bufferRow > er) return false;
    if (sr == er) {
      // Single line selection
      return col >= sc && col <= ec;
    }
    if (bufferRow == sr) return col >= sc;
    if (bufferRow == er) return col <= ec;
    return true; // Middle line — fully selected
  }

  @override
  void paint(Canvas canvas, Size size) {
    final bgPaint = Paint();

    // Pass 1: Background rectangles + selection highlight
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

      // Selection highlight
      if (selection != null && _isCellInSelection(col, row, selection!)) {
        bgPaint.color = const Color(0x40585B70);
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

    // Pass 4: Scroll indicator
    if (scrollInfo != null && scrollInfo!.totalLines > scrollInfo!.visibleLines) {
      final total = scrollInfo!.totalLines;
      final visible = scrollInfo!.visibleLines;
      final offset = scrollInfo!.displayOffset;

      if (total > 0 && visible > 0) {
        final trackHeight = size.height;
        final thumbHeight = (visible / total * trackHeight).clamp(20.0, trackHeight);
        // offset=0 means at bottom, max offset = total - visible
        final maxOffset = total - visible;
        final thumbTop = maxOffset > 0
            ? (1.0 - offset / maxOffset) * (trackHeight - thumbHeight)
            : 0.0;

        final scrollPaint = Paint()
          ..color = const Color(0x40FFFFFF)
          ..style = PaintingStyle.fill;
        canvas.drawRRect(
          RRect.fromRectAndRadius(
            Rect.fromLTWH(size.width - 4, thumbTop, 3, thumbHeight),
            const Radius.circular(1.5),
          ),
          scrollPaint,
        );
      }
    }
  }

  @override
  bool shouldRepaint(TerminalPainter oldDelegate) => true;
}
