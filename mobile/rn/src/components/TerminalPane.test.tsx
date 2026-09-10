/// <reference types="jest" />
// tsconfig pins `types` to react/react-native, so pull jest globals in here.

/**
 * TerminalPane regression: keyboard focus and the toolbar's target agree
 * (CORR-14).
 */

import React from 'react';
import { Keyboard, TextInput } from 'react-native';
import TestRenderer, { act } from 'react-test-renderer';
import type { SkFont } from '@shopify/react-native-skia';

import { KeyModifiers } from './KeyToolbar';
import { TerminalPane } from './TerminalPane';

// The Skia renderer needs a GPU surface and the clipboard a native module;
// the input chrome under test needs neither.
jest.mock('./TerminalView', () => ({ TerminalView: () => null }));
jest.mock('../native/okena', () => ({ getOkenaNative: () => ({}) }));
jest.mock('@react-native-clipboard/clipboard', () => ({ setString: () => {} }));

const unusedFontMethod = (): never => {
  throw new Error('font metrics are unused while TerminalView is mocked out');
};

const FONT: SkFont = {
  __typename__: 'Font',
  dispose: unusedFontMethod,
  measureText: unusedFontMethod,
  getTextWidth: unusedFontMethod,
  getGlyphWidths: unusedFontMethod,
  getMetrics: unusedFontMethod,
  getGlyphIDs: unusedFontMethod,
  getGlyphIntercepts: unusedFontMethod,
  getScaleX: unusedFontMethod,
  getSize: unusedFontMethod,
  getSkewX: unusedFontMethod,
  isEmbolden: unusedFontMethod,
  getTypeface: unusedFontMethod,
  setEdging: unusedFontMethod,
  setEmbeddedBitmaps: unusedFontMethod,
  setHinting: unusedFontMethod,
  setLinearMetrics: unusedFontMethod,
  setScaleX: unusedFontMethod,
  setSize: unusedFontMethod,
  setSkewX: unusedFontMethod,
  setEmbolden: unusedFontMethod,
  setSubpixel: unusedFontMethod,
  setTypeface: unusedFontMethod,
};

interface PaneOptions {
  onSelect?: (id: string) => void;
  selected?: boolean;
}

function pane(terminalId: string, options: PaneOptions) {
  return (
    <TerminalPane
      connId="conn-1"
      terminalId={terminalId}
      fonts={{ regular: FONT }}
      modifiers={new KeyModifiers()}
      onSelect={options.onSelect}
      selected={options.selected}
    />
  );
}

function renderPane(
  terminalId: string,
  options: PaneOptions = {},
): TestRenderer.ReactTestRenderer {
  let renderer!: TestRenderer.ReactTestRenderer;
  act(() => {
    renderer = TestRenderer.create(pane(terminalId, options));
  });
  return renderer;
}

/**
 * Watch `inputRef.current.focus()` — the soft keyboard's only entry point.
 * Cleared on install: the test renderer hands out one shared TextInput
 * instance, so an earlier case's calls would otherwise still be on the spy.
 */
function spyOnInputFocus(renderer: TestRenderer.ReactTestRenderer) {
  const spy = jest
    .spyOn(renderer.root.findByType(TextInput).instance, 'focus')
    .mockImplementation(() => {});
  spy.mockClear();
  return spy;
}

describe('TerminalPane selection', () => {
  afterEach(() => {
    jest.restoreAllMocks();
  });

  it('takes keyboard focus when it becomes the selection while typing', () => {
    jest.spyOn(Keyboard, 'isVisible').mockReturnValue(true);
    const renderer = renderPane('t3', { selected: false });
    const focus = spyOnInputFocus(renderer);

    // What ＋ → New Terminal does: the poll moves the selection to the new
    // terminal while the previous pane still holds the keyboard.
    act(() => {
      renderer.update(pane('t3', { selected: true }));
    });

    expect(focus).toHaveBeenCalled();
    act(() => renderer.unmount());
  });

  it('does not summon the keyboard for a selection made while it is down', () => {
    jest.spyOn(Keyboard, 'isVisible').mockReturnValue(false);
    const renderer = renderPane('t3', { selected: false });
    const focus = spyOnInputFocus(renderer);

    act(() => {
      renderer.update(pane('t3', { selected: true }));
    });

    expect(focus).not.toHaveBeenCalled();
    act(() => renderer.unmount());
  });
});

describe('TerminalPane focus', () => {
  it('reports its own terminal id when the hidden input takes focus', () => {
    const onSelect = jest.fn();
    const renderer = renderPane('t2', { onSelect });


    act(() => {
      renderer.root.findByType(TextInput).props.onFocus();
    });

    expect(onSelect).toHaveBeenCalledWith('t2');
    act(() => renderer.unmount());
  });

  it('renders without a selection callback', () => {
    const renderer = renderPane('t1');
    expect(renderer.root.findAllByType(TextInput)).toHaveLength(1);
    act(() => renderer.unmount());
  });
});
