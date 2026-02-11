import 'dart:async';
import 'dart:ui' as ui;

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../../src/rust/api/terminal.dart' as ffi;
import '../../src/rust/api/state.dart' as state_ffi;
import '../theme/app_theme.dart';
import 'terminal_painter.dart';

// Sentinel buffer: keeps spaces in the TextField so backspace always has
// something to delete. Without this, Android's soft keyboard backspace
// is a no-op on an empty field and onChanged never fires.
const _kSentinel = '        '; // 8 spaces

class TerminalView extends StatefulWidget {
  final String connId;
  final String terminalId;

  /// Called when the user swipes horizontally to switch terminals.
  /// direction: -1 = swipe right (prev), 1 = swipe left (next).
  final ValueChanged<int>? onTerminalSwipe;

  const TerminalView({
    super.key,
    required this.connId,
    required this.terminalId,
    this.onTerminalSwipe,
  });

  @override
  State<TerminalView> createState() => _TerminalViewState();
}

class _TerminalViewState extends State<TerminalView> {
  List<ffi.CellData> _cells = [];
  ffi.CursorState _cursor = const ffi.CursorState(
    col: 0,
    row: 0,
    shape: ffi.CursorShape.block,
    visible: true,
  );
  int _cols = 80;
  int _rows = 24;
  final double _fontSize = TerminalTheme.defaultFontSize;
  double _cellWidth = 0;
  double _cellHeight = 0;
  Timer? _refreshTimer;
  Timer? _resizeDebounce;

  // Keyboard input: TextField with its own FocusNode, delta-based tracking
  late final FocusNode _inputFocusNode;
  final _textController = TextEditingController(text: _kSentinel);
  String _lastInputText = _kSentinel;

  // Scroll state
  ffi.ScrollInfo _scrollInfo = const ffi.ScrollInfo(
    totalLines: 0,
    visibleLines: 0,
    displayOffset: 0,
  );
  double _scrollAccumulator = 0;

  // Selection state
  bool _isSelecting = false;
  ffi.SelectionBounds? _selection;

  // Gesture tracking for scroll vs swipe disambiguation
  Offset? _dragStart;

  @override
  void initState() {
    super.initState();
    _inputFocusNode = FocusNode(onKeyEvent: _onKeyEvent);
    _computeCellSize();
    _startRefreshLoop();
  }

