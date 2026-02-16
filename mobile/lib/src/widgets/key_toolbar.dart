import 'dart:ui';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../../src/rust/api/state.dart' as state_ffi;
import '../../src/rust/api/terminal.dart' as ffi;
import '../theme/app_theme.dart';

/// Three-state modifier cycle: inactive -> active (one-shot) -> locked (sticky).
enum ModifierState { inactive, active, locked }

/// Shared modifier state between [KeyToolbar] and [TerminalView].
class KeyModifiers extends ChangeNotifier {
  ModifierState _ctrl = ModifierState.inactive;
  ModifierState _option = ModifierState.inactive;
  ModifierState _cmd = ModifierState.inactive;

  bool get ctrl => _ctrl != ModifierState.inactive;
  bool get option => _option != ModifierState.inactive;
  bool get cmd => _cmd != ModifierState.inactive;
  bool get hasAny => ctrl || option || cmd;

  ModifierState get ctrlState => _ctrl;
  ModifierState get optionState => _option;
  ModifierState get cmdState => _cmd;

  /// Cycle: inactive -> active -> locked -> inactive.
  void toggleCtrl() { _ctrl = _nextState(_ctrl); notifyListeners(); }
  void toggleOption() { _option = _nextState(_option); notifyListeners(); }
  void toggleCmd() { _cmd = _nextState(_cmd); notifyListeners(); }

  static ModifierState _nextState(ModifierState s) => switch (s) {
    ModifierState.inactive => ModifierState.active,
    ModifierState.active   => ModifierState.locked,
    ModifierState.locked   => ModifierState.inactive,
  };

  /// Reset only one-shot (active) modifiers; locked ones persist.
  void reset() {
    final changed = _ctrl == ModifierState.active ||
        _option == ModifierState.active ||
        _cmd == ModifierState.active;
    if (!changed) return;
    if (_ctrl == ModifierState.active) _ctrl = ModifierState.inactive;
    if (_option == ModifierState.active) _option = ModifierState.inactive;
    if (_cmd == ModifierState.active) _cmd = ModifierState.inactive;
    notifyListeners();
  }
}

class KeyToolbar extends StatefulWidget {
  final String connId;
  final String? terminalId;
  final KeyModifiers modifiers;

  const KeyToolbar({
    super.key,
    required this.connId,
    this.terminalId,
    required this.modifiers,
  });

  @override
  State<KeyToolbar> createState() => _KeyToolbarState();
}

class _KeyToolbarState extends State<KeyToolbar> {
  KeyModifiers get _mod => widget.modifiers;

  // Arrow key name -> xterm suffix character
  static const _arrowChar = {
    'ArrowUp': 'A',
    'ArrowDown': 'B',
    'ArrowRight': 'C',
    'ArrowLeft': 'D',
  };

  void _sendSpecialKey(String key) {
    final tid = widget.terminalId;
    if (tid == null) return;
    state_ffi.sendSpecialKey(
      connId: widget.connId,
      terminalId: tid,
      key: key,
    );
  }

  void _sendText(String text) {
    final tid = widget.terminalId;
    if (tid == null) return;
    ffi.sendText(
      connId: widget.connId,
      terminalId: tid,
      text: text,
    );
  }

  /// Send a character key, applying any active modifiers.
  void _sendCharKey(String char) {
    if (_mod.hasAny) {
      if (_mod.ctrl) {
        final code = char.codeUnitAt(0);
        if (code >= 0x61 && code <= 0x7A) {
          _sendText(String.fromCharCode(code - 0x60));
        } else if (code >= 0x41 && code <= 0x5A) {
          _sendText(String.fromCharCode(code - 0x40));
        } else {
          _sendText(char);
        }
      } else {
        // Option/Cmd: ESC prefix
        _sendText('\x1b$char');
      }
      _mod.reset();
    } else {
      _sendText(char);
    }
  }

