/**
 * KeyToolbar.tsx — the key toolbar pinned above the soft keyboard.
 *
 * Port of `mobile/lib/src/widgets/key_toolbar.dart`:
 *   - ESC / TAB one-shot keys,
 *   - CTRL / ALT / META modifiers (tap for one-shot, hold to lock),
 *   - a handful of punctuation keys (`~ | / -`),
 *   - an arrow pad with hold-to-repeat,
 *   - clipboard paste, a reviewed compose flow, and an extended key deck for
 *     signals, navigation, and raw characters.
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
  Platform,
  type GestureResponderEvent,
} from 'react-native';
import Clipboard from '@react-native-clipboard/clipboard';

import type { OkenaNative, SpecialKey } from '../native/okena';
import { getOkenaNative } from '../native/okena';
import { OkenaColors } from '../theme';

// ── Shared modifier state ──────────────────────────────────────────────────

/** Modifier state: inactive, active for one key, or locked until released. */
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

  /** Tap toggles a one-shot modifier; long-press toggles its locked state. */
  toggleCtrl(): void {
    this.emit({
      ...this.snapshot,
      ctrl: this.snapshot.ctrl === 'inactive' ? 'active' : 'inactive',
    });
  }
  lockCtrl(): void {
    this.emit({
      ...this.snapshot,
      ctrl: this.snapshot.ctrl === 'locked' ? 'inactive' : 'locked',
    });
  }
  toggleOption(): void {
    this.emit({
      ...this.snapshot,
      option: this.snapshot.option === 'inactive' ? 'active' : 'inactive',
    });
  }
  lockOption(): void {
    this.emit({
      ...this.snapshot,
      option: this.snapshot.option === 'locked' ? 'inactive' : 'locked',
    });
  }
  toggleCmd(): void {
    this.emit({
      ...this.snapshot,
      cmd: this.snapshot.cmd === 'inactive' ? 'active' : 'inactive',
    });
  }
  lockCmd(): void {
    this.emit({
      ...this.snapshot,
      cmd: this.snapshot.cmd === 'locked' ? 'inactive' : 'locked',
    });
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
  const [composeInitialText, setComposeInitialText] = useState('');
  const [keyDeckOpen, setKeyDeckOpen] = useState(false);

  const sendSpecialKey = useCallback(
    (key: SpecialKey) => {
      if (!terminalId) return;
      void native.sendSpecialKey(connId, terminalId, key);
      modifiers.reset();
    },
    [native, connId, terminalId, modifiers],
  );

  const sendText = useCallback(
    (text: string) => {
      if (!terminalId || text.length === 0) return;
      try {
        const offset = native.getScrollInfo(connId, terminalId).displayOffset;
        if (offset > 0) native.scroll(connId, terminalId, -offset);
      } catch {
        // Input still works while the local terminal state is catching up.
      }
      void native.sendText(connId, terminalId, text);
    },
    [native, connId, terminalId],
  );

  const pasteClipboard = useCallback(() => {
    void Clipboard.getString()
      .then((text) => {
        if (/\r|\n/.test(text)) {
          setComposeInitialText(text);
          setComposeOpen(true);
        } else {
          sendText(text);
        }
      })
      .catch(() => {
        // Clipboard access can be denied by the OS.
      });
  }, [sendText]);

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
        <KeyButton
          label="esc"
          accessibilityLabel="Escape"
          onPress={() => sendSpecialKey('Escape')}
        />
        <ToggleKey
          label="ctrl"
          state={mod.ctrl}
          onPress={() => modifiers.toggleCtrl()}
          onLongPress={() => modifiers.lockCtrl()}
        />
        <ToggleKey
          label="alt"
          state={mod.option}
          onPress={() => modifiers.toggleOption()}
          onLongPress={() => modifiers.lockOption()}
        />
        <ToggleKey
          label="meta"
          state={mod.cmd}
          onPress={() => modifiers.toggleCmd()}
          onLongPress={() => modifiers.lockCmd()}
        />
        <KeyButton label="tab" onPress={() => sendSpecialKey('Tab')} />
        <KeyButton
          label="^C"
          accessibilityLabel="Control C"
          onPress={() => sendSpecialKey('CtrlC')}
        />
        <View style={styles.gap} />
        <KeyButton label="~" onPress={() => sendCharKey('~')} />
        <KeyButton label="|" onPress={() => sendCharKey('|')} />
        <KeyButton label="/" onPress={() => sendCharKey('/')} />
        <KeyButton label="-" onPress={() => sendCharKey('-')} />
        <View style={styles.gap} />
        <KeyButton
          label="paste"
          accessibilityLabel="Paste clipboard"
          onPress={pasteClipboard}
        />
        <KeyButton
          label="edit"
          accessibilityLabel="Compose input"
          onPress={() => {
            setComposeInitialText('');
            setComposeOpen(true);
          }}
        />
        <KeyButton
          label="more"
          accessibilityLabel="More terminal keys"
          onPress={() => {
            onHideKeyboard?.();
            setKeyDeckOpen(true);
          }}
        />
        <KeyButton
          label={'⌄'}
          accessibilityLabel="Hide keyboard"
          onPress={() => onHideKeyboard?.()}
        />
      </ScrollView>
      <View style={styles.arrowSlot}>
        <ArrowJoystick onArrow={handleArrow} />
      </View>

      <ComposeSheet
        visible={composeOpen}
        initialText={composeInitialText}
        onClose={() => {
          setComposeOpen(false);
          setComposeInitialText('');
        }}
        onSubmit={(text, sendEnter) => {
          sendText(text);
          if (sendEnter) sendSpecialKey('Enter');
        }}
      />
      <KeyDeck
        visible={keyDeckOpen}
        onClose={() => setKeyDeckOpen(false)}
        onSpecialKey={sendSpecialKey}
        onCharacter={sendCharKey}
        onPaste={() => {
          setKeyDeckOpen(false);
          pasteClipboard();
        }}
        onCompose={() => {
          setKeyDeckOpen(false);
          setComposeInitialText('');
          setComposeOpen(true);
        }}
      />
    </View>
  );
};

