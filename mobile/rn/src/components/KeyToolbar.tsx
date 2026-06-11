/**
 * KeyToolbar.tsx — the key toolbar pinned above the soft keyboard.
 *
 * Port of `mobile/lib/src/widgets/key_toolbar.dart`:
 *   - ESC / TAB one-shot keys,
 *   - CTRL / ALT (option) / CMD sticky three-state toggles
 *     (inactive → active one-shot → locked → inactive),
 *   - a handful of punctuation keys (`~ | / -`),
 *   - an arrow joystick (pan to fire arrows; tap fires from offset-from-center),
 *   - compose-sheet + paste + hide-keyboard icon buttons.
 *
 * Modifier semantics (the heart of the port):
 *   - CTRL + a-z/A-Z → the control character (a→0x01 … z→0x1A). Other chars
 *     pass through verbatim. After the next key the one-shot (active) modifiers
 *     reset; locked ones persist.
 *   - OPTION/CMD + char → ESC-prefixed (`\x1b` + char), the xterm meta encoding.
 *   - Arrows with modifiers → `\x1b[1;<mod><A-D>` (mod = 1 + 4·ctrl + 2·option);
 *     CMD-only arrows map to Home/End/PageUp/PageDown.
 *
 * The {@link KeyModifiers} store is SHARED with {@link TerminalPane} (the soft
 * keyboard input also consults it), exactly like the Dart `KeyModifiers`
 * `ChangeNotifier` is shared between `KeyToolbar` and `TerminalView`. It is a
 * tiny external store exposing a React hook ({@link useKeyModifiers}).
 *
 * Presentational + injected `native` (defaults to `getOkenaNative()`), mirroring
 * `TerminalView`'s prop pattern.
 */

import React, {
  useCallback,
  useEffect,
  useRef,
  useState,
  useSyncExternalStore,
} from 'react';
import {
  View,
  Text,
  Pressable,
  ScrollView,
  Modal,
  TextInput,
  StyleSheet,
  type GestureResponderEvent,
} from 'react-native';

import type { OkenaNative, SpecialKey } from '../native/okena';
import { getOkenaNative } from '../native/okena';
import { OkenaColors } from '../theme';

// ── Shared modifier state ──────────────────────────────────────────────────

/** Three-state modifier cycle: inactive → active (one-shot) → locked (sticky). */
export type ModifierState = 'inactive' | 'active' | 'locked';

interface ModifierSnapshot {
  ctrl: ModifierState;
  option: ModifierState;
  cmd: ModifierState;
}

const INITIAL_SNAPSHOT: ModifierSnapshot = {
  ctrl: 'inactive',
  option: 'inactive',
  cmd: 'inactive',
};

function nextState(s: ModifierState): ModifierState {
  switch (s) {
    case 'inactive':
      return 'active';
    case 'active':
      return 'locked';
    case 'locked':
      return 'inactive';
  }
}

/**
 * Shared modifier store between {@link KeyToolbar} and {@link TerminalPane}.
 *
 * Ports the Dart `KeyModifiers extends ChangeNotifier`. It is a minimal external
 * store (subscribe + getSnapshot) so multiple components can subscribe via
 * {@link useKeyModifiers} and stay in sync. A single immutable snapshot object is
 * swapped on every change so `useSyncExternalStore` re-renders subscribers.
 */
export class KeyModifiers {
  private snapshot: ModifierSnapshot = INITIAL_SNAPSHOT;
  private readonly listeners = new Set<() => void>();

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };

  /** Returns the current immutable snapshot (stable identity until a change). */
  getSnapshot = (): ModifierSnapshot => this.snapshot;

  private emit(next: ModifierSnapshot): void {
    this.snapshot = next;
    for (const l of this.listeners) l();
  }

  get ctrl(): boolean {
    return this.snapshot.ctrl !== 'inactive';
  }
  get option(): boolean {
    return this.snapshot.option !== 'inactive';
  }
  get cmd(): boolean {
    return this.snapshot.cmd !== 'inactive';
  }
  get hasAny(): boolean {
    return this.ctrl || this.option || this.cmd;
  }

  toggleCtrl(): void {
    this.emit({ ...this.snapshot, ctrl: nextState(this.snapshot.ctrl) });
  }
  toggleOption(): void {
    this.emit({ ...this.snapshot, option: nextState(this.snapshot.option) });
  }
  toggleCmd(): void {
    this.emit({ ...this.snapshot, cmd: nextState(this.snapshot.cmd) });
  }

  /** Reset only one-shot (active) modifiers; locked ones persist. */
  reset(): void {
    const s = this.snapshot;
    const changed =
      s.ctrl === 'active' || s.option === 'active' || s.cmd === 'active';
    if (!changed) return;
    this.emit({
      ctrl: s.ctrl === 'active' ? 'inactive' : s.ctrl,
      option: s.option === 'active' ? 'inactive' : s.option,
      cmd: s.cmd === 'active' ? 'inactive' : s.cmd,
    });
  }
}

