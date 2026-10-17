/// <reference types="jest" />
// tsconfig pins `types` to react/react-native, so pull jest globals in here.

/**
 * TerminalPane regression: keyboard focus and the toolbar's target agree
 * (CORR-14), and touch drags scroll the pane they started in.
 */

import React from 'react';
import { Keyboard, TextInput, View } from 'react-native';
import TestRenderer, { act } from 'react-test-renderer';
import type { SkFont } from '@shopify/react-native-skia';

import { KeyModifiers } from './KeyToolbar';
import { TerminalPane } from './TerminalPane';
import type { OkenaNative } from '../native/okena';
import { createMemoryPersistence } from '../state/persistence';
import { configureSettingsStore, useSettingsStore } from '../state/settingsStore';

// The Skia renderer needs a GPU surface and the clipboard a native module;
// the input chrome under test needs neither.
// The mock keeps the renderer's props so a test can report a grid size.
const mockTerminalViewProps: {
  current?: {
    onGridSizeChange?: (cols: number, rows: number, cw: number, ch: number) => void;
    fontSize?: number;
  };
} = {};
jest.mock('./TerminalView', () => ({
  TerminalView: (props: typeof mockTerminalViewProps.current) => {
    mockTerminalViewProps.current = props;
    return null;
  },
}));
jest.mock('../native/okena', () => ({ getOkenaNative: () => ({}) }));
jest.mock('@react-native-clipboard/clipboard', () => ({ setString: () => {} }));

const unusedFontMethod = (): never => {
  throw new Error('font metrics are unused while TerminalView is mocked out');
};

const FONT: SkFont = {
  __typename__: 'Font',
  dispose: unusedFontMethod,
  [Symbol.dispose]: unusedFontMethod,
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
  native?: OkenaNative;
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
      native={options.native}
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

describe('TerminalPane drag scrolling', () => {
  afterEach(() => {
    jest.restoreAllMocks();
    jest.useRealTimers();
  });

  function scrollNative() {
    const scrollWheel = jest.fn();
    const native = {
      scrollWheel,
      startSelection: jest.fn(),
      updateSelection: jest.fn(),
      clearSelection: jest.fn(),
      getSelectedText: jest.fn(),
    } as unknown as OkenaNative;
    return { native, scrollWheel };
  }

  /** The full-bleed responder view (the only one claiming touches). */
  function gestureSurface(renderer: TestRenderer.ReactTestRenderer) {
    return renderer.root.find(
      (n) => n.type === View && typeof n.props.onResponderMove === 'function',
    );
  }

  function touch(locationX: number, locationY: number, pageX: number, pageY: number) {
    return { nativeEvent: { locationX, locationY, pageX, pageY } };
  }

  function setup(onSelect?: (id: string) => void) {
    jest.useFakeTimers();
    const { native, scrollWheel } = scrollNative();
    const renderer = renderPane('t1', { native, onSelect });
    act(() => {
      mockTerminalViewProps.current?.onGridSizeChange?.(40, 20, 8, 16);
    });
    spyOnInputFocus(renderer);
    return { renderer, scrollWheel, surface: gestureSurface(renderer) };
  }

  it('scrolls whole lines in the finger direction', () => {
    const { renderer, scrollWheel, surface } = setup();
    act(() => {
      surface.props.onResponderGrant(touch(20, 100, 20, 300));
      surface.props.onResponderMove(touch(20, 120, 20, 320)); // past slop, 1 line
      surface.props.onResponderMove(touch(20, 150, 20, 350)); // 50px total
      surface.props.onResponderRelease(touch(20, 150, 20, 350));
    });
    // 20px → 1 line (4px left), +30px → 34px → 2 lines. Cell at y=150 → row 9.
    expect(scrollWheel.mock.calls).toEqual([
      ['conn-1', 't1', 1, 2, 7],
      ['conn-1', 't1', 2, 2, 9],
    ]);
    act(() => renderer.unmount());
  });

  it('measures movement from page coordinates once the finger leaves the pane', () => {
    const { renderer, scrollWheel, surface } = setup();
    act(() => {
      surface.props.onResponderGrant(touch(20, 100, 20, 300));
      // locationY is relative to whatever view is under the finger now (here a
      // view below this pane): it must not be read as a pane coordinate.
      surface.props.onResponderMove(touch(20, 5, 20, 268));
      surface.props.onResponderRelease(touch(20, 5, 20, 268));
    });
    expect(scrollWheel).toHaveBeenCalledWith('conn-1', 't1', -2, 2, 4);
    act(() => renderer.unmount());
  });

  it('treats a still touch as a tap that selects the pane, not a scroll', () => {
    const onSelect = jest.fn();
    const { renderer, scrollWheel, surface } = setup(onSelect);
    act(() => {
      surface.props.onResponderGrant(touch(20, 100, 20, 300));
      surface.props.onResponderMove(touch(22, 103, 22, 303));
      surface.props.onResponderRelease(touch(22, 103, 22, 303));
    });
    expect(scrollWheel).not.toHaveBeenCalled();
    expect(onSelect).toHaveBeenCalledWith('t1');
    act(() => renderer.unmount());
  });

  it('does not count a system-cancelled touch as a tap', () => {
    const onSelect = jest.fn();
    const { renderer, surface } = setup(onSelect);
    act(() => {
      surface.props.onResponderGrant(touch(20, 100, 20, 300));
      surface.props.onResponderTerminate(touch(20, 100, 20, 300));
    });
    expect(onSelect).not.toHaveBeenCalled();
    act(() => renderer.unmount());
  });

  it('keeps the touch for the whole gesture', () => {
    const { renderer, surface } = setup();
    expect(surface.props.onResponderTerminationRequest()).toBe(false);
    act(() => renderer.unmount());
  });
});

describe('TerminalPane font size', () => {
  beforeEach(() => {
    configureSettingsStore({ persistence: createMemoryPersistence() });
    useSettingsStore.setState({ terminalFontSize: 10.5, loaded: true });
  });

  function twoFingers(distance: number) {
    return {
      nativeEvent: {
        locationX: 50,
        locationY: 50,
        pageX: 50,
        pageY: 50,
        touches: [
          { pageX: 100, pageY: 100 },
          { pageX: 100 + distance, pageY: 100 },
        ],
      },
    };
  }

  function surfaceOf(renderer: TestRenderer.ReactTestRenderer) {
    return renderer.root.find(
      (n) => n.type === View && typeof n.props.onResponderMove === 'function',
    );
  }

  it('renders the terminal at the persisted font size', () => {
    const renderer = renderPane('t1');
    expect(mockTerminalViewProps.current?.fontSize).toBe(10.5);
    act(() => useSettingsStore.getState().setTerminalFontSize(14));
    expect(mockTerminalViewProps.current?.fontSize).toBe(14);
    act(() => renderer.unmount());
  });

  it('pinches to zoom without tapping or scrolling', () => {
    const onSelect = jest.fn();
    const scrollWheel = jest.fn();
    const native = { scrollWheel } as unknown as OkenaNative;
    const renderer = renderPane('t1', { onSelect, native });
    const surface = surfaceOf(renderer);
    act(() => {
      surface.props.onResponderGrant(twoFingers(100));
      surface.props.onResponderMove(twoFingers(100)); // pinch starts
      surface.props.onResponderMove(twoFingers(200)); // 2x
      surface.props.onResponderRelease(twoFingers(200));
    });
    expect(useSettingsStore.getState().terminalFontSize).toBe(20);
    expect(scrollWheel).not.toHaveBeenCalled();
    expect(onSelect).not.toHaveBeenCalled();
    act(() => renderer.unmount());
  });
});