  /// Handle arrow from joystick, respecting modifier state.
  void _handleArrow(String key) {
    final arrow = _arrowChar[key];

    if (arrow != null && _mod.hasAny) {
      if (_mod.cmd && !_mod.ctrl && !_mod.option) {
        switch (key) {
          case 'ArrowLeft':
            _sendSpecialKey('Home');
          case 'ArrowRight':
            _sendSpecialKey('End');
          case 'ArrowUp':
            _sendSpecialKey('PageUp');
          case 'ArrowDown':
            _sendSpecialKey('PageDown');
        }
      } else {
        int mod = 1;
        if (_mod.ctrl) mod += 4;
        if (_mod.option) mod += 2;
        _sendText('\x1b[1;$mod$arrow');
      }
      _mod.reset();
    } else {
      _sendSpecialKey(key);
      if (_mod.hasAny) _mod.reset();
    }
  }

  Future<void> _paste() async {
    final data = await Clipboard.getData('text/plain');
    if (data?.text != null && data!.text!.isNotEmpty) {
      _sendText(data.text!);
    }
  }

  @override
  void initState() {
    super.initState();
    _mod.addListener(_onModChanged);
  }

  @override
  void didUpdateWidget(KeyToolbar old) {
    super.didUpdateWidget(old);
    if (old.modifiers != widget.modifiers) {
      old.modifiers.removeListener(_onModChanged);
      widget.modifiers.addListener(_onModChanged);
    }
  }

  @override
  void dispose() {
    _mod.removeListener(_onModChanged);
    super.dispose();
  }

  void _onModChanged() {
    if (mounted) setState(() {});
  }

  @override
  Widget build(BuildContext context) {
    return ClipRect(
      child: BackdropFilter(
        filter: ImageFilter.blur(sigmaX: 24, sigmaY: 24),
        child: Container(
          decoration: const BoxDecoration(
            color: OkenaColors.glassBg,
            border: Border(
              top: BorderSide(color: OkenaColors.glassStroke, width: 0.5),
            ),
          ),
          child: SafeArea(
            top: false,
            child: Padding(
              padding: const EdgeInsets.symmetric(horizontal: 6, vertical: 5),
              child: Row(
                children: [
                  // Scrollable button row
                  Expanded(
                    child: SingleChildScrollView(
                      scrollDirection: Axis.horizontal,
                      child: Row(
                        children: [
                          _Key(label: 'esc', onTap: () => _sendSpecialKey('Escape')),
                          _ToggleKey(
                            label: '\u2303',
                            state: _mod.ctrlState,
                            onTap: _mod.toggleCtrl,
                          ),
                          _ToggleKey(
                            label: '\u2325',
                            state: _mod.optionState,
                            onTap: _mod.toggleOption,
                          ),
                          _ToggleKey(
                            label: '\u2318',
                            state: _mod.cmdState,
                            onTap: _mod.toggleCmd,
                          ),
                          _Key(label: 'tab', onTap: () => _sendSpecialKey('Tab')),
                          const SizedBox(width: 12),
                          _Key(label: '~', onTap: () => _sendCharKey('~')),
                          _Key(label: '|', onTap: () => _sendCharKey('|')),
                          _Key(label: '/', onTap: () => _sendCharKey('/')),
                          _Key(label: '-', onTap: () => _sendCharKey('-')),
                          const SizedBox(width: 12),
                          _IconKey(
                            icon: Icons.content_paste_rounded,
                            onTap: _paste,
                          ),
                          _IconKey(
                            icon: Icons.keyboard_hide_rounded,
                            onTap: () => FocusScope.of(context).unfocus(),
                          ),
                        ],
                      ),
                    ),
                  ),
                  const SizedBox(width: 6),
                  // Fixed arrow joystick
                  _ArrowJoystick(onArrow: _handleArrow),
                ],
              ),
            ),
          ),
        ),
      ),
    );
  }
}

// ── Shared key widgets ─────────────────────────────────────────────────

class _Key extends StatelessWidget {
  final String label;
  final VoidCallback onTap;