// ── Key widgets ─────────────────────────────────────────────────────────────

const KeyButton: React.FC<{
  label: string;
  onPress: () => void;
  accessibilityLabel?: string;
}> = ({ label, onPress, accessibilityLabel }) => (
  <Pressable
    style={({ pressed }) => [styles.key, pressed && styles.keyPressed]}
    onPress={onPress}
    accessibilityRole="button"
    accessibilityLabel={accessibilityLabel ?? label}
  >
    <Text style={styles.keyText}>{label}</Text>
  </Pressable>
);

const ToggleKey: React.FC<{
  label: string;
  state: ModifierState;
  onPress: () => void;
  onLongPress: () => void;
}> = ({ label, state, onPress, onLongPress }) => {
  const handledLongPress = useRef(false);
  const active = state !== 'inactive';
  const locked = state === 'locked';
  return (
    <Pressable
      style={({ pressed }) => [
        styles.key,
        styles.toggleKey,
        active && styles.toggleKeyActive,
        locked && styles.toggleKeyLocked,
        pressed && styles.keyPressed,
      ]}
      onPressIn={() => {
        handledLongPress.current = false;
      }}
      onLongPress={() => {
        handledLongPress.current = true;
        onLongPress();
      }}
      onPress={() => {
        if (!handledLongPress.current) onPress();
      }}
      delayLongPress={350}
      accessibilityRole="button"
      accessibilityLabel={`${label} modifier`}
      accessibilityHint="Tap for the next key, hold to lock"
      accessibilityState={{ selected: active }}
    >
      <Text
        style={[
          styles.keyText,
          styles.toggleKeyText,
          active && styles.toggleKeyTextActive,
          locked && styles.toggleKeyTextLocked,
        ]}
      >
        {label}
      </Text>
      <View
        style={[
          styles.modifierState,
          active && styles.modifierStateActive,
          locked && styles.modifierStateLocked,
        ]}
      />
    </Pressable>
  );
};

// ── Arrow joystick ─────────────────────────────────────────────────────────

const JOYSTICK_SIZE = 52;

