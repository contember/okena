/// <reference types="jest" />

/**
 * Key toolbar mapping: every key must write the right bytes to the PTY.
 * Expected values are the xterm sequences, written out literally so a typo in
 * `SPECIAL_KEY_BYTES` (the mirror of okena_core::keys::SpecialKey::to_bytes)
 * or in the layout fails here.
 */

import {
  KeyModifiers,
  MORE_ROWS,
  PRIMARY_KEYS,
  SHIFT_TAB,
  SPECIAL_KEY_BYTES,
  applyModifiersToText,
  encodeArrow,
  encodeChar,
  keyInputBytes,
  resolveKeyAction,
  type ModifierFlags,
  type ToolbarKey,
} from '../src/components/keyInput';

const NONE: ModifierFlags = { ctrl: false, option: false, cmd: false };
const CTRL: ModifierFlags = { ctrl: true, option: false, cmd: false };
const ALT: ModifierFlags = { ctrl: false, option: true, cmd: false };
const META: ModifierFlags = { ctrl: false, option: false, cmd: true };
const CTRL_ALT: ModifierFlags = { ctrl: true, option: true, cmd: false };

const ALL_KEYS: ToolbarKey[] = [...PRIMARY_KEYS, ...MORE_ROWS.flat()];

function bytesOf(id: string, mods: ModifierFlags = NONE): string | null {
  const key = ALL_KEYS.find((k) => k.id === id);
  if (!key) throw new Error(`no key ${id}`);
  const input = resolveKeyAction(key.action, mods);
  return input === null ? null : keyInputBytes(input);
}

describe('toolbar layout', () => {
  it('uses one 8-column grid for every row, so nothing scrolls or clips', () => {
    expect(PRIMARY_KEYS).toHaveLength(8);
    for (const row of MORE_ROWS) expect(row).toHaveLength(8);
  });

  it('has unique ids and an accessibility label on every key', () => {
    const ids = ALL_KEYS.map((k) => k.id);
    expect(new Set(ids).size).toBe(ids.length);
    for (const key of ALL_KEYS) {
      expect(key.accessibilityLabel.length).toBeGreaterThan(0);
      expect(key.label !== undefined || key.icon !== undefined).toBe(true);
    }
  });

  it('labels keys with plain ASCII only (no font-dependent glyphs)', () => {
    for (const key of ALL_KEYS) {
      for (const text of [key.label, key.caption]) {
        if (text !== undefined) expect(text).toMatch(/^[\x20-\x7e]+$/);
      }
    }
  });

  it('keeps the Claude Code essentials in the primary row', () => {
    expect(PRIMARY_KEYS.map((k) => k.id)).toEqual([
      'esc',
      'ctrl-c',
      'tab',
      'shift-tab',
      'up',
      'down',
      'enter',
      'more',
    ]);
  });
});

describe('key bytes', () => {
  it.each([
    ['esc', '\x1b'],
    ['ctrl-c', '\x03'],
    ['tab', '\t'],
    ['shift-tab', '\x1b[Z'],
    ['up', '\x1b[A'],
    ['down', '\x1b[B'],
    ['enter', '\r'],
    ['ctrl-d', '\x04'],
    ['ctrl-z', '\x1a'],
    ['ctrl-r', '\x12'],
    ['pgup', '\x1b[5~'],
    ['pgdn', '\x1b[6~'],
    ['home', '\x1b[H'],
    ['end', '\x1b[F'],
    ['left', '\x1b[D'],
    ['right', '\x1b[C'],
    ['del', '\x1b[3~'],
    ['char-/', '/'],
    ['char-@', '@'],
    ['char-|', '|'],
    ['char-~', '~'],
    ['char-\\', '\\'],
    ['char-`', '`'],
    ['char--', '-'],
    ['char-_', '_'],
  ])('%s sends %j', (id, expected) => {
    expect(bytesOf(id)).toBe(expected);
  });

  it('sends Shift+Tab as back-tab (CSI Z), not a named key', () => {
    expect(SHIFT_TAB).toBe('\x1b[Z');
    const key = PRIMARY_KEYS.find((k) => k.id === 'shift-tab')!;
    expect(resolveKeyAction(key.action)).toEqual({ kind: 'text', text: '\x1b[Z' });
  });

  it('covers every key that sends input', () => {
    const silent = ALL_KEYS.filter((k) => bytesOf(k.id) === null).map((k) => k.id);
    expect(silent.sort()).toEqual(['alt', 'compose', 'ctrl', 'keyboard', 'more', 'paste', 'select']);
  });

  it('mirrors okena_core SpecialKey::to_bytes', () => {
    expect(SPECIAL_KEY_BYTES).toEqual({
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
    });
  });
});

describe('modifiers', () => {
  it('encodes characters under ctrl / alt / both', () => {
    expect(encodeChar('c', CTRL)).toBe('\x03');
    expect(encodeChar('C', CTRL)).toBe('\x03');
    expect(encodeChar('[', CTRL)).toBe('\x1b');
    expect(encodeChar(' ', CTRL)).toBe('\x00');
    expect(encodeChar('/', CTRL)).toBe('/');
    expect(encodeChar('x', ALT)).toBe('\x1bx');
    expect(encodeChar('x', META)).toBe('\x1bx');
    expect(encodeChar('b', CTRL_ALT)).toBe('\x1b\x02');
  });

  it('encodes arrows like xterm', () => {
    expect(encodeArrow('up')).toEqual({ kind: 'special', key: 'ArrowUp' });
    expect(keyInputBytes(encodeArrow('left', CTRL))).toBe('\x1b[1;5D');
    expect(keyInputBytes(encodeArrow('right', ALT))).toBe('\x1b[1;3C');
    expect(keyInputBytes(encodeArrow('up', CTRL_ALT))).toBe('\x1b[1;7A');
    expect(encodeArrow('left', META)).toEqual({ kind: 'special', key: 'Home' });
    expect(encodeArrow('down', META)).toEqual({ kind: 'special', key: 'PageDown' });
  });

  it('ESC-prefixes fixed keys under alt (Alt+Enter = newline in Claude Code)', () => {
    expect(bytesOf('enter', ALT)).toBe('\x1b\r');
    expect(bytesOf('shift-tab', ALT)).toBe('\x1b\x1b[Z');
  });

  it('leaves fixed keys alone under ctrl', () => {
    expect(bytesOf('esc', CTRL)).toBe('\x1b');
    expect(bytesOf('tab', CTRL)).toBe('\t');
  });

  it('applies modifiers to typed text', () => {
    expect(applyModifiersToText(NONE, 'ls')).toBe('ls');
    expect(applyModifiersToText(CTRL, 'a')).toBe('\x01');
    expect(applyModifiersToText(ALT, 'ab')).toBe('\x1ba\x1bb');
  });

  it('resolves against the live modifier store', () => {
    const mods = new KeyModifiers();
    mods.toggle('ctrl');
    const slash = ALL_KEYS.find((k) => k.id === 'char-@')!;
    expect(resolveKeyAction(slash.action, mods)).toEqual({ kind: 'text', text: '\x00' });
  });
});