  const _Key({required this.label, required this.onTap});

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 2),
      child: GestureDetector(
        onTap: () {
          HapticFeedback.lightImpact();
          onTap();
        },
        child: Container(
          constraints: const BoxConstraints(minWidth: 40),
          padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 9),
          decoration: BoxDecoration(
            color: OkenaColors.keyBg,
            borderRadius: BorderRadius.circular(10),
            border: Border.all(color: OkenaColors.keyBorder, width: 0.5),
          ),
          alignment: Alignment.center,
          child: Text(
            label,
            style: const TextStyle(
              color: OkenaColors.keyText,
              fontSize: 13,
              fontWeight: FontWeight.w500,
            ),
          ),
        ),
      ),
    );
  }
}

class _IconKey extends StatelessWidget {
  final IconData icon;
  final VoidCallback onTap;

  const _IconKey({required this.icon, required this.onTap});

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 2),
      child: GestureDetector(
        onTap: () {
          HapticFeedback.lightImpact();
          onTap();
        },
        child: Container(
          constraints: const BoxConstraints(minWidth: 40),
          padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 8),
          decoration: BoxDecoration(
            color: OkenaColors.keyBg,
            borderRadius: BorderRadius.circular(10),
            border: Border.all(color: OkenaColors.keyBorder, width: 0.5),
          ),
          alignment: Alignment.center,
          child: Icon(icon, color: OkenaColors.keyText, size: 17),
        ),
      ),
    );
  }
}

class _ToggleKey extends StatelessWidget {
  final String label;
  final ModifierState state;
  final VoidCallback onTap;

  const _ToggleKey({
    required this.label,
    required this.state,
    required this.onTap,
  });

