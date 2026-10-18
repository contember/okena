/// <reference types="jest" />
// tsconfig pins `types` to react/react-native, so pull jest globals in here.

/**
 * TerminalPane regression: keyboard focus and the toolbar's target agree
 * (CORR-14), touch drags scroll the pane they started in, and the renderer
 * setting picks the native view or Skia.
 */

import React from 'react';
import { Keyboard, Platform, TextInput, View } from 'react-native';
import TestRenderer, { act } from 'react-test-renderer';
import type { SkFont } from '@shopify/react-native-skia';
import type { OkenaTerminalViewProps } from 'okena-terminal-view';

import { KeyModifiers } from './KeyToolbar';
import { AUTO_FOCUS_DELAY_MS, TerminalPane, type TerminalPaneHandle } from './TerminalPane';
import type { OkenaNative } from '../native/okena';
import { createMemoryPersistence } from '../state/persistence';
import { configureSettingsStore, DEFAULT_RENDERER, useSettingsStore } from '../state/settingsStore';

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
// The native renderer is a codegen host component; the mock keeps its props
// so a test can fire its events, and records commands.
const mockNativeViewProps: { current?: OkenaTerminalViewProps } = {};
const mockScrollToBottom = jest.fn();
const mockOpenSelectText = jest.fn();
jest.mock('okena-terminal-view', () => {
  const mockReact: typeof React = jest.requireActual('react');
  const OkenaTerminalView = mockReact.forwardRef(
    (props: OkenaTerminalViewProps, ref: React.Ref<{ nativeView: true }>) => {
      mockNativeViewProps.current = props;
      mockReact.useImperativeHandle(ref, () => ({ nativeView: true as const }));
      return null;
    },
  );
  return {
    OkenaTerminalView,
    OkenaTerminalViewCommands: {
      scrollToBottom: (ref: unknown) => mockScrollToBottom(ref),
      clearSelection: () => {},
      openSelectText: (ref: unknown) => mockOpenSelectText(ref),
    },
  };
});
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
  autoFocus?: boolean;
  focusBlocked?: boolean;
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
      autoFocus={options.autoFocus}
      focusBlocked={options.focusBlocked}
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

// Selection, focus, drag and pinch below exercise the Skia renderer's JS
// gesture surface; the native view handles those gestures itself.
beforeEach(() => {
  useSettingsStore.setState({ renderer: 'skia' });
});

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

