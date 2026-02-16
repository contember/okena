import 'dart:async';
import 'dart:ui' as ui;

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../../src/rust/api/terminal.dart' as ffi;
import '../../src/rust/api/state.dart' as state_ffi;
import '../theme/app_theme.dart';
import 'key_toolbar.dart' show KeyModifiers;
import 'terminal_painter.dart';

// Sentinel buffer: keeps spaces in the TextField so backspace always has
// something to delete. Without this, Android's soft keyboard backspace
// is a no-op on an empty field and onChanged never fires.
const _kSentinel = '        '; // 8 spaces

class TerminalView extends StatefulWidget {
  final String connId;
  final String terminalId;
  final KeyModifiers modifiers;

  const TerminalView({
    super.key,
    required this.connId,
    required this.terminalId,
    required this.modifiers,
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
  double _fontSize = TerminalTheme.defaultFontSize;
  double _baseFontSize = TerminalTheme.defaultFontSize;
  double _cellWidth = 0;
  double _cellHeight = 0;
  Timer? _refreshTimer;

  // Resize debounce
  Timer? _resizeTimer;

  // Scroll state
  double _scrollAccumulator = 0;

  // Pinch-to-zoom state
  final Map<int, Offset> _pointerPositions = {};
  bool _isPinching = false;
  bool _hasAutoFit = false;
  bool _initialResizeSent = false;
  double? _initialPinchDistance;

  // Keyboard input: TextField with its own FocusNode, delta-based tracking
  late final FocusNode _inputFocusNode;
  final _textController = TextEditingController(text: _kSentinel);
  String _lastInputText = _kSentinel;

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
      _hasAutoFit = false;
      _initialResizeSent = false;
      _fetchCells();
    }
  }

  @override
  void dispose() {
    _refreshTimer?.cancel();
    _resizeTimer?.cancel();
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
          fontFamilyFallback: TerminalTheme.fontFamilyFallback,
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

    // Auto-fit font size once for readable ~80 column display
    if (!_hasAutoFit) {
      _hasAutoFit = true;
      final charWidthRatio = _cellWidth / _fontSize;
      _fontSize = (constraints.maxWidth /
              (TerminalTheme.defaultColumns * charWidthRatio))
          .clamp(TerminalTheme.minFontSize, TerminalTheme.maxFontSize);
      _baseFontSize = _fontSize;
      _computeCellSize();
    }

    // Compute cols/rows that fit the mobile screen at the current font size
    final newCols =
        (constraints.maxWidth / _cellWidth).floor().clamp(1, 500);
    final newRows =
        (constraints.maxHeight / _cellHeight).floor().clamp(1, 200);

    if (newCols != _cols || newRows != _rows) {
      _cols = newCols;
      _rows = newRows;

      // Resize local grid immediately for responsive rendering
      ffi.resizeLocal(
        connId: widget.connId,
        terminalId: widget.terminalId,
        cols: _cols,
        rows: _rows,
      );

      if (!_initialResizeSent) {
        // First resize fires immediately — no flash of garbled content
        _initialResizeSent = true;
        _resizeTimer?.cancel();
        ffi.resizeTerminal(
          connId: widget.connId,
          terminalId: widget.terminalId,
          cols: _cols,
          rows: _rows,
        );
      } else {
        // Debounce subsequent resizes to avoid spamming during layout transitions
        _resizeTimer?.cancel();
        _resizeTimer = Timer(const Duration(milliseconds: 200), () {
          ffi.resizeTerminal(
            connId: widget.connId,
            terminalId: widget.terminalId,
            cols: _cols,
            rows: _rows,
          );
        });
      }
    }
  }

  void _resetSentinel() {
    _textController.text = _kSentinel;
    _textController.selection =
        TextSelection.collapsed(offset: _kSentinel.length);
    _lastInputText = _kSentinel;
  }

  void _onVerticalDragUpdate(DragUpdateDetails details) {
    if (_isPinching || _cellHeight <= 0) return;
    _scrollAccumulator += details.delta.dy;
    final lineDelta = (_scrollAccumulator / _cellHeight).truncate();
    if (lineDelta != 0) {
      _scrollAccumulator -= lineDelta * _cellHeight;
      ffi.scrollTerminal(
        connId: widget.connId,
        terminalId: widget.terminalId,
        delta: lineDelta,
      );
      _fetchCells();
    }
  }