const ArrowJoystick: React.FC<{ onArrow: (key: ArrowKey) => void }> = ({
  onArrow,
}) => {
  const [active, setActive] = useState<ArrowKey | null>(null);
  const activeRef = useRef<ArrowKey | null>(null);
  const repeatDelay = useRef<ReturnType<typeof setTimeout> | null>(null);
  const repeatTimer = useRef<ReturnType<typeof setInterval> | null>(null);

  const stopRepeat = useCallback(() => {
    if (repeatDelay.current) clearTimeout(repeatDelay.current);
    if (repeatTimer.current) clearInterval(repeatTimer.current);
    repeatDelay.current = null;
    repeatTimer.current = null;
    activeRef.current = null;
    setActive(null);
  }, []);

  useEffect(() => stopRepeat, [stopRepeat]);

  const dirFromDelta = (dx: number, dy: number): ArrowKey =>
    Math.abs(dx) > Math.abs(dy)
      ? dx > 0
        ? 'ArrowRight'
        : 'ArrowLeft'
      : dy > 0
      ? 'ArrowDown'
      : 'ArrowUp';

  const startRepeat = useCallback(
    (dir: ArrowKey) => {
      stopRepeat();
      activeRef.current = dir;
      setActive(dir);
      onArrow(dir);
      repeatDelay.current = setTimeout(() => {
        repeatTimer.current = setInterval(() => {
          if (activeRef.current) onArrow(activeRef.current);
        }, 70);
      }, 360);
    },
    [onArrow, stopRepeat],
  );

  const directionAt = (e: GestureResponderEvent): ArrowKey | null => {
    const dx = e.nativeEvent.locationX - JOYSTICK_SIZE / 2;
    const dy = e.nativeEvent.locationY - JOYSTICK_SIZE / 2;
    if (Math.hypot(dx, dy) < 5) return null;
    return dirFromDelta(dx, dy);
  };

  const onResponderGrant = (e: GestureResponderEvent) => {
    const dir = directionAt(e);
    if (dir) startRepeat(dir);
  };

  const onResponderMove = (e: GestureResponderEvent) => {
    const dir = directionAt(e);
    if (dir && dir !== activeRef.current) startRepeat(dir);
  };

  return (
    <View
      style={styles.joystick}
      onStartShouldSetResponder={() => true}
      onMoveShouldSetResponder={() => true}
      onResponderGrant={onResponderGrant}
      onResponderMove={onResponderMove}
      onResponderRelease={stopRepeat}
      onResponderTerminate={stopRepeat}
      accessibilityRole="adjustable"
      accessibilityLabel="Arrow keys"
      accessibilityHint="Touch a direction and hold to repeat"
    >
      <View style={styles.joystickGrid}>
        <Text
          style={[
            styles.arrowGlyph,
            active === 'ArrowUp' && styles.arrowGlyphActive,
          ]}
        >
          {'▲'}
        </Text>
        <View style={styles.arrowRow}>
          <Text
            style={[
              styles.arrowGlyph,
              active === 'ArrowLeft' && styles.arrowGlyphActive,
            ]}
          >
            {'◀'}
          </Text>
          <Text
            style={[
              styles.arrowGlyph,
              active === 'ArrowRight' && styles.arrowGlyphActive,
            ]}
          >
            {'▶'}
          </Text>
        </View>
        <Text
          style={[
            styles.arrowGlyph,
            active === 'ArrowDown' && styles.arrowGlyphActive,
          ]}
        >
          {'▼'}
        </Text>
      </View>
    </View>
  );
};

// ── Extended key deck ─────────────────────────────────────────────────────

