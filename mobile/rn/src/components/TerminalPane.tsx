/**
 * TerminalPane.tsx — the per-terminal container that drives the Skia renderer.
 *
 * Port of the chrome around the terminal canvas in
 * `mobile/lib/src/widgets/terminal_view.dart`. The existing
 * {@link import('./TerminalView').TerminalView} already owns:
 *   - measuring its own size (`onLayout`) → cols/rows,
 *   - `resizeLocal` immediately + 200ms-debounced `resizeTerminal`,
 *   - the rAF repaint loop gated on `isDirty()`,
 *   - the 3-pass Skia paint (bg / glyphs / cursor / scrollbar) AND the selection
 *     highlight overlay (when its `selecting` prop is true).
 *
 * So this container only adds the INPUT + GESTURE chrome the renderer does not:
 *   - a (near-)invisible full-bleed `TextInput` for the soft keyboard. Each
 *     edit is diffed against the previous text (see `softInput.ts`) into
 *     `Backspace` special keys + `sendText`. Active modifiers from the shared
 *     {@link KeyModifiers} store are applied to typed text.
 *   - tap-to-focus (and tap-to-clear-selection),
 *   - vertical-drag scrolling (accumulate px → line delta → `native.scrollWheel`,
 *     which moves the scrollback or, when the app enabled mouse reporting,
 *     sends wheel events — the desktop split; see `terminalScroll.ts`),
 *   - long-press to start/extend a character selection; release copies the
 *     selected text to the clipboard and clears. Double-tap selects a word.
 *   - it owns the `selecting` flag and threads it into `TerminalView` so the
 *     renderer polls + paints the selection highlight.
 *   - two-finger pinch changes the terminal font size (the persisted setting
 *     from the settings store, shared by every pane); TerminalView re-measures
 *     and resizes the PTY.
 *
 * Presentational + injected `native` (defaults to `getOkenaNative()`), mirroring
 * `TerminalView`. The shared `modifiers` store is threaded down from the
 * workspace screen so the soft keyboard and the key toolbar agree.
 */

import React, {
  forwardRef,
  useCallback,
  useEffect,
  useImperativeHandle,
  useRef,
  useState,
} from 'react';
import {
  View,
  Text,
  TextInput,
  StyleSheet,
  Keyboard,
  Platform,
  type NativeSyntheticEvent,
  type TextInputChangeEventData,
  type GestureResponderEvent,
} from 'react-native';
import Clipboard from '@react-native-clipboard/clipboard';

import type { OkenaNative } from '../native/okena';
import { getOkenaNative } from '../native/okena';
import { OkenaColors, TerminalTheme } from '../theme';
import { TerminalView, type TerminalFonts } from './TerminalView';
import { KeyModifiers, applyModifiersToText } from './keyInput';
import { useSettingsStore } from '../state/settingsStore';
import { accumulateScroll, exceedsTouchSlop, pointToCell } from './terminalScroll';
import {
  INITIAL_SOFT_INPUT,
  applyChange,
  requestReset,
  shouldResetAfter,
  type SoftInputState,
} from './softInput';

/** Imperative handle so the workspace screen can focus/blur the soft keyboard. */
export interface TerminalPaneHandle {
  focus(): void;
  blur(): void;
  /** Start a fresh soft-keyboard buffer (after input sent from elsewhere). */
  resetInput(): void;
}

export interface TerminalPaneProps {
  connId: string;
  terminalId: string;
  /** Loaded JetBrainsMono fonts, threaded down to the renderer. */
  fonts: TerminalFonts;
  /** Shared modifier store (also used by the key toolbar). */
  modifiers: KeyModifiers;
  /**
   * Called with {@link terminalId} when this pane takes keyboard focus or is
   * tapped, so the key toolbar targets the pane the user is typing into.
   */
  onSelect?: (terminalId: string) => void;
  /**
   * Whether this pane is the current toolbar target. When it becomes true while
   * the soft keyboard is up, keyboard focus follows — otherwise a selection the
   * user did not make (a newly-created terminal) would leave typing on the old
   * pane. Focus is never summoned when the keyboard is down.
   */
  selected?: boolean;
  /** Font size in pt. Defaults to the persisted setting (settings store). */
  fontSize?: number;
  /** Injected native surface (defaults to `getOkenaNative()`). */
  native?: OkenaNative;
}

/** Distance between the first two touches, or null for a single touch. */
function pinchDistance(e: GestureResponderEvent): number | null {
  // Synthetic events (tests) may omit `touches`.
  const [a, b] = e.nativeEvent.touches ?? [];
  if (!a || !b) return null;
  return Math.hypot(a.pageX - b.pageX, a.pageY - b.pageY);
}

