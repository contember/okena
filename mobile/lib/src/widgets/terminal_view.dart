import 'dart:async';
import 'dart:ui' as ui;

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../../src/rust/api/terminal.dart' as ffi;
import '../../src/rust/api/state.dart' as state_ffi;
import '../theme/app_theme.dart';
import 'terminal_painter.dart';

class TerminalView extends StatefulWidget {
  final String connId;
  final String terminalId;

  const TerminalView({
    super.key,
    required this.connId,
    required this.terminalId,
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
  final _textController = TextEditingController();
  String _lastInputText = '';

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
      _fetchCells();
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

    // Periodically reset to prevent unbounded string growth
    if (newText.length > 200) {
      _textController.clear();
      _lastInputText = '';
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
          onTap: () => _inputFocusNode.requestFocus(),
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
                  ),
                ),
                // Transparent text field overlay for soft keyboard input.
                // Must be in-layout (not off-screen) so Android shows the keyboard.
                Positioned(
                  left: 0,
                  bottom: 0,
                  width: 1,
                  height: 1,
                  child: Opacity(
                    opacity: 0,
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