describe('TerminalPane open keyboard with terminal', () => {
  beforeEach(() => {
    jest.useFakeTimers();
    jest.spyOn(Keyboard, 'isVisible').mockReturnValue(false);
  });

  afterEach(() => {
    jest.useRealTimers();
    jest.restoreAllMocks();
  });

  /** The renderer laid out: what the auto-focus waits for. */
  const reportGrid = () =>
    act(() => mockTerminalViewProps.current?.onGridSizeChange?.(80, 24, 8, 16));

  it('waits for the renderer to lay out before opening the keyboard', () => {
    const renderer = renderPane('t1', { selected: true, autoFocus: true });
    const focus = spyOnInputFocus(renderer);
    act(() => jest.advanceTimersByTime(AUTO_FOCUS_DELAY_MS * 3));
    expect(focus).not.toHaveBeenCalled();
    reportGrid();
    act(() => jest.advanceTimersByTime(AUTO_FOCUS_DELAY_MS));
    expect(focus).toHaveBeenCalledTimes(1);
    act(() => renderer.unmount());
  });

  it('opens the keyboard when shown with the setting on', () => {
    const renderer = renderPane('t1', { selected: true, autoFocus: true });
    const focus = spyOnInputFocus(renderer);
    reportGrid();
    act(() => jest.advanceTimersByTime(AUTO_FOCUS_DELAY_MS));
    expect(focus).toHaveBeenCalledTimes(1);
    act(() => renderer.unmount());
  });

  it('leaves the keyboard closed with the setting off', () => {
    const renderer = renderPane('t1', { selected: true, autoFocus: false });
    const focus = spyOnInputFocus(renderer);
    reportGrid();
    act(() => jest.advanceTimersByTime(AUTO_FOCUS_DELAY_MS * 2));
    expect(focus).not.toHaveBeenCalled();
    act(() => renderer.unmount());
  });

  it('defaults to the persisted setting, off unless chosen', () => {
    useSettingsStore.setState({ openKeyboardWithTerminal: false });
    let renderer = renderPane('t1', { selected: true });
    let focus = spyOnInputFocus(renderer);
    reportGrid();
    act(() => jest.advanceTimersByTime(AUTO_FOCUS_DELAY_MS));
    expect(focus).not.toHaveBeenCalled();
    act(() => renderer.unmount());

    useSettingsStore.setState({ openKeyboardWithTerminal: true });
    renderer = renderPane('t1', { selected: true });
    focus = spyOnInputFocus(renderer);
    reportGrid();
    act(() => jest.advanceTimersByTime(AUTO_FOCUS_DELAY_MS));
    expect(focus).toHaveBeenCalledTimes(1);
    act(() => renderer.unmount());
    useSettingsStore.setState({ openKeyboardWithTerminal: false });
  });

  it('waits while an overlay is open, then opens once it closes', () => {
    const renderer = renderPane('t1', { selected: true, autoFocus: true, focusBlocked: true });
    const focus = spyOnInputFocus(renderer);
    reportGrid();
    act(() => jest.advanceTimersByTime(AUTO_FOCUS_DELAY_MS * 2));
    expect(focus).not.toHaveBeenCalled();

    act(() => {
      renderer.update(pane('t1', { selected: true, autoFocus: true, focusBlocked: false }));
    });
    act(() => jest.advanceTimersByTime(AUTO_FOCUS_DELAY_MS));
    expect(focus).toHaveBeenCalledTimes(1);
    act(() => renderer.unmount());
  });

  it('does not reopen after an overlay that did not switch terminals', () => {
    const renderer = renderPane('t1', { selected: true, autoFocus: true });
    const focus = spyOnInputFocus(renderer);
    reportGrid();
    act(() => jest.advanceTimersByTime(AUTO_FOCUS_DELAY_MS));
    expect(focus).toHaveBeenCalledTimes(1);

    for (const focusBlocked of [true, false]) {
      act(() => {
        renderer.update(pane('t1', { selected: true, autoFocus: true, focusBlocked }));
      });
      act(() => jest.advanceTimersByTime(AUTO_FOCUS_DELAY_MS));
    }
    expect(focus).toHaveBeenCalledTimes(1);
    act(() => renderer.unmount());
  });

  it('opens again for a newly shown terminal', () => {
    const renderer = renderPane('t1', { selected: true, autoFocus: true });
    const focus = spyOnInputFocus(renderer);
    reportGrid();
    act(() => jest.advanceTimersByTime(AUTO_FOCUS_DELAY_MS));
    act(() => {
      renderer.update(pane('t2', { selected: true, autoFocus: true }));
    });
    act(() => jest.advanceTimersByTime(AUTO_FOCUS_DELAY_MS));
    expect(focus).toHaveBeenCalledTimes(2);
    act(() => renderer.unmount());
  });

  it('does not follow the selection into a pane under an overlay', () => {
    (Keyboard.isVisible as jest.Mock).mockReturnValue(true);
    const renderer = renderPane('t3', { selected: false, focusBlocked: true, autoFocus: false });
    const focus = spyOnInputFocus(renderer);
    reportGrid();
    act(() => {
      renderer.update(pane('t3', { selected: true, focusBlocked: true, autoFocus: false }));
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

describe('TerminalPane renderer', () => {
  beforeEach(() => {
    configureSettingsStore({ persistence: createMemoryPersistence() });
    useSettingsStore.setState({ terminalFontSize: 10.5, renderer: 'skia', loaded: true });
    mockNativeViewProps.current = undefined;
    mockTerminalViewProps.current = undefined;
    mockScrollToBottom.mockClear();
  });

  afterEach(() => {
    jest.restoreAllMocks();
  });

  function gestureSurfaces(renderer: TestRenderer.ReactTestRenderer) {
    return renderer.root.findAll(
      (n) => n.type === View && typeof n.props.onResponderMove === 'function',
    );
  }

  function nativeProps(): OkenaTerminalViewProps {
    const props = mockNativeViewProps.current;
    if (!props) throw new Error('the native renderer was not rendered');
    return props;
  }

  /** Fire a native event handler with `{ nativeEvent }`, as Fabric does. */
  function fire<T>(handler: ((e: never) => void) | undefined, nativeEvent: T) {
    if (!handler) throw new Error('missing event handler');
    act(() => (handler as (e: { nativeEvent: T }) => void)({ nativeEvent }));
  }

  it('uses the native view by default', () => {
    useSettingsStore.setState({ renderer: DEFAULT_RENDERER });
    const renderer = renderPane('t1');
    expect(mockNativeViewProps.current).toBeDefined();
    expect(mockTerminalViewProps.current).toBeUndefined();
    expect(gestureSurfaces(renderer)).toHaveLength(0);
    act(() => renderer.unmount());
  });

  it('uses the Skia renderer and its gesture surface when chosen', () => {
    const renderer = renderPane('t1');
    expect(mockTerminalViewProps.current).toBeDefined();
    expect(mockNativeViewProps.current).toBeUndefined();
    expect(gestureSurfaces(renderer)).toHaveLength(1);
    act(() => renderer.unmount());
  });

  it('switches to the native view without JS gestures', () => {
    const renderer = renderPane('t1');
    mockTerminalViewProps.current = undefined;
    act(() => useSettingsStore.getState().setRenderer('native'));

    expect(nativeProps()).toMatchObject({
      connId: 'conn-1',
      terminalId: 't1',
      fontSize: 10.5,
      focused: false,
      theme: { background: 0xff000000 },
    });
    expect(mockTerminalViewProps.current).toBeUndefined();
    expect(gestureSurfaces(renderer)).toHaveLength(0);
    expect(renderer.root.findAllByType(TextInput)).toHaveLength(1);
    act(() => renderer.unmount());
  });

  it('focuses the hidden input and selects the pane on a native tap', () => {
    useSettingsStore.setState({ renderer: 'native' });
    const onSelect = jest.fn();
    const renderer = renderPane('t2', { onSelect });
    const focus = spyOnInputFocus(renderer);

    fire(nativeProps().onTap, { col: 3, row: 4 });

    expect(onSelect).toHaveBeenCalledWith('t2');
    expect(focus).toHaveBeenCalled();
    act(() => renderer.unmount());
  });

  describe('reopening the keyboard after the system hid it', () => {
    const realOS = Platform.OS;
    afterEach(() => {
      Platform.OS = realOS;
      jest.restoreAllMocks();
    });

    /** A focused input with the keyboard down: Android after the Back key. */
    function focusedWithoutKeyboard(os: 'android' | 'ios') {
      Platform.OS = os;
      useSettingsStore.setState({ renderer: 'native' });
      jest.spyOn(Keyboard, 'isVisible').mockReturnValue(false);
      const renderer = renderPane('t2');
      const input = renderer.root.findByType(TextInput).instance;
      jest.spyOn(input, 'isFocused').mockReturnValue(true);
      const blur = jest.spyOn(input, 'blur').mockImplementation(() => {});
      const focus = spyOnInputFocus(renderer);
      blur.mockClear();
      return { renderer, blur, focus };
    }

    it('blurs and refocuses on Android so a tap shows the keyboard again', () => {
      const { renderer, blur, focus } = focusedWithoutKeyboard('android');
      fire(nativeProps().onTap, { col: 0, row: 0 });
      expect(blur).toHaveBeenCalledTimes(1);
      expect(focus).toHaveBeenCalledTimes(1);
      expect(blur.mock.invocationCallOrder[0]).toBeLessThan(focus.mock.invocationCallOrder[0]);
      act(() => renderer.unmount());
    });

    it('keeps the focus on iOS (a hardware keyboard has no soft keyboard)', () => {
      const { renderer, blur, focus } = focusedWithoutKeyboard('ios');
      fire(nativeProps().onTap, { col: 0, row: 0 });
      expect(blur).not.toHaveBeenCalled();
      expect(focus).toHaveBeenCalledTimes(1);
      act(() => renderer.unmount());
    });

    it('does not blur while the keyboard is up', () => {
      const { renderer, blur, focus } = focusedWithoutKeyboard('android');
      jest.spyOn(Keyboard, 'isVisible').mockReturnValue(true);
      fire(nativeProps().onTap, { col: 0, row: 0 });
      expect(blur).not.toHaveBeenCalled();
      expect(focus).toHaveBeenCalledTimes(1);
      act(() => renderer.unmount());
    });
  });

  it('persists a font size from a native pinch', () => {
    useSettingsStore.setState({ renderer: 'native' });
    const renderer = renderPane('t1');

    fire(nativeProps().onFontSizeChange, { size: 14 });

    expect(useSettingsStore.getState().terminalFontSize).toBe(14);
    expect(nativeProps().fontSize).toBe(14);
    act(() => renderer.unmount());
  });

  it('shows a Paused chip while the native view holds output for a selection', () => {
    useSettingsStore.setState({ renderer: 'native' });
    const renderer = renderPane('t1');
    const chip = () => renderer.root.findAll((n) => n.props.testID === 'terminal-output-paused');
    expect(chip()).toHaveLength(0);

    fire(nativeProps().onOutputPausedChange, { paused: true });
    expect(chip().length).toBeGreaterThan(0);

    fire(nativeProps().onOutputPausedChange, { paused: false });
    expect(chip()).toHaveLength(0);
    act(() => renderer.unmount());
  });

  it('leaves PTY sizing to the native view', () => {
    useSettingsStore.setState({ renderer: 'native' });
    const resizeLocal = jest.fn();
    const resizeTerminal = jest.fn();
    const native = { resizeLocal, resizeTerminal } as unknown as OkenaNative;
    const renderer = renderPane('t1', { native });

    fire(nativeProps().onGridSize, { cols: 60, rows: 30, cellWidth: 6.3, cellHeight: 15.6 });

    expect(resizeLocal).not.toHaveBeenCalled();
    expect(resizeTerminal).not.toHaveBeenCalled();
    act(() => renderer.unmount());
  });

  it('scrolls the native view to the bottom on typed input', () => {
    useSettingsStore.setState({ renderer: 'native' });
    const sendText = jest.fn();
    const native = { sendText, sendSpecialKey: jest.fn() } as unknown as OkenaNative;
    const renderer = renderPane('t1', { native });
    const input = renderer.root.findByType(TextInput);

    act(() => {
      input.props.onChange({ nativeEvent: { text: `${input.props.value}a` } });
    });

    expect(sendText).toHaveBeenCalledWith('conn-1', 't1', 'a');
    expect(mockScrollToBottom).toHaveBeenCalledWith({ nativeView: true });
    act(() => renderer.unmount());
  });

  it('scrolls the native view to the bottom through the pane handle', () => {
    useSettingsStore.setState({ renderer: 'native' });
    const ref = React.createRef<TerminalPaneHandle>();
    let renderer!: TestRenderer.ReactTestRenderer;
    act(() => {
      renderer = TestRenderer.create(
        <TerminalPane
          ref={ref}
          connId="conn-1"
          terminalId="t1"
          fonts={{ regular: FONT }}
          modifiers={new KeyModifiers()}
        />,
      );
    });

    act(() => ref.current?.scrollToBottom());

    expect(mockScrollToBottom).toHaveBeenCalledWith({ nativeView: true });

    act(() => ref.current?.openSelectText());
    expect(mockOpenSelectText).toHaveBeenCalledWith({ nativeView: true });
    act(() => renderer.unmount());
  });
});