  @override
  void didUpdateWidget(TerminalView oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.connId != widget.connId ||
        oldWidget.terminalId != widget.terminalId) {
      _isSelecting = false;
      _selection = null;
      _fetchCells();
      // Force resize so the new terminal matches the mobile viewport
      if (_cols > 0 && _rows > 0) {
        ffi.resizeTerminal(
          connId: widget.connId,
          terminalId: widget.terminalId,
          cols: _cols,
          rows: _rows,
        );
      }
    }
  }

  @override
  void dispose() {
    _refreshTimer?.cancel();
    _resizeDebounce?.cancel();
    _inputFocusNode.dispose();
    _textController.dispose();
    super.dispose();
  }

  void _computeCellSize() {
    final tp = TextPainter(
      text: TextSpan(
        text: 'M',
        style: TextStyle(
          fontFamily: TerminalTheme.fontFamily,
          fontSize: _fontSize,
        ),
      ),
      textDirection: ui.TextDirection.ltr,
    )..layout();

    _cellWidth = tp.width;
    _cellHeight = tp.height * TerminalTheme.lineHeightFactor;
  }

  void _startRefreshLoop() {
    _refreshTimer = Timer.periodic(
      const Duration(milliseconds: 33), // ~30fps
      (_) => _checkDirty(),
    );
  }

  void _checkDirty() {
    if (!mounted) return;
    final dirty =
        state_ffi.isDirty(connId: widget.connId, terminalId: widget.terminalId);
    if (dirty) {
      _fetchCells();
    }
  }

  void _fetchCells() {
    if (!mounted) return;
    setState(() {
      _cells = ffi.getVisibleCells(
        connId: widget.connId,
        terminalId: widget.terminalId,
      );
      _cursor = ffi.getCursor(
        connId: widget.connId,
        terminalId: widget.terminalId,
      );
      _scrollInfo = ffi.getScrollInfo(
        connId: widget.connId,
        terminalId: widget.terminalId,
      );
      if (_isSelecting) {
        _selection = ffi.getSelectionBounds(
          connId: widget.connId,
          terminalId: widget.terminalId,
        );
      }
    });
  }

  void _onLayout(BoxConstraints constraints) {
    if (_cellWidth <= 0 || _cellHeight <= 0) return;

    final newCols = (constraints.maxWidth / _cellWidth).floor().clamp(1, 500);
    final newRows = (constraints.maxHeight / _cellHeight).floor().clamp(1, 200);

    if (newCols != _cols || newRows != _rows) {
      _cols = newCols;
      _rows = newRows;
      _resizeDebounce?.cancel();
      _resizeDebounce = Timer(const Duration(milliseconds: 200), () {
        ffi.resizeTerminal(
          connId: widget.connId,
          terminalId: widget.terminalId,
          cols: _cols,
          rows: _rows,
        );
      });
    }
  }

  // --- Touch to cell conversion ---
  (int col, int row) _touchToCell(Offset pos) {
    final col = (pos.dx / _cellWidth).floor().clamp(0, _cols - 1);
    final row = (pos.dy / _cellHeight).floor().clamp(0, _rows - 1);
    return (col, row);
  }

  // --- Scroll handling ---
  void _onVerticalDragUpdate(DragUpdateDetails details) {
    if (_cellHeight <= 0) return;
    _scrollAccumulator += -details.delta.dy;
    final lines = (_scrollAccumulator / _cellHeight).truncate();
    if (lines != 0) {
      _scrollAccumulator -= lines * _cellHeight;
      ffi.scroll(
        connId: widget.connId,
        terminalId: widget.terminalId,
        delta: lines,
      );
      _fetchCells();
    }
  }

  void _onVerticalDragEnd(DragEndDetails details) {
    _scrollAccumulator = 0;
  }

  // --- Selection handling ---
  void _onLongPressStart(LongPressStartDetails details) {
    final (col, row) = _touchToCell(details.localPosition);
    ffi.startSelection(
      connId: widget.connId,
      terminalId: widget.terminalId,
      col: col,
      row: row,
    );
    setState(() {
      _isSelecting = true;
      _selection = ffi.getSelectionBounds(
        connId: widget.connId,
        terminalId: widget.terminalId,
      );
    });
  }

  void _onLongPressMoveUpdate(LongPressMoveUpdateDetails details) {
    if (!_isSelecting) return;
    final (col, row) = _touchToCell(details.localPosition);
    ffi.updateSelection(
      connId: widget.connId,
      terminalId: widget.terminalId,
      col: col,
      row: row,
    );
    setState(() {
      _selection = ffi.getSelectionBounds(
        connId: widget.connId,
        terminalId: widget.terminalId,
      );
    });
  }

  void _onLongPressEnd(LongPressEndDetails details) {
    if (!_isSelecting) return;
    _copySelectionAndClear();
  }

  void _onDoubleTap() {
    // Use the last known tap position for word selection
    if (_lastTapPosition != null) {
      final (col, row) = _touchToCell(_lastTapPosition!);
      ffi.startWordSelection(
        connId: widget.connId,
        terminalId: widget.terminalId,
        col: col,
        row: row,
      );
      setState(() {
        _isSelecting = true;
        _selection = ffi.getSelectionBounds(
          connId: widget.connId,
          terminalId: widget.terminalId,
        );
      });
      _copySelectionAndClear();
    }
  }

  Offset? _lastTapPosition;

  void _onTapDown(TapDownDetails details) {
    _lastTapPosition = details.localPosition;
  }

  void _onTap() {
    // Clear selection on single tap if one exists
    if (_isSelecting) {
      ffi.clearSelection(
        connId: widget.connId,
        terminalId: widget.terminalId,
      );
      setState(() {
        _isSelecting = false;
        _selection = null;
      });
      return;
    }
    _inputFocusNode.requestFocus();
  }

  void _copySelectionAndClear() {
    final text = ffi.getSelectedText(
      connId: widget.connId,
      terminalId: widget.terminalId,
    );
    if (text != null && text.isNotEmpty) {
      Clipboard.setData(ClipboardData(text: text));
      if (mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          const SnackBar(
            content: Text('Copied to clipboard'),
            duration: Duration(seconds: 1),
          ),
        );
      }
    }
    ffi.clearSelection(
      connId: widget.connId,
      terminalId: widget.terminalId,
    );
    setState(() {
      _isSelecting = false;
      _selection = null;
    });
  }

  // --- Horizontal swipe handling ---
  void _onHorizontalDragStart(DragStartDetails details) {
    _dragStart = details.localPosition;
  }

  void _onHorizontalDragEnd(DragEndDetails details) {
    final velocity = details.velocity.pixelsPerSecond;
    if (velocity.dx.abs() > 300 && _dragStart != null) {
      final direction = velocity.dx > 0 ? -1 : 1; // right swipe = prev, left = next
      widget.onTerminalSwipe?.call(direction);
    }
    _dragStart = null;
  }

  void _resetSentinel() {
    _textController.text = _kSentinel;
    _textController.selection =
        TextSelection.collapsed(offset: _kSentinel.length);
    _lastInputText = _kSentinel;
  }

  void _onTextChanged(String newText) {
    if (newText.length > _lastInputText.length) {
      // Characters added — send the delta
      final delta = newText.substring(_lastInputText.length);
      ffi.sendText(
        connId: widget.connId,
        terminalId: widget.terminalId,
        text: delta,
      );
    } else if (newText.length < _lastInputText.length) {
      // Characters deleted — user pressed backspace
      final deletedCount = _lastInputText.length - newText.length;
      for (int i = 0; i < deletedCount; i++) {
        state_ffi.sendSpecialKey(
          connId: widget.connId,
          terminalId: widget.terminalId,
          key: 'Backspace',
        );
      }
    }
    _lastInputText = newText;

    // Re-seed if buffer runs low (backspace ate into the sentinel)
    if (newText.length < 3) {
      _resetSentinel();
    }
    // Reset if too long to prevent unbounded growth
    if (newText.length > 200) {
      _resetSentinel();
    }
  }

  KeyEventResult _onKeyEvent(FocusNode node, KeyEvent event) {
    if (event is! KeyDownEvent && event is! KeyRepeatEvent) {
      return KeyEventResult.ignored;
    }

    final key = event.logicalKey;
    String? specialKey;

    if (key == LogicalKeyboardKey.enter) {
      specialKey = 'Enter';
    } else if (key == LogicalKeyboardKey.backspace) {
      specialKey = 'Backspace';
    } else if (key == LogicalKeyboardKey.arrowUp) {
      specialKey = 'ArrowUp';
    } else if (key == LogicalKeyboardKey.arrowDown) {
      specialKey = 'ArrowDown';
    } else if (key == LogicalKeyboardKey.arrowLeft) {
      specialKey = 'ArrowLeft';
    } else if (key == LogicalKeyboardKey.arrowRight) {
      specialKey = 'ArrowRight';
    } else if (key == LogicalKeyboardKey.home) {
      specialKey = 'Home';
    } else if (key == LogicalKeyboardKey.end) {
      specialKey = 'End';
    } else if (key == LogicalKeyboardKey.pageUp) {
      specialKey = 'PageUp';
    } else if (key == LogicalKeyboardKey.pageDown) {
      specialKey = 'PageDown';
    } else if (key == LogicalKeyboardKey.delete) {
      specialKey = 'Delete';
    } else if (key == LogicalKeyboardKey.tab) {
      specialKey = 'Tab';
    } else if (key == LogicalKeyboardKey.escape) {
      specialKey = 'Escape';
    }

    if (specialKey != null) {
      state_ffi.sendSpecialKey(
        connId: widget.connId,
        terminalId: widget.terminalId,
        key: specialKey,
      );
      return KeyEventResult.handled;
    }

    // Let TextField handle normal character input via onChanged
    return KeyEventResult.ignored;
  }

  @override
  Widget build(BuildContext context) {
    return LayoutBuilder(
      builder: (context, constraints) {
        _onLayout(constraints);

        return GestureDetector(
          onTapDown: _onTapDown,
          onTap: _onTap,
          onDoubleTap: _onDoubleTap,
          // Vertical drag for scrollback
          onVerticalDragUpdate: _onVerticalDragUpdate,
          onVerticalDragEnd: _onVerticalDragEnd,
          // Long press for selection
          onLongPressStart: _onLongPressStart,
          onLongPressMoveUpdate: _onLongPressMoveUpdate,
          onLongPressEnd: _onLongPressEnd,
          // Horizontal drag for terminal switching
          onHorizontalDragStart: _onHorizontalDragStart,
          onHorizontalDragEnd: _onHorizontalDragEnd,
          behavior: HitTestBehavior.opaque,
          child: Container(
            color: TerminalTheme.bgColor,
            width: constraints.maxWidth,
            height: constraints.maxHeight,
            child: Stack(
              children: [
                // Terminal canvas
                CustomPaint(
                  size: Size(constraints.maxWidth, constraints.maxHeight),
                  painter: TerminalPainter(
                    cells: _cells,
                    cursor: _cursor,
                    cols: _cols,
                    rows: _rows,
                    cellWidth: _cellWidth,
                    cellHeight: _cellHeight,
                    fontSize: _fontSize,
                    fontFamily: TerminalTheme.fontFamily,
                    selection: _selection,
                    scrollInfo: _scrollInfo,
                  ),
                ),
                // Transparent text field for soft keyboard input.
                // Sized 1x1 in-layout (not off-screen) so Android shows the
                // keyboard. Opacity > 0 to keep IME interaction working.
                Positioned(
                  left: 0,
                  bottom: 0,
                  width: 1,
                  height: 1,
                  child: Opacity(
                    opacity: 0.01,
                    child: TextField(
                      focusNode: _inputFocusNode,
                      controller: _textController,
                      autofocus: false,
                      enableSuggestions: false,
                      autocorrect: false,
                      showCursor: false,
                      enableInteractiveSelection: false,
                      onChanged: _onTextChanged,
                      keyboardType: TextInputType.text,
                      textInputAction: TextInputAction.none,
                      decoration: const InputDecoration.collapsed(
                        hintText: '',
                      ),
                      style: const TextStyle(
                        color: Colors.transparent,
                        fontSize: 1,
                        height: 1,
                      ),
                    ),
                  ),
                ),
              ],
            ),
          ),
        );
      },
    );
  }
}