/** Subscribe to a {@link KeyModifiers} store and re-render on changes. */
export function useKeyModifiers(mod: KeyModifiers): ModifierSnapshot {
  return useSyncExternalStore(mod.subscribe, mod.getSnapshot, mod.getSnapshot);
}

// ── Control-char / meta encoding (shared with TerminalPane) ─────────────────

/**
 * Apply the active modifiers to a run of characters, returning the bytes to
 * send. Mirrors the Dart `_applyModifiers` in terminal_view.dart (used for soft
 * keyboard input) — CTRL maps a-z/A-Z to control chars and drops other chars;
 * OPTION/CMD ESC-prefixes each char.
 *
 * Does NOT reset the modifiers (the caller does, after sending).
 */
export function applyModifiersToText(mod: KeyModifiers, chars: string): string {
  if (!mod.hasAny) return chars;
  let out = '';
  for (const ch of chars) {
    const code = ch.charCodeAt(0);
    if (mod.ctrl) {
      if (code >= 0x61 && code <= 0x7a) {
        out += String.fromCharCode(code - 0x60);
      } else if (code >= 0x41 && code <= 0x5a) {
        out += String.fromCharCode(code - 0x40);
      }
      // other chars are dropped under CTRL (matches Dart)
    } else if (mod.option || mod.cmd) {
      out += '\x1b' + ch;
    }
  }
  return out;
}

// ── Arrow encoding ───────────────────────────────────────────────────────────

type ArrowKey = 'ArrowUp' | 'ArrowDown' | 'ArrowLeft' | 'ArrowRight';

const ARROW_CHAR: Record<ArrowKey, string> = {
  ArrowUp: 'A',
  ArrowDown: 'B',
  ArrowRight: 'C',
  ArrowLeft: 'D',
};

// ── Props ─────────────────────────────────────────────────────────────────

export interface KeyToolbarProps {
  connId: string;
  terminalId: string | null;
  /** Shared modifier store (also consulted by {@link TerminalPane}). */
  modifiers: KeyModifiers;
  /** Hide the soft keyboard (WorkspaceScreen wires this to blur the input). */
  onHideKeyboard?: () => void;
  /** Injected native surface (defaults to `getOkenaNative()`). */
  native?: OkenaNative;
}

// ── Component ─────────────────────────────────────────────────────────────────

