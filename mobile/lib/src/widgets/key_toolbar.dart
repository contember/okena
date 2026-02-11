import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:shared_preferences/shared_preferences.dart';

import '../../src/rust/api/state.dart' as state_ffi;
import '../../src/rust/api/terminal.dart' as ffi;

const _kComposeHistoryKey = 'compose_history';
const _kMaxHistory = 30;

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

  // Compose history
  List<String> _composeHistory = [];

  @override
  void initState() {
    super.initState();
    _loadComposeHistory();
  }

  Future<void> _loadComposeHistory() async {
    final prefs = await SharedPreferences.getInstance();
    final history = prefs.getStringList(_kComposeHistoryKey);
    if (history != null) {
      setState(() => _composeHistory = history);
    }
  }

  Future<void> _saveComposeHistory() async {
    final prefs = await SharedPreferences.getInstance();
    await prefs.setStringList(_kComposeHistoryKey, _composeHistory);
  }

  void _addToHistory(String text) {
    _composeHistory.remove(text); // dedup
    _composeHistory.insert(0, text);
    if (_composeHistory.length > _kMaxHistory) {
      _composeHistory = _composeHistory.sublist(0, _kMaxHistory);
    }
    _saveComposeHistory();
  }

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

  void _sendCtrlChar(String letter) {
    final code = letter.toLowerCase().codeUnitAt(0);
    if (code >= 0x61 && code <= 0x7A) {
      _sendText(String.fromCharCode(code - 0x60));
    }
  }

  void _onCtrlTap() {
    HapticFeedback.lightImpact();
    setState(() => _ctrlActive = !_ctrlActive);
  }

  void _onAltTap() {
    HapticFeedback.lightImpact();
    setState(() => _altActive = !_altActive);
  }

  void _handleKey(String key) {
    if (_ctrlActive) {
      if (key.length == 1) {
        final code = key.codeUnitAt(0);
        if (code >= 0x61 && code <= 0x7A) {
          _sendText(String.fromCharCode(code - 0x60));
        } else if (code >= 0x41 && code <= 0x5A) {
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

  void _pasteFromClipboard() async {
    HapticFeedback.lightImpact();
    final data = await Clipboard.getData(Clipboard.kTextPlain);
    if (data?.text != null && data!.text!.isNotEmpty) {
      _sendText(data.text!);
    }
  }

  void _showCtrlGrid() {
    const shortcuts = [
      ('C', 'kill'),
      ('D', 'eof'),
      ('Z', 'suspend'),
      ('L', 'clear'),
      ('A', 'bol'),
      ('E', 'eol'),
      ('R', 'search'),
      ('W', 'del word'),
      ('U', 'del left'),
      ('K', 'del right'),
      ('P', 'prev'),
      ('N', 'next'),
    ];

    showModalBottomSheet(
      context: context,
      backgroundColor: const Color(0xFF2A2A3E),
      shape: const RoundedRectangleBorder(
        borderRadius: BorderRadius.vertical(top: Radius.circular(16)),
      ),
      builder: (ctx) {
        return Padding(
          padding: const EdgeInsets.fromLTRB(12, 16, 12, 16),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              const Padding(
                padding: EdgeInsets.only(left: 4, bottom: 12),
                child: Text(
                  'CTRL + ...',
                  style: TextStyle(
                    color: Colors.white54,
                    fontSize: 13,
                    fontFamily: 'JetBrainsMono',
                  ),
                ),
              ),
              GridView.count(
                shrinkWrap: true,
                crossAxisCount: 4,
                mainAxisSpacing: 6,
                crossAxisSpacing: 6,
                childAspectRatio: 1.6,
                physics: const NeverScrollableScrollPhysics(),
                children: shortcuts.map((s) {
                  final (letter, label) = s;
                  return Material(
                    color: const Color(0xFF363650),
                    borderRadius: BorderRadius.circular(8),
                    child: InkWell(
                      borderRadius: BorderRadius.circular(8),
                      onTap: () {
                        HapticFeedback.lightImpact();
                        _sendCtrlChar(letter);
                        Navigator.of(ctx).pop();
                      },
                      child: Column(
                        mainAxisAlignment: MainAxisAlignment.center,
                        children: [
                          Text(
                            '^$letter',
                            style: const TextStyle(
                              color: Colors.white,
                              fontSize: 15,
                              fontWeight: FontWeight.bold,
                              fontFamily: 'JetBrainsMono',
                            ),
                          ),
                          const SizedBox(height: 2),
                          Text(
                            label,
                            style: const TextStyle(
                              color: Colors.white38,
                              fontSize: 10,
                            ),
                          ),
                        ],
                      ),
                    ),
                  );
                }).toList(),
              ),
              SizedBox(height: MediaQuery.of(ctx).padding.bottom),
            ],
          ),
        );
      },
    );
  }

  void _showComposeSheet() {
    final controller = TextEditingController();
    bool sendEnter = true;
    int historyIdx = -1;

    showModalBottomSheet(
      context: context,
      isScrollControlled: true,
      backgroundColor: const Color(0xFF2A2A3E),
      shape: const RoundedRectangleBorder(
        borderRadius: BorderRadius.vertical(top: Radius.circular(16)),
      ),
      builder: (ctx) {
        return StatefulBuilder(
          builder: (ctx, setSheetState) {
            void submit() {
              final text = controller.text;
              if (text.isEmpty) return;
              HapticFeedback.mediumImpact();
              _sendText(text);
              if (sendEnter) {
                _sendSpecialKey('Enter');
              }
              _addToHistory(text);
              Navigator.of(ctx).pop();
            }

            void historyUp() {
              if (_composeHistory.isEmpty) return;
              final newIdx = (historyIdx + 1).clamp(0, _composeHistory.length - 1);
              if (newIdx != historyIdx) {
                setSheetState(() {
                  historyIdx = newIdx;
                  controller.text = _composeHistory[historyIdx];
                  controller.selection = TextSelection.collapsed(
                    offset: controller.text.length,
                  );
                });
              }
            }

            void historyDown() {
              if (historyIdx <= 0) {
                setSheetState(() {
                  historyIdx = -1;
                  controller.text = '';
                });
                return;
              }
              setSheetState(() {
                historyIdx--;
                controller.text = _composeHistory[historyIdx];
                controller.selection = TextSelection.collapsed(
                  offset: controller.text.length,
                );
              });
            }

            return Padding(
              padding: EdgeInsets.only(
                left: 16,
                right: 16,
                top: 16,
                bottom: MediaQuery.of(ctx).viewInsets.bottom + 16,
              ),
              child: Column(
                mainAxisSize: MainAxisSize.min,
                children: [
                  // Top row: history nav + enter toggle
                  Row(
                    children: [
                      IconButton(
                        icon: const Icon(Icons.arrow_upward, size: 20),
                        color: _composeHistory.isNotEmpty
                            ? Colors.white70
                            : Colors.white24,
                        onPressed: _composeHistory.isNotEmpty ? historyUp : null,
                        tooltip: 'Previous command',
                        visualDensity: VisualDensity.compact,
                      ),
                      IconButton(
                        icon: const Icon(Icons.arrow_downward, size: 20),
                        color: historyIdx > 0 ? Colors.white70 : Colors.white24,
                        onPressed: historyIdx >= 0 ? historyDown : null,
                        tooltip: 'Next command',
                        visualDensity: VisualDensity.compact,
                      ),
                      const Spacer(),
                      GestureDetector(
                        onTap: () {
                          setSheetState(() => sendEnter = !sendEnter);
                        },
                        child: Container(
                          padding: const EdgeInsets.symmetric(
                            horizontal: 10,
                            vertical: 4,
                          ),
                          decoration: BoxDecoration(
                            color: sendEnter
                                ? Colors.blue.shade700
                                : const Color(0xFF363650),
                            borderRadius: BorderRadius.circular(12),
                          ),
                          child: Row(
                            mainAxisSize: MainAxisSize.min,
                            children: [
                              Icon(
                                Icons.keyboard_return,
                                size: 14,
                                color:
                                    sendEnter ? Colors.white : Colors.white54,
                              ),
                              const SizedBox(width: 4),
                              Text(
                                'Enter',
                                style: TextStyle(
                                  color: sendEnter
                                      ? Colors.white
                                      : Colors.white54,
                                  fontSize: 12,
                                  fontFamily: 'JetBrainsMono',
                                ),
                              ),
                            ],
                          ),
                        ),
                      ),
                    ],
                  ),
                  const SizedBox(height: 8),
                  TextField(
                    controller: controller,
                    autofocus: true,
                    maxLines: null,
                    minLines: 3,
                    style: const TextStyle(
                      color: Colors.white,
                      fontFamily: 'JetBrainsMono',
                      fontSize: 14,
                    ),
                    decoration: InputDecoration(
                      hintText: 'Enter command...',
                      hintStyle: const TextStyle(color: Colors.white38),
                      filled: true,
                      fillColor: const Color(0xFF363650),
                      border: OutlineInputBorder(
                        borderRadius: BorderRadius.circular(8),
                        borderSide: BorderSide.none,
                      ),
                      suffixIcon: IconButton(
                        icon: const Icon(Icons.send, color: Colors.blue),
                        onPressed: submit,
                      ),
                    ),
                  ),
                ],
              ),
            );
          },
        );
      },
    );
  }

  void _sendShiftTab() {
    HapticFeedback.lightImpact();
    // Shift+Tab = reverse tab escape sequence
    _sendText('\x1b[Z');
  }

  void _showMoreKeys() {
    showModalBottomSheet(
      context: context,
      backgroundColor: const Color(0xFF2A2A3E),
      shape: const RoundedRectangleBorder(
        borderRadius: BorderRadius.vertical(top: Radius.circular(16)),
      ),
      builder: (ctx) {
        return Padding(
          padding: const EdgeInsets.fromLTRB(12, 16, 12, 16),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              const Padding(
                padding: EdgeInsets.only(left: 4, bottom: 12),
                child: Text(
                  'More keys',
                  style: TextStyle(
                    color: Colors.white54,
                    fontSize: 13,
                    fontFamily: 'JetBrainsMono',
                  ),
                ),
              ),
              GridView.count(
                shrinkWrap: true,
                crossAxisCount: 4,
                mainAxisSpacing: 6,
                crossAxisSpacing: 6,
                childAspectRatio: 1.8,
                physics: const NeverScrollableScrollPhysics(),
                children: [
                  _buildGridKey(ctx, 'TAB', () => _sendSpecialKey('Tab')),
                  _buildGridKey(ctx, 'ALT', () {
                    _onAltTap();
                    Navigator.of(ctx).pop();
                  }, toggle: _altActive),
                  _buildGridKey(ctx, 'DEL', () => _sendSpecialKey('Delete')),
                  _buildGridKey(ctx, 'HOME', () => _sendSpecialKey('Home')),
                  _buildGridKey(ctx, 'END', () => _sendSpecialKey('End')),
                  _buildGridKey(ctx, 'PG\u2191', () => _sendSpecialKey('PageUp')),
                  _buildGridKey(ctx, 'PG\u2193', () => _sendSpecialKey('PageDown')),
                ],
              ),
              SizedBox(height: MediaQuery.of(ctx).padding.bottom),
            ],
          ),
        );
      },
    );
  }

  Widget _buildGridKey(BuildContext ctx, String label, VoidCallback onTap,
      {bool toggle = false}) {
    return Material(
      color: toggle ? Colors.blue.shade700 : const Color(0xFF363650),
      borderRadius: BorderRadius.circular(8),
      child: InkWell(
        borderRadius: BorderRadius.circular(8),
        onTap: () {
          HapticFeedback.lightImpact();
          onTap();
          if (!toggle) Navigator.of(ctx).pop();
        },
        child: Center(
          child: Text(
            label,
            style: TextStyle(
              color: toggle ? Colors.white : Colors.white70,
              fontSize: 14,
              fontWeight: toggle ? FontWeight.bold : FontWeight.normal,
              fontFamily: 'JetBrainsMono',
            ),
          ),
        ),
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final modifierActive = _ctrlActive || _altActive;
    return Container(
      color: const Color(0xFF2A2A3E),
      child: SafeArea(
        top: false,
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            if (modifierActive)
              Container(height: 2, color: Colors.blue),
            Padding(
              padding: const EdgeInsets.fromLTRB(2, 4, 2, 4),
              child: IntrinsicHeight(
                child: Row(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    // Left: two rows of action keys (~75% width)
                    Expanded(
                      flex: 3,
                      child: Column(
                        mainAxisSize: MainAxisSize.min,
                        children: [
                          Row(children: [
                            _buildIconKey(Icons.edit_note, _showComposeSheet),
                            _buildIconKey(Icons.content_paste, _pasteFromClipboard),
                            _buildKey('ESC', () => _sendSpecialKey('Escape')),
                            _buildKey('ENT', () => _sendSpecialKey('Enter')),
                          ]),
                          Row(children: [
                            _buildToggleKey(
                              'CTRL', _ctrlActive,
                              onTap: _onCtrlTap,
                              onLongPress: _showCtrlGrid,
                            ),
                            _buildKey('TAB', () => _sendSpecialKey('Tab')),
                            _buildKey('S+T', _sendShiftTab),
                            _buildIconKey(Icons.more_horiz, _showMoreKeys),
                          ]),
                        ],
                      ),
                    ),
                    // Divider
                    Container(
                      width: 1,
                      color: Colors.white10,
                      margin: const EdgeInsets.symmetric(horizontal: 3, vertical: 6),
                    ),
                    // Right: arrow d-pad (~25% width)
                    Expanded(
                      flex: 1,
                      child: _buildDpad(),
                    ),
                  ],
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }

  Widget _buildDpad() {
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 2, vertical: 2),
      child: Column(
        mainAxisAlignment: MainAxisAlignment.center,
        children: [
          // Top row: spacer | ↑ | spacer
          Row(
            children: [
              const Expanded(child: SizedBox()),
              Expanded(child: _buildDpadKey('\u2191', () => _handleKey('ArrowUp'))),
              const Expanded(child: SizedBox()),
            ],
          ),
          const SizedBox(height: 2),
          // Bottom row: ← | ↓ | →
          Row(
            children: [
              Expanded(child: _buildDpadKey('\u2190', () => _handleKey('ArrowLeft'))),
              Expanded(child: _buildDpadKey('\u2193', () => _handleKey('ArrowDown'))),
              Expanded(child: _buildDpadKey('\u2192', () => _handleKey('ArrowRight'))),
            ],
          ),
        ],
      ),
    );
  }

  Widget _buildDpadKey(String label, VoidCallback onTap) {
    return Padding(
      padding: const EdgeInsets.all(1),
      child: Material(
        color: const Color(0xFF363650),
        borderRadius: BorderRadius.circular(6),
        child: InkWell(
          borderRadius: BorderRadius.circular(6),
          onTap: () {
            HapticFeedback.lightImpact();
            onTap();
          },
          child: Container(
            height: 30,
            alignment: Alignment.center,
            child: Text(
              label,
              style: const TextStyle(
                color: Colors.white70,
                fontSize: 14,
                fontFamily: 'JetBrainsMono',
              ),
            ),
          ),
        ),
      ),
    );
  }

  Widget _buildKey(String label, VoidCallback onTap) {
    return Expanded(
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 1.5, vertical: 5),
        child: Material(
          color: const Color(0xFF363650),
          borderRadius: BorderRadius.circular(6),
          child: InkWell(
            borderRadius: BorderRadius.circular(6),
            onTap: () {
              HapticFeedback.lightImpact();
              onTap();
            },
            child: Container(
              padding: const EdgeInsets.symmetric(vertical: 8),
              alignment: Alignment.center,
              child: Text(
                label,
                style: const TextStyle(
                  color: Colors.white70,
                  fontSize: 12,
                  fontFamily: 'JetBrainsMono',
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }

  Widget _buildIconKey(IconData icon, VoidCallback onTap) {
    return Expanded(
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 1.5, vertical: 5),
        child: Material(
          color: const Color(0xFF363650),
          borderRadius: BorderRadius.circular(6),
          child: InkWell(
            borderRadius: BorderRadius.circular(6),
            onTap: () {
              HapticFeedback.lightImpact();
              onTap();
            },
            child: Container(
              padding: const EdgeInsets.symmetric(vertical: 8),
              alignment: Alignment.center,
              child: Icon(icon, color: Colors.white70, size: 16),
            ),
          ),
        ),
      ),
    );
  }

  Widget _buildToggleKey(
    String label,
    bool active, {
    required VoidCallback onTap,
    VoidCallback? onLongPress,
  }) {
    return Expanded(
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 1.5, vertical: 5),
        child: Material(
          color: active ? Colors.blue.shade700 : const Color(0xFF363650),
          borderRadius: BorderRadius.circular(6),
          child: InkWell(
            borderRadius: BorderRadius.circular(6),
            onTap: onTap,
            onLongPress: onLongPress,
            child: Container(
              padding: const EdgeInsets.symmetric(vertical: 8),
              alignment: Alignment.center,
              child: Text(
                label,
                style: TextStyle(
                  color: active ? Colors.white : Colors.white70,
                  fontSize: 12,
                  fontWeight: active ? FontWeight.bold : FontWeight.normal,
                  fontFamily: 'JetBrainsMono',
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }
}
