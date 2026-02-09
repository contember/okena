import 'package:flutter/material.dart';

import '../../src/rust/api/state.dart' as state_ffi;
import '../../src/rust/api/terminal.dart' as ffi;

class KeyToolbar extends StatefulWidget {
  final String connId;
  final String? terminalId;

  const KeyToolbar({
    super.key,
    required this.connId,
    this.terminalId,
  });

  @override
  State<KeyToolbar> createState() => _KeyToolbarState();
}

class _KeyToolbarState extends State<KeyToolbar> {
  bool _ctrlActive = false;
  bool _altActive = false;

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

  void _onCtrlTap() {
    setState(() => _ctrlActive = !_ctrlActive);
  }

  void _onAltTap() {
    setState(() => _altActive = !_altActive);
  }

  void _handleKey(String key) {
    if (_ctrlActive) {
      // CTRL+key: send control character
      if (key.length == 1) {
        final code = key.codeUnitAt(0);
        if (code >= 0x61 && code <= 0x7A) {
          // a-z → ctrl char
          _sendText(String.fromCharCode(code - 0x60));
        } else if (code >= 0x41 && code <= 0x5A) {
          // A-Z → ctrl char
          _sendText(String.fromCharCode(code - 0x40));
        }
      }
      setState(() => _ctrlActive = false);
    } else {
      _sendSpecialKey(key);
    }
    if (_altActive) {
      setState(() => _altActive = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    return Container(
      color: const Color(0xFF2A2A3E),
      child: SafeArea(
        top: false,
        child: SingleChildScrollView(
          scrollDirection: Axis.horizontal,
          child: Row(
            children: [
              _buildKey('ESC', () => _sendSpecialKey('Escape')),
              _buildKey('TAB', () => _sendSpecialKey('Tab')),
              _buildToggleKey('CTRL', _ctrlActive, _onCtrlTap),
              _buildToggleKey('ALT', _altActive, _onAltTap),
              const SizedBox(width: 4),
              _buildKey('\u2190', () => _handleKey('ArrowLeft')),
              _buildKey('\u2193', () => _handleKey('ArrowDown')),
              _buildKey('\u2191', () => _handleKey('ArrowUp')),
              _buildKey('\u2192', () => _handleKey('ArrowRight')),
            ],
          ),
        ),
      ),
    );
  }

  Widget _buildKey(String label, VoidCallback onTap) {
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 2, vertical: 6),
      child: Material(
        color: const Color(0xFF363650),
        borderRadius: BorderRadius.circular(6),
        child: InkWell(
          borderRadius: BorderRadius.circular(6),
          onTap: onTap,
          child: Container(
            constraints: const BoxConstraints(minWidth: 44),
            padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 8),
            alignment: Alignment.center,
            child: Text(
              label,
              style: const TextStyle(
                color: Colors.white70,
                fontSize: 13,
                fontFamily: 'JetBrainsMono',
              ),
            ),
          ),
        ),
      ),
    );
  }

  Widget _buildToggleKey(String label, bool active, VoidCallback onTap) {
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 2, vertical: 6),
      child: Material(
        color: active ? Colors.blue.shade700 : const Color(0xFF363650),
        borderRadius: BorderRadius.circular(6),
        child: InkWell(
          borderRadius: BorderRadius.circular(6),
          onTap: onTap,
          child: Container(
            constraints: const BoxConstraints(minWidth: 44),
            padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 8),
            alignment: Alignment.center,
            child: Text(
              label,
              style: TextStyle(
                color: active ? Colors.white : Colors.white70,
                fontSize: 13,
                fontWeight: active ? FontWeight.bold : FontWeight.normal,
                fontFamily: 'JetBrainsMono',
              ),
            ),
          ),
        ),
      ),
    );
  }
}