  bool get _active => state != ModifierState.inactive;
  bool get _locked => state == ModifierState.locked;

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 2),
      child: GestureDetector(
        onTap: () {
          HapticFeedback.lightImpact();
          onTap();
        },
        child: AnimatedContainer(
          duration: const Duration(milliseconds: 150),
          curve: Curves.easeOutCubic,
          constraints: const BoxConstraints(minWidth: 40),
          padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 7),
          decoration: BoxDecoration(
            color: _active ? OkenaColors.accent : OkenaColors.keyBg,
            borderRadius: BorderRadius.circular(10),
            border: Border.all(
              color: _active ? OkenaColors.accent : OkenaColors.keyBorder,
              width: 0.5,
            ),
            boxShadow: _active
                ? [
                    BoxShadow(
                      color: OkenaColors.accent.withOpacity(0.35),
                      blurRadius: 12,
                      spreadRadius: -2,
                    ),
                  ]
                : null,
          ),
          alignment: Alignment.center,
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              Text(
                label,
                style: TextStyle(
                  color: _active ? Colors.white : OkenaColors.keyText,
                  fontSize: 16,
                  fontWeight: _active ? FontWeight.w700 : FontWeight.w500,
                ),
              ),
              // Small bar indicator for locked state
              AnimatedContainer(
                duration: const Duration(milliseconds: 150),
                curve: Curves.easeOutCubic,
                width: 12,
                height: 2,
                margin: const EdgeInsets.only(top: 1),
                decoration: BoxDecoration(
                  color: _locked ? Colors.white : Colors.transparent,
                  borderRadius: BorderRadius.circular(1),
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

// ── Arrow Joystick ─────────────────────────────────────────────────────

class _ArrowJoystick extends StatefulWidget {
  final ValueChanged<String> onArrow;

  const _ArrowJoystick({required this.onArrow});

  @override
  State<_ArrowJoystick> createState() => _ArrowJoystickState();
}

class _ArrowJoystickState extends State<_ArrowJoystick> {
  static const _size = 52.0;
  static const _dragThreshold = 14.0;

  String? _activeDirection;
  Offset _panOrigin = Offset.zero;
  bool _hasMoved = false;

  void _fire(String direction) {
    widget.onArrow(direction);
    HapticFeedback.selectionClick();
    setState(() => _activeDirection = direction);
  }

  void _onPanStart(DragStartDetails details) {
    _panOrigin = details.localPosition;
    _hasMoved = false;
  }

  void _onPanUpdate(DragUpdateDetails details) {
    final delta = details.localPosition - _panOrigin;
    if (delta.distance >= _dragThreshold) {
      _hasMoved = true;
      final dir = delta.dx.abs() > delta.dy.abs()
          ? (delta.dx > 0 ? 'ArrowRight' : 'ArrowLeft')
          : (delta.dy > 0 ? 'ArrowDown' : 'ArrowUp');
      _fire(dir);
      _panOrigin = details.localPosition;
    }
  }

  void _onPanEnd(DragEndDetails details) {
    if (!_hasMoved) {
      final center = const Offset(_size / 2, _size / 2);
      final delta = _panOrigin - center;
      if (delta.distance >= 4) {
        final dir = delta.dx.abs() > delta.dy.abs()
            ? (delta.dx > 0 ? 'ArrowRight' : 'ArrowLeft')
            : (delta.dy > 0 ? 'ArrowDown' : 'ArrowUp');
        _fire(dir);
        Future.delayed(const Duration(milliseconds: 120), () {
          if (mounted) setState(() => _activeDirection = null);
        });
        return;
      }
    }
    setState(() => _activeDirection = null);
  }

  @override
  Widget build(BuildContext context) {
    return GestureDetector(
      onPanStart: _onPanStart,
      onPanUpdate: _onPanUpdate,
      onPanEnd: _onPanEnd,
      child: Container(
        width: _size,
        height: _size,
        decoration: BoxDecoration(
          color: OkenaColors.keyBg,
          borderRadius: BorderRadius.circular(16),
          border: Border.all(color: OkenaColors.keyBorder, width: 0.5),
        ),
        child: CustomPaint(
          size: const Size(_size, _size),
          painter: _JoystickPainter(_activeDirection),
        ),
      ),
    );
  }
}

class _JoystickPainter extends CustomPainter {
  final String? activeDirection;

  _JoystickPainter(this.activeDirection);

  @override
  void paint(Canvas canvas, Size size) {
    final center = Offset(size.width / 2, size.height / 2);
    const armLength = 12.0;
    const gap = 3.0;
    const tipSize = 4.0;

    const dirs = {
      'ArrowUp': Offset(0, -1),
      'ArrowDown': Offset(0, 1),
      'ArrowLeft': Offset(-1, 0),
      'ArrowRight': Offset(1, 0),
    };

    for (final entry in dirs.entries) {
      final isActive = activeDirection == entry.key;
      final color = isActive ? OkenaColors.accent : const Color(0x61FFFFFF);
      final paint = Paint()
        ..color = color
        ..strokeWidth = 1.5
        ..strokeCap = StrokeCap.round;

      final d = entry.value;
      final armStart = center + d * gap;
      final armEnd = center + d * armLength;

      paint.style = PaintingStyle.stroke;
      canvas.drawLine(armStart, armEnd, paint);

      paint.style = PaintingStyle.fill;
      final path = Path();
      if (d.dy != 0) {
        path.moveTo(armEnd.dx, armEnd.dy);
        path.lineTo(armEnd.dx - tipSize, armEnd.dy - d.dy * tipSize);
        path.lineTo(armEnd.dx + tipSize, armEnd.dy - d.dy * tipSize);
      } else {
        path.moveTo(armEnd.dx, armEnd.dy);
        path.lineTo(armEnd.dx - d.dx * tipSize, armEnd.dy - tipSize);
        path.lineTo(armEnd.dx - d.dx * tipSize, armEnd.dy + tipSize);
      }
      path.close();
      canvas.drawPath(path, paint);
    }

    canvas.drawCircle(
      center,
      1.5,
      Paint()..color = const Color(0x3DFFFFFF),
    );
  }

  @override
  bool shouldRepaint(_JoystickPainter old) =>
      old.activeDirection != activeDirection;
}