// Internal mutable grid size, reported by TerminalView via onGridSizeChange.
interface Grid {
  cols: number;
  rows: number;
  cellWidth: number;
  cellHeight: number;
}

export const TerminalPane = forwardRef<TerminalPaneHandle, TerminalPaneProps>(
  (
    {
      connId,
      terminalId,
      fonts,
      modifiers,
      onSelect,
      selected = false,
      fontSize,
      native = getOkenaNative(),
    },
    ref,
  ) => {
    const storedFontSize = useSettingsStore((s) => s.terminalFontSize);
    const setFontSize = useSettingsStore((s) => s.setTerminalFontSize);
    const effectiveFontSize = fontSize ?? storedFontSize;
    // Active pinch: start distance + font size at its start.
    const pinch = useRef<{ distance: number; fontSize: number } | null>(null);
    const inputRef = useRef<TextInput>(null);
    const [selecting, setSelecting] = useState(false);
    const [copied, setCopied] = useState(false);
    const copiedTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

    // What the hidden TextInput holds (sentinel-padded). The field is
    // controlled so resets go through RN's event-counted text update;
    // `setNativeProps({ text })` is silently ignored on the new architecture.
    const softInput = useRef<SoftInputState>(INITIAL_SOFT_INPUT);
    const [inputValue, setInputValue] = useState(INITIAL_SOFT_INPUT.text);

    // Grid geometry — TerminalView measures the glyph cell and computes
    // cols/rows; we mirror it so touch coordinates can be converted to cells
    // and drag distance to lines.
    const grid = useRef<Grid>({
      cols: 80,
      rows: 24,
      cellWidth: 0,
      cellHeight: 0,
    });

    // Vertical-scroll accumulator (px) → whole-line deltas.
    const scrollAccum = useRef(0);
    const dragLastPageY = useRef<number | null>(null);

    const resetInput = useCallback(() => {
      const next = requestReset(softInput.current);
      if (next === null) return;
      softInput.current = next;
      setInputValue(next.text);
    }, []);

    useImperativeHandle(
      ref,
      () => ({
        focus: () => inputRef.current?.focus(),
        blur: () => inputRef.current?.blur(),
        resetInput,
      }),
      [resetInput],
    );

    const onGridSizeChange = useCallback(
      (cols: number, rows: number, cellWidth: number, cellHeight: number) => {
        grid.current = { cols, rows, cellWidth, cellHeight };
      },
      [],
    );

    // ── soft keyboard input ──────────────────────────────────────────────────

    const onInputFocus = useCallback(() => {
      resetInput();
      onSelect?.(terminalId);
    }, [resetInput, onSelect, terminalId]);

    useEffect(() => {
      resetInput();
      return () => {
        if (copiedTimer.current) clearTimeout(copiedTimer.current);
      };
    }, [terminalId, resetInput]);

    useEffect(() => {
      if (selected && Keyboard.isVisible()) inputRef.current?.focus();
    }, [selected]);

    const onChange = useCallback(
      (e: NativeSyntheticEvent<TextInputChangeEventData>) => {
        const { delta, state } = applyChange(softInput.current, e.nativeEvent.text);
        softInput.current = state;
        setInputValue(state.text);

        // `sendText` / `sendSpecialKey` snap the view back to the bottom
        // natively, like every desktop input path.
        for (let i = 0; i < delta.backspaces; i++) {
          void native.sendSpecialKey(connId, terminalId, 'Backspace');
        }
        // Soft-keyboard Return arrives as \n; the terminal wants \r.
        let text = delta.text.replace(/\n/g, '\r');
        if (text.length > 0 && modifiers.hasAny) {
          text = applyModifiersToText(modifiers, text);
          modifiers.reset();
        }
        if (text.length > 0) {
          void native.sendText(connId, terminalId, text);
        }

        if (shouldResetAfter(state, delta)) resetInput();
      },
      [native, connId, terminalId, modifiers, resetInput],
    );

    // ── touch → cell ──────────────────────────────────────────────────────────

    const touchToCell = useCallback(
      (x: number, y: number): { col: number; row: number } =>
        pointToCell(x, y, grid.current),
      [],
    );

    // ── selection ──────────────────────────────────────────────────────────────

    const copySelectionAndClear = useCallback(() => {
      try {
        const selectedText = native.getSelectedText(connId, terminalId);
        if (selectedText !== undefined && selectedText.length > 0) {
          Clipboard.setString(selectedText);
          setCopied(true);
          if (copiedTimer.current) clearTimeout(copiedTimer.current);
          copiedTimer.current = setTimeout(() => setCopied(false), 1200);
        }
      } catch {
        // ignore
      }
      try {
        native.clearSelection(connId, terminalId);
      } catch {
        // ignore
      }
      setSelecting(false);
    }, [native, connId, terminalId]);

    // ── gesture responder (tap / drag-scroll / long-press select) ───────────────
    //
    // Positions: `locationX/Y` is only trusted at grant (where it is relative to
    // this pane). After that, movement is measured with `pageX/Y` deltas — RN
    // reports `location*` relative to whatever view is under the finger, which
    // breaks once a drag leaves the pane (e.g. into the other half of a split).

    const longPressTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
    const grantPos = useRef<{ x: number; y: number }>({ x: 0, y: 0 });
    const grantPage = useRef<{ x: number; y: number }>({ x: 0, y: 0 });
    const lastTap = useRef<{ x: number; y: number; t: number } | null>(null);
    const moved = useRef(false);
    const selectingRef = useRef(false);

    const clearLongPress = useCallback(() => {
      if (longPressTimer.current) {
        clearTimeout(longPressTimer.current);
        longPressTimer.current = null;
      }
    }, []);

    /** Current touch position in pane coordinates (grant point + page delta). */
    const panePoint = useCallback((e: GestureResponderEvent) => {
      const { pageX, pageY } = e.nativeEvent;
      return {
        x: grantPos.current.x + (pageX - grantPage.current.x),
        y: grantPos.current.y + (pageY - grantPage.current.y),
      };
    }, []);

    /** Handles a two-finger move; returns true while a pinch owns the gesture. */
    const handlePinch = useCallback(
      (e: GestureResponderEvent): boolean => {
        const distance = pinchDistance(e);
        if (distance === null || selectingRef.current) return pinch.current !== null;
        if (!pinch.current) {
          clearLongPress();
          moved.current = true;
          pinch.current = { distance: Math.max(distance, 1), fontSize: effectiveFontSize };
          return true;
        }
        setFontSize((pinch.current.fontSize * distance) / pinch.current.distance);
        return true;
      },
      [clearLongPress, effectiveFontSize, setFontSize],
    );

    const onGrant = useCallback(
      (e: GestureResponderEvent) => {
        pinch.current = null;
        const { locationX: x, locationY: y, pageX, pageY } = e.nativeEvent;
        grantPos.current = { x, y };
        grantPage.current = { x: pageX, y: pageY };
        dragLastPageY.current = pageY;
        scrollAccum.current = 0;
        moved.current = false;

        clearLongPress();
        longPressTimer.current = setTimeout(() => {
          // Begin a character selection at the grant cell.
          const { col, row } = touchToCell(
            grantPos.current.x,
            grantPos.current.y,
          );
          try {
            native.startSelection(connId, terminalId, col, row);
          } catch {
            // ignore
          }
          selectingRef.current = true;
          setSelecting(true);
        }, 350);
      },
      [native, connId, terminalId, touchToCell, clearLongPress],
    );

    const onMove = useCallback(
      (e: GestureResponderEvent) => {
        if (handlePinch(e)) return;
        const { x, y } = panePoint(e);

        if (selectingRef.current) {
          // Extend the active selection.
          const { col, row } = touchToCell(x, y);
          try {
            native.updateSelection(connId, terminalId, col, row);
          } catch {
            // ignore
          }
          return;
        }

        if (
          !moved.current &&
          exceedsTouchSlop(x - grantPos.current.x, y - grantPos.current.y)
        ) {
          moved.current = true;
          clearLongPress();
        }
        if (!moved.current) return;

        // Vertical-drag scrolling: accumulate px, emit whole-line deltas.
        const { pageY } = e.nativeEvent;
        if (dragLastPageY.current === null) return;
        const step = accumulateScroll(
          scrollAccum.current,
          pageY - dragLastPageY.current,
          grid.current.cellHeight,
        );
        dragLastPageY.current = pageY;
        scrollAccum.current = step.remainderPx;
        if (step.lines !== 0) {
          const { col, row } = touchToCell(x, y);
          try {
            native.scrollWheel(connId, terminalId, step.lines, col, row);
          } catch {
            // ignore
          }
        }
      },
      [native, connId, terminalId, touchToCell, clearLongPress, panePoint, handlePinch],
    );

    const endGesture = useCallback(() => {
      pinch.current = null;
      clearLongPress();
      dragLastPageY.current = null;
      scrollAccum.current = 0;
    }, [clearLongPress]);

    const onRelease = useCallback(
      () => {
        endGesture();

        if (selectingRef.current) {
          selectingRef.current = false;
          copySelectionAndClear();
          return;
        }

        if (!moved.current) {
          // A tap. If a selection exists, clear it; else (double-tap?) word
          // select, otherwise focus the keyboard.
          const now = Date.now();
          const { x, y } = grantPos.current;
          const prevTap = lastTap.current;
          const isDouble =
            prevTap !== null &&
            now - prevTap.t < 300 &&
            Math.hypot(x - prevTap.x, y - prevTap.y) < 24;

          // Any tap makes this pane the toolbar / keyboard target — in a split
          // this is how the user picks the pane to work in.
          onSelect?.(terminalId);

          if (selecting) {
            try {
              native.clearSelection(connId, terminalId);
            } catch {
              // ignore
            }
            setSelecting(false);
          } else if (isDouble) {
            const { col, row } = touchToCell(x, y);
            try {
              native.startWordSelection(connId, terminalId, col, row);
            } catch {
              // ignore
            }
            selectingRef.current = true;
            setSelecting(true);
            copySelectionAndClear();
          } else {
            // `focus()` on an already-focused input does not re-fire `onFocus`,
            // so the `onSelect` above is what re-syncs the toolbar target.
            inputRef.current?.focus();
          }
          lastTap.current = { x, y, t: now };
        }
      },
      [
        native,
        connId,
        terminalId,
        touchToCell,
        selecting,
        copySelectionAndClear,
        endGesture,
        onSelect,
      ],
    );

    // The system took the touch (e.g. an OS gesture). Never treat that as a
    // tap; finish an in-flight selection the same way a release would.
    const onTerminate = useCallback(() => {
      endGesture();
      if (selectingRef.current) {
        selectingRef.current = false;
        copySelectionAndClear();
      }
    }, [endGesture, copySelectionAndClear]);

    return (
      <View style={styles.root}>
        {/* The Skia renderer. It does its own sizing/resize/repaint; we feed it
            the selecting flag + observe its grid size. */}
        <View style={StyleSheet.absoluteFill}>
          <TerminalView
            native={native}
            connId={connId}
            terminalId={terminalId}
            fonts={fonts}
            fontSize={effectiveFontSize}
            selecting={selecting}
            onGridSizeChange={onGridSizeChange}
          />
        </View>

        {/* Gesture surface — tap to focus, drag to scroll, long-press to select. */}
        <View
          style={StyleSheet.absoluteFill}
          onStartShouldSetResponder={() => true}
          onMoveShouldSetResponder={() => true}
          onResponderGrant={onGrant}
          onResponderMove={onMove}
          // Once a pane owns a touch it keeps it: a scroll or selection drag
          // must not be stolen by an ancestor mid-gesture.
          onResponderTerminationRequest={() => false}
          onResponderRelease={onRelease}
          onResponderTerminate={onTerminate}
        />

        {/* Hidden soft-keyboard input. Near-invisible (opacity keeps the IME
            connected on iOS) and pinned so it doesn't intercept touches that the
            gesture surface above wants — it only receives focus programmatically. */}
        <TextInput
          ref={inputRef}
          style={styles.hiddenInput}
          value={inputValue}
          onChange={onChange}
          autoCapitalize="none"
          autoCorrect={false}
          spellCheck={false}
          autoComplete="off"
          importantForAutofill="no"
          textContentType="none"
          multiline
          caretHidden
          contextMenuHidden
          keyboardType={
            Platform.OS === 'android' ? 'visible-password' : 'ascii-capable'
          }
          disableFullscreenUI
          onFocus={onInputFocus}
          // Keep it from being read out / styled visibly.
          underlineColorAndroid="transparent"
        />
        {copied ? (
          <View style={styles.copyNotice} pointerEvents="none">
            <Text style={styles.copyNoticeText}>Copied</Text>
          </View>
        ) : null}
      </View>
    );
  },
);

TerminalPane.displayName = 'TerminalPane';

const styles = StyleSheet.create({
  root: {
    flex: 1,
    backgroundColor: TerminalTheme.bgColor,
  },
  hiddenInput: {
    position: 'absolute',
    left: 0,
    top: 0,
    width: 1,
    height: 1,
    opacity: 0.01,
    color: 'transparent',
    backgroundColor: 'transparent',
    padding: 0,
  },
  copyNotice: {
    position: 'absolute',
    alignSelf: 'center',
    bottom: 16,
    paddingHorizontal: 12,
    paddingVertical: 7,
    borderRadius: 8,
    backgroundColor: OkenaColors.surfaceOverlay,
    borderWidth: StyleSheet.hairlineWidth,
    borderColor: OkenaColors.borderLight,
  },
  copyNoticeText: {
    color: OkenaColors.textPrimary,
    fontSize: 12,
    fontWeight: '600',
  },
});

export default TerminalPane;
