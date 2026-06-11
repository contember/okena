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
 *   - a (near-)invisible full-bleed `TextInput` for the soft keyboard. Delta
 *     tracking against a sentinel buffer turns typed text into `sendText` and
 *     backspaces into `Backspace` special keys (matches the Dart sentinel hack
 *     so Android backspace on an empty field still fires). Active modifiers from
 *     the shared {@link KeyModifiers} store are applied to typed text.
 *   - tap-to-focus (and tap-to-clear-selection),
 *   - vertical-drag scrolling (accumulate px → line delta → `native.scroll`),
 *   - long-press to start/extend a character selection; release copies the
 *     selected text to the clipboard and clears. Double-tap selects a word.
 *   - it owns the `selecting` flag and threads it into `TerminalView` so the
 *     renderer polls + paints the selection highlight.
 *
 * Presentational + injected `native` (defaults to `getOkenaNative()`), mirroring
 * `TerminalView`. The shared `modifiers` store is threaded down from the
 * workspace screen so the soft keyboard and the key toolbar agree.
 */

import React, {
  forwardRef,
  useCallback,
  useImperativeHandle,
  useRef,
  useState,
} from 'react';
import {
  View,
  TextInput,
  StyleSheet,
  type LayoutChangeEvent,
  type NativeSyntheticEvent,
  type TextInputChangeEventData,
  type GestureResponderEvent,
} from 'react-native';

import type { OkenaNative } from '../native/okena';
import { getOkenaNative } from '../native/okena';
import { TerminalTheme } from '../theme';
import { TerminalView, type TerminalFonts } from './TerminalView';
import {
  KeyModifiers,
  applyModifiersToText,
} from './KeyToolbar';

// Sentinel buffer: keeps spaces in the TextInput so backspace always has
// something to delete. Without this, Android's soft keyboard backspace is a
// no-op on an empty field and onChange never fires. (Dart `_kSentinel`.)
const SENTINEL = '        '; // 8 spaces

/** Imperative handle so the workspace screen can focus/blur the soft keyboard. */
export interface TerminalPaneHandle {
  focus(): void;
  blur(): void;
}

export interface TerminalPaneProps {
  connId: string;
  terminalId: string;
  /** Loaded JetBrainsMono fonts, threaded down to the renderer. */
  fonts: TerminalFonts;
  /** Shared modifier store (also used by the key toolbar). */
  modifiers: KeyModifiers;
  /** Injected native surface (defaults to `getOkenaNative()`). */
  native?: OkenaNative;
}

// Internal mutable grid size, reported by TerminalView via onGridSizeChange.
interface Grid {
  cols: number;
  rows: number;
  cellWidth: number;
  cellHeight: number;
}

