/// <reference types="jest" />

/**
 * KeyToolbar wiring: taps reach the right native call, the More panel
 * expands/collapses, and modifiers combine with the next key.
 */

import React from 'react';
import { Keyboard } from 'react-native';
import TestRenderer, { act } from 'react-test-renderer';

import { KeyToolbar } from './KeyToolbar';
import { KeyModifiers } from './keyInput';
import type { OkenaNative } from '../native/okena';

jest.mock('../native/okena', () => ({ getOkenaNative: () => ({}) }));
jest.mock('@react-native-clipboard/clipboard', () => ({
  getString: () => Promise.resolve(''),
  setString: () => {},
}));

function setup(options: { keyboardVisible?: boolean } = {}) {
  jest.spyOn(Keyboard, 'isVisible').mockReturnValue(options.keyboardVisible ?? false);
  const native = {
    sendText: jest.fn((_c: string, _t: string, _text: string) => Promise.resolve()),
    sendSpecialKey: jest.fn((_c: string, _t: string, _key: string) => Promise.resolve()),
    getScrollInfo: jest.fn(() => ({ displayOffset: 0 })),
    scroll: jest.fn(),
  };
  const modifiers = new KeyModifiers();
  const onHideKeyboard = jest.fn();
  const onShowKeyboard = jest.fn();
  const onSelectText = jest.fn();
  let renderer!: TestRenderer.ReactTestRenderer;
  act(() => {
    renderer = TestRenderer.create(
      <KeyToolbar
        connId="c1"
        terminalId="t1"
        modifiers={modifiers}
        native={native as unknown as OkenaNative}
        onHideKeyboard={onHideKeyboard}
        onShowKeyboard={onShowKeyboard}
        onSelectText={onSelectText}
      />,
    );
  });
  // The outermost node carrying the key's label is the Pressable itself.
  const matches = (label: string) =>
    renderer.root.findAll(
      (n) => n.props.accessibilityLabel === label && n.props.accessibilityRole === 'button',
    );
  const key = (label: string) => {
    const found = matches(label);
    if (found.length === 0) throw new Error(`no key ${label}`);
    return found[0]!;
  };
  const hasKey = (label: string) => matches(label).length > 0;
  const tap = (label: string) => act(() => key(label).props.onPress());
  const pressIn = (label: string) =>
    act(() => {
      key(label).props.onPressIn();
      key(label).props.onPressOut();
    });
  return {
    renderer,
    native,
    modifiers,
    onHideKeyboard,
    onShowKeyboard,
    onSelectText,
    key,
    hasKey,
    tap,
    pressIn,
  };
}

describe('KeyToolbar', () => {
  afterEach(() => jest.restoreAllMocks());

  it('sends named keys through sendSpecialKey', () => {
    const t = setup();
    t.tap('Escape');
    t.tap('Control C');
    t.tap('Tab');
    t.tap('Return');
    expect(t.native.sendSpecialKey.mock.calls.map((c) => c[2])).toEqual([
      'Escape',
      'CtrlC',
      'Tab',
      'Enter',
    ]);
    act(() => t.renderer.unmount());
  });

  it('sends Shift+Tab as CSI Z text', () => {
    const t = setup();
    t.tap('More keys');
    t.tap('Shift Tab');
    expect(t.native.sendText).toHaveBeenCalledWith('c1', 't1', '\x1b[Z');
    act(() => t.renderer.unmount());
  });

  it('fires arrows on press-in (hold repeats)', () => {
    const t = setup();
    t.pressIn('Arrow up');
    t.pressIn('Arrow down');
    expect(t.native.sendSpecialKey.mock.calls.map((c) => c[2])).toEqual([
      'ArrowUp',
      'ArrowDown',
    ]);
    act(() => t.renderer.unmount());
  });

  it('expands and collapses the More panel', () => {
    const t = setup();
    expect(t.hasKey('Slash')).toBe(false);
    t.tap('More keys');
    expect(t.hasKey('Slash')).toBe(true);
    t.tap('Slash');
    expect(t.native.sendText).toHaveBeenCalledWith('c1', 't1', '/');
    // The panel stays open for repeated use.
    expect(t.hasKey('Slash')).toBe(true);
    t.tap('More keys');
    expect(t.hasKey('Slash')).toBe(false);
    act(() => t.renderer.unmount());
  });

  it('applies a one-shot ctrl to the next key, then releases it', () => {
    const t = setup();
    t.tap('More keys');
    t.tap('Control modifier');
    expect(t.modifiers.ctrl).toBe(true);
    t.pressIn('Arrow left');
    expect(t.native.sendText).toHaveBeenCalledWith('c1', 't1', '\x1b[1;5D');
    expect(t.modifiers.ctrl).toBe(false);
    act(() => t.renderer.unmount());
  });

  it('locks a modifier on long press', () => {
    const t = setup();
    t.tap('More keys');
    act(() => {
      const ctrl = t.key('Control modifier');
      ctrl.props.onPressIn();
      ctrl.props.onLongPress();
      ctrl.props.onPress();
    });
    expect(t.modifiers.getSnapshot().ctrl).toBe('locked');
    act(() => t.renderer.unmount());
  });

  it('toggles the soft keyboard', () => {
    const hidden = setup({ keyboardVisible: false });
    hidden.tap('More keys');
    hidden.tap('Toggle keyboard');
    expect(hidden.onShowKeyboard).toHaveBeenCalled();
    act(() => hidden.renderer.unmount());

    const shown = setup({ keyboardVisible: true });
    shown.tap('More keys');
    shown.tap('Toggle keyboard');
    expect(shown.onHideKeyboard).toHaveBeenCalled();
    act(() => shown.renderer.unmount());
  });

  it('opens compose from the primary row with autocorrect on', () => {
    const t = setup();
    t.tap('Compose input');
    const field = t.renderer.root.findByProps({ multiline: true });
    expect(field.props.autoCorrect).toBe(true);
    expect(field.props.autoCapitalize).toBe('sentences');
    expect(t.native.sendText).not.toHaveBeenCalled();
    act(() => t.renderer.unmount());
  });

    it('opens the select text mode without sending input', () => {
    const t = setup();
    t.tap('More keys');
    t.tap('Select text');
    expect(t.onSelectText).toHaveBeenCalledTimes(1);
    expect(t.native.sendText).not.toHaveBeenCalled();
    expect(t.native.sendSpecialKey).not.toHaveBeenCalled();
    act(() => t.renderer.unmount());
  });
});