export const KeyToolbar: React.FC<KeyToolbarProps> = ({
  connId,
  terminalId,
  modifiers,
  onHideKeyboard,
  native = getOkenaNative(),
}) => {
  const mod = useKeyModifiers(modifiers);
  const [composeOpen, setComposeOpen] = useState(false);

  const sendSpecialKey = useCallback(
    (key: SpecialKey) => {
      if (!terminalId) return;
      void native.sendSpecialKey(connId, terminalId, key);
    },
    [native, connId, terminalId],
  );

  const sendText = useCallback(
    (text: string) => {
      if (!terminalId || text.length === 0) return;
      void native.sendText(connId, terminalId, text);
    },
    [native, connId, terminalId],
  );

  /** Send a character key, applying any active modifiers (Dart `_sendCharKey`). */
  const sendCharKey = useCallback(
    (char: string) => {
      if (modifiers.hasAny) {
        if (modifiers.ctrl) {
          const code = char.charCodeAt(0);
          if (code >= 0x61 && code <= 0x7a) {
            sendText(String.fromCharCode(code - 0x60));
          } else if (code >= 0x41 && code <= 0x5a) {
            sendText(String.fromCharCode(code - 0x40));
          } else {
            sendText(char);
          }
        } else {
          // Option/Cmd: ESC prefix.
          sendText('\x1b' + char);
        }
        modifiers.reset();
      } else {
        sendText(char);
      }
    },
    [modifiers, sendText],
  );

  /** Handle arrow from joystick, respecting modifier state (Dart `_handleArrow`). */
  const handleArrow = useCallback(
    (key: ArrowKey) => {
      const arrow = ARROW_CHAR[key];
      if (modifiers.hasAny) {
        if (modifiers.cmd && !modifiers.ctrl && !modifiers.option) {
          switch (key) {
            case 'ArrowLeft':
              sendSpecialKey('Home');
              break;
            case 'ArrowRight':
              sendSpecialKey('End');
              break;
            case 'ArrowUp':
              sendSpecialKey('PageUp');
              break;
            case 'ArrowDown':
              sendSpecialKey('PageDown');
              break;
          }
        } else {
          let m = 1;
          if (modifiers.ctrl) m += 4;
          if (modifiers.option) m += 2;
          sendText(`\x1b[1;${m}${arrow}`);
        }
        modifiers.reset();
      } else {
        sendSpecialKey(key);
        if (modifiers.hasAny) modifiers.reset();
      }
    },
    [modifiers, sendSpecialKey, sendText],
  );

  return (
    <View style={styles.root}>
      <ScrollView
        horizontal
        showsHorizontalScrollIndicator={false}
        keyboardShouldPersistTaps="always"
        contentContainerStyle={styles.scrollContent}
        style={styles.scroll}
      >
        <KeyButton label="esc" onPress={() => sendSpecialKey('Escape')} />
        <ToggleKey label={'⌃'} state={mod.ctrl} onPress={() => modifiers.toggleCtrl()} />
        <ToggleKey label={'⌥'} state={mod.option} onPress={() => modifiers.toggleOption()} />
        <ToggleKey label={'⌘'} state={mod.cmd} onPress={() => modifiers.toggleCmd()} />
        <KeyButton label="tab" onPress={() => sendSpecialKey('Tab')} />
        <View style={styles.gap} />
        <KeyButton label="~" onPress={() => sendCharKey('~')} />
        <KeyButton label="|" onPress={() => sendCharKey('|')} />
        <KeyButton label="/" onPress={() => sendCharKey('/')} />
        <KeyButton label="-" onPress={() => sendCharKey('-')} />
        <View style={styles.gap} />
        <KeyButton label={'✎'} onPress={() => setComposeOpen(true)} />
        <KeyButton label={'⌄'} onPress={() => onHideKeyboard?.()} />
      </ScrollView>
      <View style={styles.arrowSlot}>
        <ArrowJoystick onArrow={handleArrow} />
      </View>

      <ComposeSheet
        visible={composeOpen}
        onClose={() => setComposeOpen(false)}
        onSubmit={(text, sendEnter) => {
          sendText(text);
          if (sendEnter) sendSpecialKey('Enter');
        }}
      />
    </View>
  );
};

// ── Key widgets ─────────────────────────────────────────────────────────────

const KeyButton: React.FC<{ label: string; onPress: () => void }> = ({
  label,
  onPress,
}) => (
  <Pressable style={styles.key} onPress={onPress}>
    <Text style={styles.keyText}>{label}</Text>
  </Pressable>
);

const ToggleKey: React.FC<{
  label: string;
  state: ModifierState;
  onPress: () => void;
}> = ({ label, state, onPress }) => {
  const active = state !== 'inactive';
  const locked = state === 'locked';
  return (
    <Pressable
      style={[styles.key, styles.toggleKey, active && styles.toggleKeyActive]}
      onPress={onPress}
    >
      <Text style={[styles.keyText, active && styles.toggleKeyTextActive]}>{label}</Text>
      <View style={[styles.lockBar, locked && styles.lockBarActive]} />
    </Pressable>
  );
};

// ── Arrow joystick ─────────────────────────────────────────────────────────

const JOYSTICK_SIZE = 52;
const DRAG_THRESHOLD = 14;