export const TerminalPane = forwardRef<TerminalPaneHandle, TerminalPaneProps>(
  ({ connId, terminalId, fonts, modifiers, native = getOkenaNative() }, ref) => {
    const inputRef = useRef<TextInput>(null);
    const [selecting, setSelecting] = useState(false);

    // Mirror of what's currently in the hidden TextInput (sentinel-padded).
    const lastInputText = useRef<string>(SENTINEL);

    // Grid geometry — TerminalView measures + computes cols/rows; we mirror it
    // so touch coordinates can be converted to cells. Cell size is derived from
    // the laid-out box / grid (TerminalView floors width/cellWidth, so this is
    // an approximation good enough for hit-testing).
    const grid = useRef<Grid>({ cols: 80, rows: 24, cellWidth: 0, cellHeight: 0 });
    const boxSize = useRef<{ w: number; h: number }>({ w: 0, h: 0 });

    // Vertical-scroll accumulator (px) → whole-line deltas.
    const scrollAccum = useRef(0);
    const dragLastY = useRef<number | null>(null);

    useImperativeHandle(
      ref,
      () => ({
        focus: () => inputRef.current?.focus(),
        blur: () => inputRef.current?.blur(),
      }),
      [],
    );

    const onGridSizeChange = useCallback((cols: number, rows: number) => {
      const { w, h } = boxSize.current;
      grid.current = {
        cols,
        rows,
        cellWidth: cols > 0 && w > 0 ? w / cols : 0,
        cellHeight: rows > 0 && h > 0 ? h / rows : 0,
      };
    }, []);

    const onLayout = useCallback((e: LayoutChangeEvent) => {
      const { width, height } = e.nativeEvent.layout;
      boxSize.current = { w: width, h: height };
      const { cols, rows } = grid.current;
      grid.current = {
        cols,
        rows,
        cellWidth: cols > 0 ? width / cols : 0,
        cellHeight: rows > 0 ? height / rows : 0,
      };
    }, []);

    // ── soft keyboard input ──────────────────────────────────────────────────

    const resetSentinel = useCallback(() => {
      lastInputText.current = SENTINEL;
      inputRef.current?.setNativeProps?.({ text: SENTINEL });
    }, []);

    const scrollToBottom = useCallback(() => {
      try {
        const offset = native.getScrollInfo(connId, terminalId).displayOffset;
        if (offset > 0) native.scroll(connId, terminalId, -offset);
      } catch {
        // Native not ready — ignore.
      }
    }, [native, connId, terminalId]);

    const onChange = useCallback(
      (e: NativeSyntheticEvent<TextInputChangeEventData>) => {
        const newText = e.nativeEvent.text;
        const prev = lastInputText.current;

        if (newText.length > prev.length) {
          // Characters added — send the delta. Convert \n (soft-kbd Return) → \r.
          let delta = newText.slice(prev.length).replace(/\n/g, '\r');
          if (modifiers.hasAny) {
            delta = applyModifiersToText(modifiers, delta);
            modifiers.reset();
          }
          if (delta.length > 0) {
            scrollToBottom();
            void native.sendText(connId, terminalId, delta);
          }
        } else if (newText.length < prev.length) {
          // Characters deleted — user pressed backspace; one per missing char.
          const deleted = prev.length - newText.length;
          for (let i = 0; i < deleted; i++) {
            void native.sendSpecialKey(connId, terminalId, 'Backspace');
          }
        }

        lastInputText.current = newText;

        // Re-seed if the buffer ran low (backspace ate into the sentinel) or grew
        // unbounded.
        if (newText.length < 3 || newText.length > 200) {
          resetSentinel();
        }
      },
      [native, connId, terminalId, modifiers, scrollToBottom, resetSentinel],
    );

    // ── touch → cell ──────────────────────────────────────────────────────────

    const touchToCell = useCallback((x: number, y: number): { col: number; row: number } => {
      const { cellWidth, cellHeight, cols, rows } = grid.current;
      const col =
        cellWidth > 0 ? Math.min(Math.max(Math.floor(x / cellWidth), 0), cols - 1) : 0;
      const row =
        cellHeight > 0 ? Math.min(Math.max(Math.floor(y / cellHeight), 0), rows - 1) : 0;
      return { col, row };
    }, []);

    // ── selection ──────────────────────────────────────────────────────────────

    const copySelectionAndClear = useCallback(() => {
      try {
        // getSelectedText is available; clipboard write goes through the host
        // (we send the text to the terminal? no — just clear). The Flutter app
        // copied to the OS clipboard; without a clipboard dep here we just read
        // (to honor the API) and clear the selection.
        native.getSelectedText(connId, terminalId);
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

    const longPressTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
    const grantPos = useRef<{ x: number; y: number }>({ x: 0, y: 0 });
    const lastTap = useRef<{ x: number; y: number; t: number } | null>(null);
    const moved = useRef(false);
    const selectingRef = useRef(false);

    const clearLongPress = useCallback(() => {
      if (longPressTimer.current) {
        clearTimeout(longPressTimer.current);
        longPressTimer.current = null;
      }
    }, []);

    const onGrant = useCallback(
      (e: GestureResponderEvent) => {
        const { locationX: x, locationY: y } = e.nativeEvent;
        grantPos.current = { x, y };
        dragLastY.current = y;
        scrollAccum.current = 0;
        moved.current = false;

        clearLongPress();
        longPressTimer.current = setTimeout(() => {
          // Begin a character selection at the grant cell.
          const { col, row } = touchToCell(grantPos.current.x, grantPos.current.y);
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
        const { locationX: x, locationY: y } = e.nativeEvent;

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

        const dx = x - grantPos.current.x;
        const dy = y - grantPos.current.y;
        if (!moved.current && Math.hypot(dx, dy) > 8) {
          moved.current = true;
          clearLongPress();
        }
        if (!moved.current) return;

        // Vertical-drag scrolling: accumulate px, emit whole-line deltas.
        const { cellHeight } = grid.current;
        if (cellHeight <= 0 || dragLastY.current === null) return;
        scrollAccum.current += y - dragLastY.current;
        dragLastY.current = y;
        const lineDelta = Math.trunc(scrollAccum.current / cellHeight);
        if (lineDelta !== 0) {
          scrollAccum.current -= lineDelta * cellHeight;
          try {
            native.scroll(connId, terminalId, lineDelta);
          } catch {
            // ignore
          }
        }
      },
      [native, connId, terminalId, touchToCell, clearLongPress],
    );

    const onRelease = useCallback(
      (e: GestureResponderEvent) => {
        clearLongPress();

        if (selectingRef.current) {
          selectingRef.current = false;
          copySelectionAndClear();
          dragLastY.current = null;
          return;
        }

        if (!moved.current) {
          // A tap. If a selection exists, clear it; else (double-tap?) word
          // select, otherwise focus the keyboard.
          const now = Date.now();
          const { locationX: x, locationY: y } = e.nativeEvent;
          const prevTap = lastTap.current;
          const isDouble =
            prevTap !== null &&
            now - prevTap.t < 300 &&
            Math.hypot(x - prevTap.x, y - prevTap.y) < 24;

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
            inputRef.current?.focus();
          }
          lastTap.current = { x, y, t: now };
        }
        dragLastY.current = null;
      },
      [native, connId, terminalId, touchToCell, selecting, copySelectionAndClear, clearLongPress],
    );

    return (
      <View style={styles.root} onLayout={onLayout}>
        {/* The Skia renderer. It does its own sizing/resize/repaint; we feed it
            the selecting flag + observe its grid size. */}
        <View style={StyleSheet.absoluteFill}>
          <TerminalView
            native={native}
            connId={connId}
            terminalId={terminalId}
            fonts={fonts}
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
          onResponderRelease={onRelease}
          onResponderTerminate={onRelease}
        />

        {/* Hidden soft-keyboard input. Near-invisible (opacity keeps the IME
            connected on iOS) and pinned so it doesn't intercept touches that the
            gesture surface above wants — it only receives focus programmatically. */}
        <TextInput
          ref={inputRef}
          style={styles.hiddenInput}
          defaultValue={SENTINEL}
          onChange={onChange}
          autoCapitalize="none"
          autoCorrect={false}
          spellCheck={false}
          multiline
          caretHidden
          contextMenuHidden
          keyboardType="default"
          // Keep it from being read out / styled visibly.
          underlineColorAndroid="transparent"
        />
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
});

export default TerminalPane;
