/**
 * keyInput.ts — pure key encoding + the key toolbar layout.
 *
 * Kept free of native modules (no Skia, no clipboard) so the encoding and the
 * toolbar layout are unit-testable and shared between {@link KeyToolbar} and
 * {@link TerminalPane} (the soft keyboard applies the same modifiers).
 *
 * Every toolbar key resolves to a {@link KeyInput}: either a named
 * `SpecialKey` (sent via `sendSpecialKey`, encoded by
 * `okena_core::keys::SpecialKey::to_bytes`) or raw text (sent via `sendText`).
 * Sequences the named enum cannot express over the FFI (Shift+Tab, Ctrl+L,
 * Ctrl+R, modified arrows) are sent as raw text.
 */

import { useSyncExternalStore } from 'react';

import type { SpecialKey } from '../native/okena';

// ── Shared modifier state ──────────────────────────────────────────────────

/** Modifier state: inactive, active for one key, or locked until released. */
export type ModifierState = 'inactive' | 'active' | 'locked';

export interface ModifierSnapshot {
  ctrl: ModifierState;
  option: ModifierState;
  cmd: ModifierState;
}

/** Boolean view of the modifiers, as consumed by the encoders. */
export interface ModifierFlags {
  ctrl: boolean;
  option: boolean;
  cmd: boolean;
}

const INITIAL_SNAPSHOT: ModifierSnapshot = {
  ctrl: 'inactive',
  option: 'inactive',
  cmd: 'inactive',
};

/**
 * Shared modifier store between {@link KeyToolbar} and {@link TerminalPane}.
 *
 * A minimal external store (subscribe + getSnapshot) so several components
 * stay in sync via {@link useKeyModifiers}. A new immutable snapshot is swapped
 * in on every change so `useSyncExternalStore` re-renders subscribers.
 */
