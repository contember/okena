/**
 * KeyToolbar.tsx — the key toolbar pinned above the soft keyboard.
 *
 * Layout (see `PRIMARY_KEYS` / `MORE_ROWS` in `keyInput.ts`):
 *   - one always-visible row of eight equal keys tuned for driving Claude Code:
 *     esc · ^C · tab · shift+tab · up · down · return · more,
 *   - "more" expands a three-row panel above it (same 8-column grid) with
 *     paste/compose, `/ @ | ~ \ \``, ctrl/alt modifiers, ^D ^L ^Z ^R, paging,
 *     home/end, `- _`, left/right, forward delete and the keyboard toggle.
 *     The panel stays open until "more" is tapped again, so repeated keys
 *     (arrows) don't need reopening.
 *   - swiping down on the toolbar hides the soft keyboard.
 *
 * Every key fits the screen width (no horizontal scrolling, nothing clipped)
 * and is 44pt tall. Labels are plain ASCII text or View-drawn icons
 * (`KeyIcons.tsx`), never font-dependent glyphs.
 *
 * Modifiers (ctrl/alt) are shared with {@link TerminalPane} through the
 * {@link KeyModifiers} store: tap for one key, hold to lock. While a modifier
 * is set and the panel is collapsed, the "more" key shows its name.
 *
 * Presentational + injected `native` (defaults to `getOkenaNative()`).
 */

import React, { useCallback, useEffect, useRef, useState } from 'react';
import {
  View,
  Text,
  Pressable,
  Modal,
  KeyboardAvoidingView,
  TextInput,
  StyleSheet,
  Platform,
  Keyboard,
  PanResponder,
} from 'react-native';
import Clipboard from '@react-native-clipboard/clipboard';

import type { OkenaNative } from '../native/okena';
import { getOkenaNative } from '../native/okena';
import { OkenaColors, TerminalTheme } from '../theme';
import {
  KeyModifiers,
  MORE_ROWS,
  PRIMARY_KEYS,
  resolveKeyAction,
  useKeyModifiers,
  type KeyInput,
  type ModifierSnapshot,
  type ModifierState,
  type ToolbarKey,
} from './keyInput';
import { KeyIcon } from './KeyIcons';

// Re-exported for existing importers (WorkspaceScreen, LayoutRenderer, tests).
export {
  KeyModifiers,
  applyModifiersToText,
  useKeyModifiers,
  type ModifierState,
} from './keyInput';

const REPEAT_DELAY_MS = 380;
const REPEAT_INTERVAL_MS = 70;

// ── Props ─────────────────────────────────────────────────────────────────

export interface KeyToolbarProps {
  connId: string;
  terminalId: string | null;
  /** Shared modifier store (also consulted by {@link TerminalPane}). */
  modifiers: KeyModifiers;
  /** Hide the soft keyboard (WorkspaceScreen wires this to blur the input). */
  onHideKeyboard?: () => void;
  /** Show the soft keyboard (focus the selected pane's input). */
  onShowKeyboard?: () => void;
  /** Called after the toolbar sent input, so the pane can reset its keyboard buffer. */
  onInputSent?: () => void;
  /** Injected native surface (defaults to `getOkenaNative()`). */
  native?: OkenaNative;
}

function useKeyboardVisible(): boolean {
  const [visible, setVisible] = useState(() => Keyboard.isVisible());
  useEffect(() => {
    const show = Platform.OS === 'ios' ? 'keyboardWillShow' : 'keyboardDidShow';
    const hide = Platform.OS === 'ios' ? 'keyboardWillHide' : 'keyboardDidHide';
    const subs = [
      Keyboard.addListener(show, () => setVisible(true)),
      Keyboard.addListener(hide, () => setVisible(false)),
    ];
    return () => subs.forEach((s) => s.remove());
  }, []);
  return visible;
}

function modifierLabel(mod: ModifierSnapshot): string | null {
  const parts: string[] = [];
  if (mod.ctrl !== 'inactive') parts.push('ctrl');
  if (mod.option !== 'inactive' || mod.cmd !== 'inactive') parts.push('alt');
  return parts.length > 0 ? parts.join('+') : null;
}

// ── Component ─────────────────────────────────────────────────────────────────

