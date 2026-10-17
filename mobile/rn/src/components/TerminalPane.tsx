/**
 * TerminalPane.tsx — the per-terminal container that drives the terminal renderer.
 *
 * Two renderers, picked by the `renderer` setting (settings store):
 *   - `native`: the platform view from `okena-terminal-view`. It owns scrolling,
 *     pinch, selection and PTY sizing itself and reports back through events
 *     (`onTap`, `onGridSize`, `onFontSizeChange`, `onSelectionCopied`). This
 *     container then only keeps the soft-keyboard input below, turns a tap
 *     into keyboard focus and sends `scrollToBottom` after input. JS does
 *     not resize the PTY in this mode. Nothing consumes `onScrollStateChange`
 *     yet.
 *   - `skia`: the JS + Skia renderer, with the gesture chrome described below.
 *
 * Skia mode:
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
import {
  OkenaTerminalView,
  OkenaTerminalViewCommands,
  type FontSizeChangeEvent,
  type GridSizeEvent,
  type TerminalViewTheme,
} from 'okena-terminal-view';
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
  /**
   * Bring the native renderer back to the live bottom after input sent from
   * elsewhere (the key toolbar). A no-op with the Skia renderer, which follows
   * the Rust-side scroll position.
   */
  scrollToBottom(): void;
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
  /**
   * Open the soft keyboard when the pane is shown (the "Open keyboard with
   * terminal" setting). Defaults to the persisted setting. The pane is keyed
   * by terminal, so "shown" covers launch / restore, a project switch and a
   * terminal chip switch.
   */
  autoFocus?: boolean;
  /**
   * An overlay (drawer, switcher, settings, a sheet) is on top: never take
   * keyboard focus. A pending auto-focus waits until the overlay closes.
   */
  focusBlocked?: boolean;
  /** Font size in pt. Defaults to the persisted setting (settings store). */
  fontSize?: number;
  /** Injected native surface (defaults to `getOkenaNative()`). */
  native?: OkenaNative;
}

/**
 * Delay before an automatic keyboard focus: long enough for a closing modal
 * (drawer slide-out is 220 ms) to leave, short enough to feel immediate.
 */
export const AUTO_FOCUS_DELAY_MS = 300;

/** Distance between the first two touches, or null for a single touch. */
function pinchDistance(e: GestureResponderEvent): number | null {
  // Synthetic events (tests) may omit `touches`.
  const [a, b] = e.nativeEvent.touches ?? [];
  if (!a || !b) return null;
  return Math.hypot(a.pageX - b.pageX, a.pageY - b.pageY);
}

/** Colors for the native renderer, as packed ARGB like the Rust cell colors. */
const NATIVE_THEME: TerminalViewTheme = {
  background: TerminalTheme.bgColorArgb,
  foreground: TerminalTheme.fgColorArgb,
  cursor: TerminalTheme.cursorColorArgb,
  selection: TerminalTheme.selectionOverlayArgb,
};