const ArrowJoystick: React.FC<{ onArrow: (key: ArrowKey) => void }> = ({
  onArrow,
}) => {
  const originRef = useRef<{ x: number; y: number }>({ x: 0, y: 0 });
  const movedRef = useRef(false);
  const [active, setActive] = useState<ArrowKey | null>(null);
  const clearTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(
    () => () => {
      if (clearTimer.current) clearTimeout(clearTimer.current);
    },
    [],
  );

  const dirFromDelta = (dx: number, dy: number): ArrowKey =>
    Math.abs(dx) > Math.abs(dy)
      ? dx > 0
        ? 'ArrowRight'
        : 'ArrowLeft'
      : dy > 0
        ? 'ArrowDown'
        : 'ArrowUp';

  const fire = useCallback(
    (dir: ArrowKey) => {
      onArrow(dir);
      setActive(dir);
    },
    [onArrow],
  );

  const onResponderGrant = (e: GestureResponderEvent) => {
    originRef.current = {
      x: e.nativeEvent.locationX,
      y: e.nativeEvent.locationY,
    };
    movedRef.current = false;
  };

  const onResponderMove = (e: GestureResponderEvent) => {
    const dx = e.nativeEvent.locationX - originRef.current.x;
    const dy = e.nativeEvent.locationY - originRef.current.y;
    if (Math.hypot(dx, dy) >= DRAG_THRESHOLD) {
      movedRef.current = true;
      fire(dirFromDelta(dx, dy));
      originRef.current = {
        x: e.nativeEvent.locationX,
        y: e.nativeEvent.locationY,
      };
    }
  };

  const onResponderRelease = () => {
    if (!movedRef.current) {
      const cx = JOYSTICK_SIZE / 2;
      const cy = JOYSTICK_SIZE / 2;
      const dx = originRef.current.x - cx;
      const dy = originRef.current.y - cy;
      if (Math.hypot(dx, dy) >= 4) {
        fire(dirFromDelta(dx, dy));
        if (clearTimer.current) clearTimeout(clearTimer.current);
        clearTimer.current = setTimeout(() => setActive(null), 120);
        return;
      }
    }
    setActive(null);
  };

  return (
    <View
      style={styles.joystick}
      onStartShouldSetResponder={() => true}
      onMoveShouldSetResponder={() => true}
      onResponderGrant={onResponderGrant}
      onResponderMove={onResponderMove}
      onResponderRelease={onResponderRelease}
      onResponderTerminate={onResponderRelease}
    >
      <View style={styles.joystickGrid}>
        <Text style={[styles.arrowGlyph, active === 'ArrowUp' && styles.arrowGlyphActive]}>
          {'▲'}
        </Text>
        <View style={styles.arrowRow}>
          <Text style={[styles.arrowGlyph, active === 'ArrowLeft' && styles.arrowGlyphActive]}>
            {'◀'}
          </Text>
          <Text style={[styles.arrowGlyph, active === 'ArrowRight' && styles.arrowGlyphActive]}>
            {'▶'}
          </Text>
        </View>
        <Text style={[styles.arrowGlyph, active === 'ArrowDown' && styles.arrowGlyphActive]}>
          {'▼'}
        </Text>
      </View>
    </View>
  );
};

// ── Compose sheet ─────────────────────────────────────────────────────────

const ComposeSheet: React.FC<{
  visible: boolean;
  onClose: () => void;
  onSubmit: (text: string, sendEnter: boolean) => void;
}> = ({ visible, onClose, onSubmit }) => {
  const [text, setText] = useState('');
  const [sendEnter, setSendEnter] = useState(true);

  useEffect(() => {
    if (visible) setText('');
  }, [visible]);

  const submit = () => {
    if (text.length === 0) {
      onClose();
      return;
    }
    onSubmit(text, sendEnter);
    onClose();
  };

  return (
    <Modal
      visible={visible}
      transparent
      animationType="slide"
      onRequestClose={onClose}
    >
      <Pressable style={styles.composeBackdrop} onPress={onClose} />
      <View style={styles.composeSheet}>
        <View style={styles.composeHeader}>
          <Pressable
            style={[styles.enterToggle, sendEnter && styles.enterToggleActive]}
            onPress={() => setSendEnter((v) => !v)}
          >
            <Text
              style={[
                styles.enterToggleText,
                sendEnter && styles.enterToggleTextActive,
              ]}
            >
              {'⏎'} Enter
            </Text>
          </Pressable>
        </View>
        <TextInput
          style={styles.composeInput}
          value={text}
          onChangeText={setText}
          autoFocus
          multiline
          placeholder="Enter command..."
          placeholderTextColor={OkenaColors.textTertiary}
        />
        <View style={styles.composeActions}>
          <Pressable style={styles.composeBtn} onPress={onClose}>
            <Text style={styles.composeBtnText}>Cancel</Text>
          </Pressable>
          <Pressable style={[styles.composeBtn, styles.composeSend]} onPress={submit}>
            <Text style={styles.composeSendText}>Send</Text>
          </Pressable>
        </View>
      </View>
    </Modal>
  );
};