  void _onVerticalDragEnd(DragEndDetails details) {
    _scrollAccumulator = 0;
  }

  // --- Pinch-to-zoom via raw pointer events ---

  double _computePinchDistance() {
    final points = _pointerPositions.values.toList();
    if (points.length < 2) return 0;
    return (points[0] - points[1]).distance;
  }

  void _onPointerDown(PointerDownEvent event) {
    _pointerPositions[event.pointer] = event.localPosition;
    if (_pointerPositions.length == 2) {
      _isPinching = true;
      _baseFontSize = _fontSize;
      _initialPinchDistance = _computePinchDistance();
    }
  }

  void _onPointerMove(PointerMoveEvent event) {
    _pointerPositions[event.pointer] = event.localPosition;
    if (_pointerPositions.length >= 2 &&
        _initialPinchDistance != null &&
        _initialPinchDistance! > 0) {
      final scale = _computePinchDistance() / _initialPinchDistance!;
      final newSize = (_baseFontSize * scale).clamp(
        TerminalTheme.minFontSize,
        TerminalTheme.maxFontSize,
      );
      if (newSize != _fontSize) {
        setState(() {
          _fontSize = newSize;
          _computeCellSize();
        });
      }
    }
  }

  void _onPointerUp(PointerUpEvent event) {
    _pointerPositions.remove(event.pointer);
    if (_pointerPositions.length < 2) {
      _isPinching = false;
      _initialPinchDistance = null;
    }
  }

  void _onPointerCancel(PointerCancelEvent event) {
    _pointerPositions.remove(event.pointer);
    if (_pointerPositions.length < 2) {
      _isPinching = false;
      _initialPinchDistance = null;
    }
  }

  void _scrollToBottom() {
    final offset = ffi.getDisplayOffset(
      connId: widget.connId,
      terminalId: widget.terminalId,
    );
    if (offset > 0) {
      ffi.scrollTerminal(
        connId: widget.connId,
        terminalId: widget.terminalId,
        delta: -offset,
      );
    }
  }

  String _applyModifiers(String chars) {
    final mod = widget.modifiers;
    if (!mod.hasAny) return chars;

    final buf = StringBuffer();
    for (final ch in chars.codeUnits) {
      if (mod.ctrl) {
        // Control character: a-z → 0x01-0x1A, A-Z → 0x01-0x1A
        if (ch >= 0x61 && ch <= 0x7A) {
          buf.writeCharCode(ch - 0x60);
        } else if (ch >= 0x41 && ch <= 0x5A) {
          buf.writeCharCode(ch - 0x40);
        }
      } else if (mod.option || mod.cmd) {
        // Meta/Option: ESC prefix + character
        buf.write('\x1b');
        buf.writeCharCode(ch);
      }
    }
    mod.reset();
    return buf.toString();
  }

  void _onTextChanged(String newText) {
    if (newText.length > _lastInputText.length) {
      // Characters added — send the delta.
      // Convert \n (from soft keyboard Return) to \r (terminal Enter).
      var delta = newText.substring(_lastInputText.length).replaceAll('\n', '\r');
      delta = _applyModifiers(delta);
      _scrollToBottom();
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
      _scrollToBottom();
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

        return Listener(
          onPointerDown: _onPointerDown,
          onPointerMove: _onPointerMove,
          onPointerUp: _onPointerUp,
          onPointerCancel: _onPointerCancel,
          child: GestureDetector(
            onVerticalDragUpdate: _onVerticalDragUpdate,
            onVerticalDragEnd: _onVerticalDragEnd,
            child: Container(
              color: OkenaColors.background,
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
                      devicePixelRatio: MediaQuery.devicePixelRatioOf(context),
                    ),
                  ),
                  // Transparent text field for soft keyboard input.
                  // Fills the terminal area so tapping anywhere opens the
                  // keyboard. Opacity > 0 keeps the iOS IME connected.
                  Positioned.fill(
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
                        keyboardType: TextInputType.multiline,
                        textInputAction: TextInputAction.newline,
                        maxLines: null,
                        decoration: const InputDecoration.collapsed(
                          hintText: '',
                        ),
                        style: const TextStyle(
                          color: Colors.transparent,
                          fontSize: 16,
                          height: 1,
                        ),
                      ),
                    ),
                  ),
                ],
              ),
            ),
          ),
        );
      },
    );
  }
}