// Internal mutable grid size, reported by the renderer (onGridSizeChange /
// onGridSize).
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
      autoFocus,
      focusBlocked = false,
      fontSize,
      native = getOkenaNative(),
    },
    ref,
  ) => {
    const storedAutoFocus = useSettingsStore((s) => s.openKeyboardWithTerminal);
    const effectiveAutoFocus = autoFocus ?? storedAutoFocus;
    const storedFontSize = useSettingsStore((s) => s.terminalFontSize);
    const setFontSize = useSettingsStore((s) => s.setTerminalFontSize);
    const renderer = useSettingsStore((s) => s.renderer);
    const nativeRenderer = renderer === 'native';
    const nativeViewRef = useRef<React.ComponentRef<typeof OkenaTerminalView>>(null);
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

    // The native view keeps its own scroll offset, so the Rust-side snap to the
    // bottom on input does not move it: send it there explicitly.
    const scrollToBottom = useCallback(() => {
      if (nativeRenderer && nativeViewRef.current) {
        OkenaTerminalViewCommands.scrollToBottom(nativeViewRef.current);
      }
    }, [nativeRenderer]);

    useImperativeHandle(
      ref,
      () => ({
        focus: () => inputRef.current?.focus(),
        blur: () => inputRef.current?.blur(),
        resetInput,
        scrollToBottom,
      }),
      [resetInput, scrollToBottom],
    );

    // The renderer has laid out and reported its grid at least once.
    const [gridReady, setGridReady] = useState(false);
    const onGridSizeChange = useCallback(
      (cols: number, rows: number, cellWidth: number, cellHeight: number) => {
        grid.current = { cols, rows, cellWidth, cellHeight };
        setGridReady(true);
      },
      [],
    );

    const flashCopied = useCallback(() => {
      setCopied(true);
      if (copiedTimer.current) clearTimeout(copiedTimer.current);
      copiedTimer.current = setTimeout(() => setCopied(false), 1200);
    }, []);

    // ── soft keyboard input ──────────────────────────────────────────────────

    // A tap opens the keyboard. On Android the Back key hides it but leaves
    // the input focused, and `focus()` on a focused input does nothing, so
    // blur first. iOS has no such dismissal; there a focused input with no
    // keyboard is a hardware keyboard, which must keep its focus.
    const summonKeyboard = useCallback(() => {
      const input = inputRef.current;
      if (!input) return;
      if (Platform.OS === 'android' && input.isFocused() && !Keyboard.isVisible()) input.blur();
      input.focus();
    }, []);

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
      if (selected && !focusBlocked && Keyboard.isVisible()) inputRef.current?.focus();
      // Only a selection change moves focus; an overlay closing does not.
      // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [selected]);

    // Auto-open the keyboard once per shown terminal. It waits for the
    // renderer's first grid report and then a short delay, so the pane lays
    // out at full size first and the keyboard's shrink is an ordinary resize;
    // the delay also lets a closing overlay leave first.
    const autoFocusPending = useRef(true);
    useEffect(() => {
      autoFocusPending.current = true;
    }, [terminalId]);
    useEffect(() => {
      if (!effectiveAutoFocus || focusBlocked || !gridReady || !autoFocusPending.current) return;
      const timer = setTimeout(() => {
        autoFocusPending.current = false;
        inputRef.current?.focus();
      }, AUTO_FOCUS_DELAY_MS);
      return () => clearTimeout(timer);
    }, [effectiveAutoFocus, focusBlocked, gridReady, terminalId]);

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
        if (delta.backspaces > 0 || text.length > 0) scrollToBottom();

        if (shouldResetAfter(state, delta)) resetInput();
      },
      [native, connId, terminalId, modifiers, resetInput, scrollToBottom],
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
          flashCopied();
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
    }, [native, connId, terminalId, flashCopied]);

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
            summonKeyboard();
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
        summonKeyboard,
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

    // ── native renderer events ────────────────────────────────────────────────

    const onNativeTap = useCallback(() => {
      // Same as a Skia-mode tap: make this pane the target, then summon the
      // keyboard (`focus()` on a focused input does not re-fire `onFocus`).
      onSelect?.(terminalId);
      summonKeyboard();
    }, [onSelect, terminalId, summonKeyboard]);

    const onNativeGridSize = useCallback(
      (e: NativeSyntheticEvent<GridSizeEvent>) => {
        const { cols, rows, cellWidth, cellHeight } = e.nativeEvent;
        // Tracking only: the native view resizes the PTY itself.
        onGridSizeChange(cols, rows, cellWidth, cellHeight);
      },
      [onGridSizeChange],
    );

    const onNativeFontSizeChange = useCallback(
      (e: NativeSyntheticEvent<FontSizeChangeEvent>) => setFontSize(e.nativeEvent.size),
      [setFontSize],
    );

    return (
      <View style={styles.root}>
        {nativeRenderer ? (
          // The native renderer: draws, scrolls, pinches, selects and sizes the
          // PTY on its own. Text input stays in JS (the TextInput below).
          <OkenaTerminalView
            ref={nativeViewRef}
            style={StyleSheet.absoluteFill}
            connId={connId}
            terminalId={terminalId}
            fontSize={effectiveFontSize}
            theme={NATIVE_THEME}
            focused={selected}
            onTap={onNativeTap}
            onGridSize={onNativeGridSize}
            onFontSizeChange={onNativeFontSizeChange}
            // The view already copied to the clipboard; just confirm it.
            onSelectionCopied={flashCopied}
          />
        ) : (
          <>
            {/* The Skia renderer. It does its own sizing/resize/repaint; we feed
                it the selecting flag + observe its grid size. */}
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
          </>
        )}

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