// ── Styles ─────────────────────────────────────────────────────────────────

const styles = StyleSheet.create({
  root: {
    flexDirection: 'row',
    alignItems: 'center',
    paddingHorizontal: 6,
    paddingVertical: 5,
    backgroundColor: OkenaColors.glassBg,
    borderTopWidth: StyleSheet.hairlineWidth,
    borderTopColor: OkenaColors.glassStroke,
  },
  scroll: { flex: 1 },
  scrollContent: { alignItems: 'center' },
  gap: { width: 12 },
  key: {
    minWidth: 40,
    paddingHorizontal: 8,
    paddingVertical: 9,
    marginHorizontal: 2,
    borderRadius: 10,
    backgroundColor: OkenaColors.keyBg,
    borderWidth: StyleSheet.hairlineWidth,
    borderColor: OkenaColors.keyBorder,
    alignItems: 'center',
    justifyContent: 'center',
  },
  keyText: {
    color: OkenaColors.keyText,
    fontSize: 13,
    fontWeight: '500',
  },
  toggleKey: { paddingVertical: 7 },
  toggleKeyActive: {
    backgroundColor: OkenaColors.accent,
    borderColor: OkenaColors.accent,
  },
  toggleKeyTextActive: { color: '#ffffff', fontWeight: '700', fontSize: 16 },
  lockBar: {
    width: 12,
    height: 2,
    marginTop: 1,
    borderRadius: 1,
    backgroundColor: 'transparent',
  },
  lockBarActive: { backgroundColor: '#ffffff' },
  arrowSlot: { marginLeft: 6 },
  joystick: {
    width: JOYSTICK_SIZE,
    height: JOYSTICK_SIZE,
    borderRadius: 16,
    backgroundColor: OkenaColors.keyBg,
    borderWidth: StyleSheet.hairlineWidth,
    borderColor: OkenaColors.keyBorder,
    alignItems: 'center',
    justifyContent: 'center',
  },
  joystickGrid: { alignItems: 'center', justifyContent: 'center' },
  arrowRow: { flexDirection: 'row', alignItems: 'center' },
  arrowGlyph: {
    color: 'rgba(255,255,255,0.38)',
    fontSize: 9,
    marginHorizontal: 6,
    marginVertical: 1,
  },
  arrowGlyphActive: { color: OkenaColors.accent },
  // Compose sheet
  composeBackdrop: { flex: 1, backgroundColor: 'rgba(0,0,0,0.4)' },
  composeSheet: {
    backgroundColor: OkenaColors.surface,
    borderTopLeftRadius: 16,
    borderTopRightRadius: 16,
    padding: 16,
  },
  composeHeader: {
    flexDirection: 'row',
    justifyContent: 'flex-end',
    marginBottom: 8,
  },
  enterToggle: {
    paddingHorizontal: 10,
    paddingVertical: 4,
    borderRadius: 12,
    backgroundColor: OkenaColors.surfaceElevated,
  },
  enterToggleActive: { backgroundColor: OkenaColors.accent },
  enterToggleText: {
    color: OkenaColors.textTertiary,
    fontSize: 12,
    fontFamily: 'JetBrainsMono',
  },
  enterToggleTextActive: { color: '#ffffff' },
  composeInput: {
    minHeight: 96,
    color: OkenaColors.textPrimary,
    fontFamily: 'JetBrainsMono',
    fontSize: 14,
    backgroundColor: OkenaColors.surfaceElevated,
    borderRadius: 8,
    padding: 12,
    textAlignVertical: 'top',
  },
  composeActions: {
    flexDirection: 'row',
    justifyContent: 'flex-end',
    marginTop: 12,
  },
  composeBtn: {
    paddingHorizontal: 16,
    paddingVertical: 8,
    borderRadius: 8,
    marginLeft: 8,
  },
  composeBtnText: { color: OkenaColors.textSecondary, fontSize: 14 },
  composeSend: { backgroundColor: OkenaColors.accent },
  composeSendText: { color: '#ffffff', fontSize: 14, fontWeight: '600' },
});

export default KeyToolbar;