export const KeyToolbar: React.FC<KeyToolbarProps> = ({
  connId,
  terminalId,
  modifiers,
  onHideKeyboard,
  onShowKeyboard,
  onInputSent,
  native = getOkenaNative(),
}) => {
  const mod = useKeyModifiers(modifiers);
  const keyboardVisible = useKeyboardVisible();
  const [moreOpen, setMoreOpen] = useState(false);
  const [composeOpen, setComposeOpen] = useState(false);
  const [composeInitialText, setComposeInitialText] = useState('');

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
      onInputSent?.();
    },
    [native, connId, terminalId, onInputSent],
  );

  const sendInput = useCallback(
    (input: KeyInput) => {
      if (!terminalId) return;
      if (input.kind === 'special') {
        void native.sendSpecialKey(connId, terminalId, input.key);
        onInputSent?.();
      } else {
        sendText(input.text);
      }
    },
    [native, connId, terminalId, sendText, onInputSent],
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

  // Latest callbacks for the swipe responder, which is created once.
  const hideRef = useRef(onHideKeyboard);
  hideRef.current = onHideKeyboard;
  const keyboardVisibleRef = useRef(keyboardVisible);
  keyboardVisibleRef.current = keyboardVisible;

  const swipe = useRef(
    PanResponder.create({
      onMoveShouldSetPanResponderCapture: (_e, g) =>
        keyboardVisibleRef.current && g.dy > 14 && Math.abs(g.dy) > Math.abs(g.dx) * 1.5,
      onPanResponderGrant: () => hideRef.current?.(),
    }),
  ).current;

  const onKey = useCallback(
    (key: ToolbarKey, longPress = false) => {
      const { action } = key;
      switch (action.type) {
        case 'modifier': {
          const which = action.modifier;
          if (longPress) modifiers.lock(which);
          else modifiers.toggle(which);
          return;
        }
        case 'paste':
          pasteClipboard();
          return;
        case 'compose':
          setComposeInitialText('');
          setComposeOpen(true);
          return;
        case 'keyboard':
          if (keyboardVisible) onHideKeyboard?.();
          else onShowKeyboard?.();
          return;
        case 'more':
          setMoreOpen((open) => !open);
          return;
        default: {
          const input = resolveKeyAction(action, modifiers);
          if (input) sendInput(input);
          modifiers.reset();
        }
      }
    },
    [modifiers, pasteClipboard, keyboardVisible, onHideKeyboard, onShowKeyboard, sendInput],
  );

  const renderRow = (keys: readonly ToolbarKey[], rowKey: string) => (
    <View key={rowKey} style={styles.row}>
      {keys.map((key) => {
        let state: ModifierState = 'inactive';
        let label = key.label;
        if (key.action.type === 'modifier') {
          state = mod[key.action.modifier];
        } else if (key.action.type === 'more') {
          const active = modifierLabel(mod);
          if (moreOpen) state = 'active';
          else if (active) label = active;
        } else if (key.action.type === 'keyboard' && keyboardVisible) {
          state = 'active';
        }
        return (
          <KeyCap
            key={key.id}
            def={key}
            label={label}
            state={state}
            onKey={onKey}
          />
        );
      })}
    </View>
  );

  return (
    <View style={styles.root} {...swipe.panHandlers}>
      {moreOpen ? (
        <View style={styles.panel}>
          {MORE_ROWS.map((row, i) => renderRow(row, `more-${i}`))}
        </View>
      ) : null}
      {renderRow(PRIMARY_KEYS, 'primary')}

      <ComposeSheet
        visible={composeOpen}
        initialText={composeInitialText}
        onClose={() => {
          setComposeOpen(false);
          setComposeInitialText('');
        }}
        onSubmit={(text, sendEnter) => {
          sendText(text);
          if (sendEnter) sendInput({ kind: 'special', key: 'Enter' });
        }}
      />
    </View>
  );
};

// ── Key cap ───────────────────────────────────────────────────────────────