const KeyDeck: React.FC<{
  visible: boolean;
  onClose: () => void;
  onSpecialKey: (key: SpecialKey) => void;
  onCharacter: (character: string) => void;
  onPaste: () => void;
  onCompose: () => void;
}> = ({ visible, onClose, onSpecialKey, onCharacter, onPaste, onCompose }) => (
  <Modal
    visible={visible}
    transparent
    animationType="slide"
    onRequestClose={onClose}
  >
    <Pressable style={styles.deckBackdrop} onPress={onClose} />
    <View style={styles.deckSheet}>
      <View style={styles.sheetHandle} />
      <View style={styles.deckHeader}>
        <View>
          <Text style={styles.deckTitle}>Terminal keys</Text>
          <Text style={styles.deckSubtitle}>
            Signals, navigation and raw input
          </Text>
        </View>
        <Pressable
          style={({ pressed }) => [
            styles.deckClose,
            pressed && styles.keyPressed,
          ]}
          onPress={onClose}
          accessibilityRole="button"
          accessibilityLabel="Close terminal keys"
        >
          <Text style={styles.deckCloseText}>Done</Text>
        </Pressable>
      </View>

      <DeckSection label="Signals">
        <DeckKey
          label="ctrl c"
          code="^C"
          onPress={() => onSpecialKey('CtrlC')}
        />
        <DeckKey
          label="ctrl d"
          code="^D"
          onPress={() => onSpecialKey('CtrlD')}
        />
        <DeckKey
          label="ctrl z"
          code="^Z"
          onPress={() => onSpecialKey('CtrlZ')}
        />
        <DeckKey
          label="escape"
          code="esc"
          onPress={() => onSpecialKey('Escape')}
        />
      </DeckSection>

      <DeckSection label="Navigation">
        <DeckKey
          label="home"
          code="home"
          onPress={() => onSpecialKey('Home')}
        />
        <DeckKey label="end" code="end" onPress={() => onSpecialKey('End')} />
        <DeckKey
          label="page up"
          code="pgup"
          onPress={() => onSpecialKey('PageUp')}
        />
        <DeckKey
          label="page down"
          code="pgdn"
          onPress={() => onSpecialKey('PageDown')}
        />
        <DeckKey
          label="delete"
          code="del"
          onPress={() => onSpecialKey('Delete')}
        />
        <DeckKey
          label="arrow left"
          code="←"
          onPress={() => onSpecialKey('ArrowLeft')}
        />
        <DeckKey
          label="arrow up"
          code="↑"
          onPress={() => onSpecialKey('ArrowUp')}
        />
        <DeckKey
          label="arrow down"
          code="↓"
          onPress={() => onSpecialKey('ArrowDown')}
        />
        <DeckKey
          label="arrow right"
          code="→"
          onPress={() => onSpecialKey('ArrowRight')}
        />
      </DeckSection>

      <DeckSection label="Characters">
        {['~', '|', '\\', '/', '-', '_', '=', ':', ';'].map((character) => (
          <DeckKey
            key={character}
            label={character}
            code={character}
            onPress={() => onCharacter(character)}
            compact
          />
        ))}
      </DeckSection>

      <View style={styles.deckActions}>
        <Pressable
          style={({ pressed }) => [
            styles.deckAction,
            pressed && styles.keyPressed,
          ]}
          onPress={onPaste}
        >
          <Text style={styles.deckActionText}>Paste clipboard</Text>
        </Pressable>
        <Pressable
          style={({ pressed }) => [
            styles.deckAction,
            styles.deckActionPrimary,
            pressed && styles.keyPressed,
          ]}
          onPress={onCompose}
        >
          <Text style={styles.deckActionPrimaryText}>Compose input</Text>
        </Pressable>
      </View>
    </View>
  </Modal>
);

const DeckSection: React.FC<{ label: string; children: React.ReactNode }> = ({
  label,
  children,
}) => (
  <View style={styles.deckSection}>
    <Text style={styles.deckSectionLabel}>{label}</Text>
    <View style={styles.deckKeys}>{children}</View>
  </View>
);

const DeckKey: React.FC<{
  label: string;
  code: string;
  onPress: () => void;
  compact?: boolean;
}> = ({ label, code, onPress, compact = false }) => (
  <Pressable
    style={({ pressed }) => [
      styles.deckKey,
      compact && styles.deckKeyCompact,
      pressed && styles.deckKeyPressed,
    ]}
    onPress={onPress}
    accessibilityRole="button"
    accessibilityLabel={label}
  >
    <Text style={styles.deckKeyText}>{code}</Text>
  </Pressable>
);

// ── Compose sheet ─────────────────────────────────────────────────────────