export class KeyModifiers implements ModifierFlags {
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
    this.toggle('ctrl');
  }
  lockCtrl(): void {
    this.lock('ctrl');
  }
  toggleOption(): void {
    this.toggle('option');
  }
  lockOption(): void {
    this.lock('option');
  }
  toggleCmd(): void {
    this.toggle('cmd');
  }
  lockCmd(): void {
    this.lock('cmd');
  }

  toggle(which: keyof ModifierSnapshot): void {
    this.emit({
      ...this.snapshot,
      [which]: this.snapshot[which] === 'inactive' ? 'active' : 'inactive',
    });
  }

  lock(which: keyof ModifierSnapshot): void {
    this.emit({
      ...this.snapshot,
      [which]: this.snapshot[which] === 'locked' ? 'inactive' : 'locked',
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

const NO_MODIFIERS: ModifierFlags = { ctrl: false, option: false, cmd: false };

// ── Byte sequences ─────────────────────────────────────────────────────────

/**
 * Bytes each named special key produces — a mirror of
 * `okena_core::keys::SpecialKey::to_bytes` (crates/okena-core/src/keys.rs).
 * Used to ESC-prefix a special key under Alt, and by the tests to assert what
 * every toolbar key ends up writing to the PTY.
 */
export const SPECIAL_KEY_BYTES: Readonly<Record<SpecialKey, string>> = {
  Enter: '\r',
  Escape: '\x1b',
  CtrlC: '\x03',
  CtrlD: '\x04',
  CtrlZ: '\x1a',
  Tab: '\t',
  ArrowUp: '\x1b[A',
  ArrowDown: '\x1b[B',
  ArrowRight: '\x1b[C',
  ArrowLeft: '\x1b[D',
  Home: '\x1b[H',
  End: '\x1b[F',
  PageUp: '\x1b[5~',
  PageDown: '\x1b[6~',
  Backspace: '\x7f',
  Delete: '\x1b[3~',
};

/** Back-tab (Shift+Tab): CSI Z. Claude Code cycles permission/plan mode on it. */
export const SHIFT_TAB = '\x1b[Z';

/** What a key press writes: a named special key or raw text. */
export type KeyInput =
  | { kind: 'special'; key: SpecialKey }
  | { kind: 'text'; text: string };

/** The bytes a {@link KeyInput} ends up writing to the PTY. */
export function keyInputBytes(input: KeyInput): string {
  return input.kind === 'special' ? SPECIAL_KEY_BYTES[input.key] : input.text;
}

/** Control character for `ch` under Ctrl, or `null` when it has none. */
export function controlChar(ch: string): string | null {
  if (ch.length !== 1) return null;
  const code = ch.charCodeAt(0);
  if (code >= 0x61 && code <= 0x7a) return String.fromCharCode(code - 0x60); // a-z
  if (code >= 0x40 && code <= 0x5f) return String.fromCharCode(code - 0x40); // @ A-Z [ \ ] ^ _
  if (ch === ' ') return '\x00';
  if (ch === '?') return '\x7f';
  return null;
}

/**
 * Encode one character with modifiers (xterm conventions): Ctrl maps to the
 * control character (characters without one pass through), Alt/Meta prefix
 * ESC. Ctrl+Alt yields ESC + control character.
 */
export function encodeChar(ch: string, mods: ModifierFlags = NO_MODIFIERS): string {
  const base = mods.ctrl ? controlChar(ch) ?? ch : ch;
  return mods.option || mods.cmd ? '\x1b' + base : base;
}

/**
 * Apply the active modifiers to typed text (soft keyboard), char by char.
 * Does NOT reset the modifiers (the caller does, after sending).
 */
export function applyModifiersToText(mods: ModifierFlags, chars: string): string {
  if (!mods.ctrl && !mods.option && !mods.cmd) return chars;
  let out = '';
  for (const ch of chars) out += encodeChar(ch, mods);
  return out;
}

export type ArrowDirection = 'up' | 'down' | 'left' | 'right';

const ARROW_SPECIAL: Record<ArrowDirection, SpecialKey> = {
  up: 'ArrowUp',
  down: 'ArrowDown',
  left: 'ArrowLeft',
  right: 'ArrowRight',
};
const ARROW_FINAL: Record<ArrowDirection, string> = {
  up: 'A',
  down: 'B',
  right: 'C',
  left: 'D',
};
/** Meta-only arrows jump like macOS Cmd+arrow. */
const META_ARROW: Record<ArrowDirection, SpecialKey> = {
  left: 'Home',
  right: 'End',
  up: 'PageUp',
  down: 'PageDown',
};

/**
 * Encode an arrow with modifiers: plain → the named key; Meta only →
 * Home/End/PageUp/PageDown; Ctrl/Alt → `CSI 1 ; <mod> <A-D>` with
 * mod = 1 + 2·alt + 4·ctrl (xterm).
 */
export function encodeArrow(
  dir: ArrowDirection,
  mods: ModifierFlags = NO_MODIFIERS,
): KeyInput {
  if (!mods.ctrl && !mods.option) {
    return { kind: 'special', key: mods.cmd ? META_ARROW[dir] : ARROW_SPECIAL[dir] };
  }
  const m = 1 + (mods.option ? 2 : 0) + (mods.ctrl ? 4 : 0);
  return { kind: 'text', text: `\x1b[1;${m}${ARROW_FINAL[dir]}` };
}

// ── Toolbar layout ─────────────────────────────────────────────────────────

/** Modifier keys exposed in the toolbar (Meta is redundant with Alt + home/end/pg keys). */
export type ToolbarModifier = 'ctrl' | 'option';

export type KeyAction =
  /** A fixed sequence. Alt/Meta prefix it with ESC (e.g. Alt+Enter = newline in Claude Code). */
  | { type: 'send'; input: KeyInput }
  /** A printable character; all modifiers apply. */
  | { type: 'char'; char: string }
  /** An arrow; modifiers produce the xterm modified-arrow sequences. */
  | { type: 'arrow'; dir: ArrowDirection }
  | { type: 'modifier'; modifier: ToolbarModifier }
  | { type: 'paste' }
  | { type: 'compose' }
  /** Open the native "Select text" mode of the target terminal. */
  | { type: 'selectText' }
  | { type: 'keyboard' }
  | { type: 'more' };

export type KeyIconName = 'up' | 'down' | 'left' | 'right' | 'return' | 'keyboard' | 'more';

export interface ToolbarKey {
  id: string;
  /** Visible text label (plain ASCII only — no font-dependent glyphs). */
  label?: string;
  /** Small caption rendered above the label (e.g. "shift" over "tab"). */
  caption?: string;
  /** View-drawn icon used instead of a text label. */
  icon?: KeyIconName;
  accessibilityLabel: string;
  action: KeyAction;
  /** Hold to auto-repeat (arrows, delete). */
  repeat?: boolean;
  /** Visual emphasis for the most important key in a row. */
  emphasis?: boolean;
}

const special = (key: SpecialKey): KeyAction => ({
  type: 'send',
  input: { kind: 'special', key },
});
const text = (value: string): KeyAction => ({
  type: 'send',
  input: { kind: 'text', text: value },
});
const char = (c: string, accessibilityLabel: string): ToolbarKey => ({
  id: `char-${c}`,
  label: c,
  accessibilityLabel,
  action: { type: 'char', char: c },
});

/**
 * The always-visible row: what driving Claude Code needs constantly.
 * esc interrupts, ^C clears/exits, tab completes, shift+tab cycles permission
 * and plan mode, up/down walk history and menus, return confirms.
 */
export const PRIMARY_KEYS: readonly ToolbarKey[] = [
  { id: 'esc', label: 'esc', accessibilityLabel: 'Escape', action: special('Escape') },
  { id: 'ctrl-c', label: '^C', accessibilityLabel: 'Control C', action: special('CtrlC') },
  { id: 'tab', label: 'tab', accessibilityLabel: 'Tab', action: special('Tab') },
  {
    id: 'shift-tab',
    label: 'tab',
    caption: 'shift',
    accessibilityLabel: 'Shift Tab',
    action: text(SHIFT_TAB),
  },
  { id: 'up', icon: 'up', accessibilityLabel: 'Arrow up', action: { type: 'arrow', dir: 'up' }, repeat: true },
  { id: 'down', icon: 'down', accessibilityLabel: 'Arrow down', action: { type: 'arrow', dir: 'down' }, repeat: true },
  { id: 'enter', icon: 'return', accessibilityLabel: 'Return', action: special('Enter'), emphasis: true },
  { id: 'more', icon: 'more', accessibilityLabel: 'More keys', action: { type: 'more' } },
];

/**
 * The expandable panel above the primary row, top to bottom. Same 8-column
 * grid as the primary row; left/right sit directly above up/down, and the
 * keyboard toggle sits above the More key.
 */
export const MORE_ROWS: readonly (readonly ToolbarKey[])[] = [
  [
    { id: 'paste', label: 'paste', accessibilityLabel: 'Paste clipboard', action: { type: 'paste' } },
    { id: 'compose', label: 'edit', accessibilityLabel: 'Compose input', action: { type: 'compose' } },
    { id: 'select', label: 'select', accessibilityLabel: 'Select text', action: { type: 'selectText' } },
    char('/', 'Slash'),
    char('@', 'At sign'),
    char('|', 'Pipe'),
    char('~', 'Tilde'),
    char('\\', 'Backslash'),
  ],
  [
    { id: 'ctrl', label: 'ctrl', accessibilityLabel: 'Control modifier', action: { type: 'modifier', modifier: 'ctrl' } },
    { id: 'alt', label: 'alt', accessibilityLabel: 'Alt modifier', action: { type: 'modifier', modifier: 'option' } },
    { id: 'ctrl-d', label: '^D', accessibilityLabel: 'Control D', action: special('CtrlD') },
    char('`', 'Backtick'),
    { id: 'ctrl-z', label: '^Z', accessibilityLabel: 'Control Z', action: special('CtrlZ') },
    { id: 'ctrl-r', label: '^R', accessibilityLabel: 'Control R, history search', action: text('\x12') },
    { id: 'pgup', label: 'pgup', accessibilityLabel: 'Page up', action: special('PageUp'), repeat: true },
    { id: 'pgdn', label: 'pgdn', accessibilityLabel: 'Page down', action: special('PageDown'), repeat: true },
  ],
  [
    { id: 'home', label: 'home', accessibilityLabel: 'Home', action: special('Home') },
    { id: 'end', label: 'end', accessibilityLabel: 'End', action: special('End') },
    char('-', 'Dash'),
    char('_', 'Underscore'),
    { id: 'left', icon: 'left', accessibilityLabel: 'Arrow left', action: { type: 'arrow', dir: 'left' }, repeat: true },
    { id: 'right', icon: 'right', accessibilityLabel: 'Arrow right', action: { type: 'arrow', dir: 'right' }, repeat: true },
    { id: 'del', label: 'del', accessibilityLabel: 'Forward delete', action: special('Delete'), repeat: true },
    { id: 'keyboard', icon: 'keyboard', accessibilityLabel: 'Toggle keyboard', action: { type: 'keyboard' } },
  ],
];

/**
 * Resolve a key action to the input it sends under the given modifiers, or
 * `null` for UI actions (paste, compose, select text, modifiers, keyboard, more).
 */
export function resolveKeyAction(
  action: KeyAction,
  mods: ModifierFlags = NO_MODIFIERS,
): KeyInput | null {
  switch (action.type) {
    case 'send':
      if (mods.option || mods.cmd) {
        return { kind: 'text', text: '\x1b' + keyInputBytes(action.input) };
      }
      return action.input;
    case 'char':
      return { kind: 'text', text: encodeChar(action.char, mods) };
    case 'arrow':
      return encodeArrow(action.dir, mods);
    default:
      return null;
  }
}