const KeyCap: React.FC<{
  def: ToolbarKey;
  /** Overrides `def.label` (the "more" key shows active modifiers). */
  label?: string;
  state: ModifierState;
  onKey: (key: ToolbarKey, longPress?: boolean) => void;
}> = ({ def, label, state, onKey }) => {
  const repeatDelay = useRef<ReturnType<typeof setTimeout> | null>(null);
  const repeatTimer = useRef<ReturnType<typeof setInterval> | null>(null);
  const handledLongPress = useRef(false);

  const stopRepeat = useCallback(() => {
    if (repeatDelay.current) clearTimeout(repeatDelay.current);
    if (repeatTimer.current) clearInterval(repeatTimer.current);
    repeatDelay.current = null;
    repeatTimer.current = null;
  }, []);
  useEffect(() => stopRepeat, [stopRepeat]);

  const isModifier = def.action.type === 'modifier';
  const active = state !== 'inactive';
  const locked = state === 'locked';
  const color = locked ? '#ffffff' : active || def.emphasis ? OkenaColors.accent : OkenaColors.keyText;
  const isSymbol = label !== undefined && label.length === 1;
  const showModifierText = def.action.type === 'more' && label !== undefined;

  const pressHandlers = def.repeat
    ? {
        onPressIn: () => {
          stopRepeat();
          onKey(def);
          repeatDelay.current = setTimeout(() => {
            repeatTimer.current = setInterval(() => onKey(def), REPEAT_INTERVAL_MS);
          }, REPEAT_DELAY_MS);
        },
        onPressOut: stopRepeat,
      }
    : isModifier
      ? {
          onPressIn: () => {
            handledLongPress.current = false;
          },
          onLongPress: () => {
            handledLongPress.current = true;
            onKey(def, true);
          },
          onPress: () => {
            if (!handledLongPress.current) onKey(def);
          },
          delayLongPress: 350,
        }
      : { onPress: () => onKey(def) };

  return (
    <Pressable
      {...pressHandlers}
      style={({ pressed }) => [
        styles.key,
        def.emphasis && styles.keyEmphasis,
        active && styles.keyActive,
        locked && styles.keyLocked,
        pressed && !locked && styles.keyPressed,
      ]}
      accessibilityRole="button"
      accessibilityLabel={def.accessibilityLabel}
      accessibilityHint={isModifier ? 'Tap for the next key, hold to lock' : undefined}
      accessibilityState={isModifier || def.action.type === 'more' ? { selected: active } : undefined}
    >
      {def.icon && !showModifierText ? (
        <KeyIcon name={def.icon} color={color} />
      ) : (
        <View style={styles.labelBox}>
          {def.caption ? (
            <Text style={[styles.caption, { color: active ? color : OkenaColors.keyTextMuted }]}>
              {def.caption}
            </Text>
          ) : null}
          <Text
            style={[
              isSymbol ? styles.symbolText : styles.wordText,
              { color: showModifierText ? OkenaColors.accent : color },
            ]}
            numberOfLines={1}
            adjustsFontSizeToFit
            minimumFontScale={0.75}
          >
            {label}
          </Text>
        </View>
      )}
      {isModifier ? (
        <View style={[styles.lockBar, locked && styles.lockBarOn]} />
      ) : null}
    </Pressable>
  );
};

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
      <KeyboardAvoidingView
        style={styles.composeContainer}
        behavior="padding"
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
                pressed && styles.pressedDim,
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
            keyboardAppearance="dark"
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
              <Text style={styles.composeSendText}>Run</Text>
            </Pressable>
          </View>
        </View>
      </KeyboardAvoidingView>
    </Modal>
  );
};

// ── Styles ─────────────────────────────────────────────────────────────────

const KEY_GAP = 5;

const styles = StyleSheet.create({
  root: {
    paddingHorizontal: 4,
    paddingTop: 6,
    paddingBottom: 6,
    backgroundColor: OkenaColors.surface,
    borderTopWidth: StyleSheet.hairlineWidth,
    borderTopColor: OkenaColors.borderLight,
  },
  panel: {
    paddingBottom: KEY_GAP,
    marginBottom: KEY_GAP,
    borderBottomWidth: StyleSheet.hairlineWidth,
    borderBottomColor: OkenaColors.border,
    gap: KEY_GAP,
  },
  row: { flexDirection: 'row', gap: KEY_GAP },
  key: {
    flex: 1,
    minWidth: 0,
    height: 44,
    borderRadius: 8,
    backgroundColor: OkenaColors.keyBg,
    alignItems: 'center',
    justifyContent: 'center',
    // Subtle bottom edge, like the iOS keyboard keys.
    borderBottomWidth: 1,
    borderBottomColor: '#00000099',
  },
  keyPressed: { backgroundColor: OkenaColors.keyBgPressed },
  keyEmphasis: { backgroundColor: OkenaColors.accentSoft },
  keyActive: {
    backgroundColor: OkenaColors.accentSoft,
    borderWidth: 1,
    borderColor: OkenaColors.accent,
  },
  keyLocked: { backgroundColor: OkenaColors.accent, borderColor: OkenaColors.accent },
  labelBox: { alignItems: 'center', justifyContent: 'center', paddingHorizontal: 2 },
  caption: {
    fontSize: 9,
    fontWeight: '600',
    letterSpacing: 0.3,
    marginBottom: -1,
  },
  wordText: { fontSize: 14, fontWeight: '600', letterSpacing: 0.2 },
  symbolText: {
    fontFamily: TerminalTheme.fontFamily,
    fontSize: 18,
  },
  lockBar: {
    position: 'absolute',
    bottom: 5,
    width: 14,
    height: 2,
    borderRadius: 1,
    backgroundColor: 'transparent',
  },
  lockBarOn: { backgroundColor: '#ffffff' },
  pressedDim: { opacity: 0.62 },
  sheetHandle: {
    alignSelf: 'center',
    width: 36,
    height: 4,
    marginBottom: 14,
    borderRadius: 2,
    backgroundColor: OkenaColors.borderLight,
  },
  // Compose sheet
  composeContainer: { flex: 1 },
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
    fontFamily: TerminalTheme.fontFamily,
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
    minHeight: 44,
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