const ComposeSheet: React.FC<{
  visible: boolean;
  initialText: string;
  onClose: () => void;
  onSubmit: (text: string, sendEnter: boolean) => void;
}> = ({ visible, initialText, onClose, onSubmit }) => {
  const [text, setText] = useState('');

  useEffect(() => {
    if (visible) setText(initialText);
  }, [visible, initialText]);

  const submit = (sendEnter: boolean) => {
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
        <View style={styles.sheetHandle} />
        <View style={styles.composeHeader}>
          <View style={styles.composeHeading}>
            <Text style={styles.composeTitle}>Compose input</Text>
            <Text style={styles.composeSubtitle}>
              Review text before it reaches the terminal
            </Text>
          </View>
          <Pressable
            style={({ pressed }) => [
              styles.composePaste,
              pressed && styles.keyPressed,
            ]}
            onPress={() => {
              void Clipboard.getString()
                .then((clipboardText) =>
                  setText((current) => current + clipboardText),
                )
                .catch(() => {
                  // Clipboard access can be denied by the OS.
                });
            }}
          >
            <Text style={styles.composePasteText}>Paste</Text>
          </Pressable>
        </View>
        <TextInput
          style={styles.composeInput}
          value={text}
          onChangeText={setText}
          autoFocus
          multiline
          autoCapitalize="none"
          autoCorrect={false}
          spellCheck={false}
          autoComplete="off"
          importantForAutofill="no"
          keyboardType={
            Platform.OS === 'android' ? 'visible-password' : 'ascii-capable'
          }
          placeholder="Command, path, or multiline input"
          placeholderTextColor={OkenaColors.textTertiary}
        />
        <View style={styles.composeActions}>
          <Pressable style={styles.composeBtn} onPress={onClose}>
            <Text style={styles.composeBtnText}>Cancel</Text>
          </Pressable>
          <Pressable
            style={[styles.composeBtn, styles.composeInsert]}
            onPress={() => submit(false)}
          >
            <Text style={styles.composeInsertText}>Insert</Text>
          </Pressable>
          <Pressable
            style={[styles.composeBtn, styles.composeSend]}
            onPress={() => submit(true)}
          >
            <Text style={styles.composeSendText}>Run ↵</Text>
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
    paddingVertical: 6,
    backgroundColor: OkenaColors.glassBg,
    borderTopWidth: StyleSheet.hairlineWidth,
    borderTopColor: OkenaColors.glassStroke,
  },
  scroll: { flex: 1 },
  scrollContent: { alignItems: 'center' },
  gap: { width: 8 },
  key: {
    minWidth: 42,
    minHeight: 42,
    paddingHorizontal: 10,
    paddingVertical: 8,
    marginHorizontal: 2,
    borderRadius: 8,
    backgroundColor: OkenaColors.keyBg,
    borderWidth: StyleSheet.hairlineWidth,
    borderColor: OkenaColors.keyBorder,
    alignItems: 'center',
    justifyContent: 'center',
  },
  keyPressed: { opacity: 0.62 },
  keyText: {
    color: OkenaColors.keyText,
    fontFamily: 'JetBrainsMono',
    fontSize: 12,
    fontWeight: '600',
  },
  toggleKey: { minWidth: 50, paddingVertical: 6 },
  toggleKeyActive: {
    backgroundColor: OkenaColors.accentSoft,
    borderColor: OkenaColors.accent,
  },
  toggleKeyLocked: { backgroundColor: OkenaColors.accent },
  toggleKeyText: { fontSize: 11 },
  toggleKeyTextActive: { color: OkenaColors.accent, fontWeight: '700' },
  toggleKeyTextLocked: { color: '#ffffff' },
  modifierState: {
    width: 4,
    height: 2,
    marginTop: 3,
    borderRadius: 1,
    backgroundColor: 'transparent',
  },
  modifierStateActive: { backgroundColor: OkenaColors.accent },
  modifierStateLocked: { width: 18, backgroundColor: '#ffffff' },
  arrowSlot: { marginLeft: 6 },
  joystick: {
    width: JOYSTICK_SIZE,
    height: JOYSTICK_SIZE,
    borderRadius: 12,
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
  sheetHandle: {
    alignSelf: 'center',
    width: 36,
    height: 4,
    marginBottom: 14,
    borderRadius: 2,
    backgroundColor: OkenaColors.borderLight,
  },
  // Extended key deck
  deckBackdrop: { flex: 1, backgroundColor: OkenaColors.backdrop },
  deckSheet: {
    backgroundColor: OkenaColors.surface,
    borderTopLeftRadius: 16,
    borderTopRightRadius: 16,
    borderTopWidth: StyleSheet.hairlineWidth,
    borderColor: OkenaColors.borderLight,
    paddingHorizontal: 16,
    paddingTop: 10,
    paddingBottom: 24,
  },
  deckHeader: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'space-between',
    marginBottom: 14,
  },
  deckTitle: {
    color: OkenaColors.textPrimary,
    fontSize: 18,
    fontWeight: '700',
  },
  deckSubtitle: { color: OkenaColors.textTertiary, fontSize: 12, marginTop: 2 },
  deckClose: {
    minHeight: 40,
    paddingHorizontal: 12,
    alignItems: 'center',
    justifyContent: 'center',
  },
  deckCloseText: { color: OkenaColors.accent, fontSize: 14, fontWeight: '600' },
  deckSection: { marginBottom: 14 },
  deckSectionLabel: {
    color: OkenaColors.textTertiary,
    fontSize: 11,
    fontWeight: '700',
    letterSpacing: 0.8,
    textTransform: 'uppercase',
    marginBottom: 7,
  },
  deckKeys: { flexDirection: 'row', flexWrap: 'wrap', marginHorizontal: -3 },
  deckKey: {
    minWidth: 64,
    minHeight: 44,
    paddingHorizontal: 10,
    margin: 3,
    borderRadius: 8,
    alignItems: 'center',
    justifyContent: 'center',
    backgroundColor: OkenaColors.surfaceElevated,
    borderWidth: StyleSheet.hairlineWidth,
    borderColor: OkenaColors.keyBorder,
  },
  deckKeyCompact: { minWidth: 44, paddingHorizontal: 8 },
  deckKeyPressed: {
    backgroundColor: OkenaColors.accentSoft,
    borderColor: OkenaColors.accent,
  },
  deckKeyText: {
    color: OkenaColors.textPrimary,
    fontFamily: 'JetBrainsMono',
    fontSize: 12,
    fontWeight: '600',
  },
  deckActions: { flexDirection: 'row', marginHorizontal: -4, marginTop: 2 },
  deckAction: {
    flex: 1,
    minHeight: 46,
    marginHorizontal: 4,
    borderRadius: 8,
    alignItems: 'center',
    justifyContent: 'center',
    backgroundColor: OkenaColors.surfaceElevated,
    borderWidth: StyleSheet.hairlineWidth,
    borderColor: OkenaColors.borderLight,
  },
  deckActionPrimary: {
    backgroundColor: OkenaColors.accent,
    borderColor: OkenaColors.accent,
  },
  deckActionText: {
    color: OkenaColors.textSecondary,
    fontSize: 13,
    fontWeight: '600',
  },
  deckActionPrimaryText: { color: '#ffffff', fontSize: 13, fontWeight: '700' },
  // Compose sheet
  composeBackdrop: { flex: 1, backgroundColor: OkenaColors.backdrop },
  composeSheet: {
    backgroundColor: OkenaColors.surface,
    borderTopLeftRadius: 16,
    borderTopRightRadius: 16,
    borderTopWidth: StyleSheet.hairlineWidth,
    borderColor: OkenaColors.borderLight,
    paddingHorizontal: 16,
    paddingTop: 10,
    paddingBottom: 20,
  },
  composeHeader: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'space-between',
    marginBottom: 12,
  },
  composeHeading: { flex: 1, paddingRight: 12 },
  composeTitle: {
    color: OkenaColors.textPrimary,
    fontSize: 18,
    fontWeight: '700',
  },
  composeSubtitle: {
    color: OkenaColors.textTertiary,
    fontSize: 12,
    marginTop: 2,
  },
  composePaste: {
    minHeight: 40,
    paddingHorizontal: 12,
    borderRadius: 8,
    alignItems: 'center',
    justifyContent: 'center',
    backgroundColor: OkenaColors.surfaceElevated,
    borderWidth: StyleSheet.hairlineWidth,
    borderColor: OkenaColors.borderLight,
  },
  composePasteText: {
    color: OkenaColors.accent,
    fontSize: 13,
    fontWeight: '600',
  },
  composeInput: {
    minHeight: 120,
    maxHeight: 240,
    color: OkenaColors.textPrimary,
    fontFamily: 'JetBrainsMono',
    fontSize: 14,
    backgroundColor: OkenaColors.surfaceElevated,
    borderRadius: 8,
    borderWidth: StyleSheet.hairlineWidth,
    borderColor: OkenaColors.borderLight,
    padding: 12,
    textAlignVertical: 'top',
  },
  composeActions: {
    flexDirection: 'row',
    justifyContent: 'flex-end',
    marginTop: 12,
  },
  composeBtn: {
    minHeight: 42,
    paddingHorizontal: 16,
    borderRadius: 8,
    marginLeft: 8,
    alignItems: 'center',
    justifyContent: 'center',
  },
  composeBtnText: { color: OkenaColors.textSecondary, fontSize: 14 },
  composeInsert: {
    backgroundColor: OkenaColors.surfaceElevated,
    borderWidth: StyleSheet.hairlineWidth,
    borderColor: OkenaColors.borderLight,
  },
  composeInsertText: {
    color: OkenaColors.textPrimary,
    fontSize: 14,
    fontWeight: '600',
  },
  composeSend: { backgroundColor: OkenaColors.accent },
  composeSendText: { color: '#ffffff', fontSize: 14, fontWeight: '600' },
});

export default KeyToolbar;
