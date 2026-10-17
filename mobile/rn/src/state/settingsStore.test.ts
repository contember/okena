/// <reference types="jest" />

import { createMemoryPersistence, type Persistence } from './persistence';
import {
  configureSettingsStore,
  DEFAULT_FONT_SIZE,
  DEFAULT_PROJECT_SORT_MODE,
  DEFAULT_RENDERER,
  MAX_FONT_SIZE,
  MIN_FONT_SIZE,
  normalizeFontSize,
  SETTINGS_KEY,
  SETTINGS_VERSION,
  useSettingsStore,
} from './settingsStore';

function setup(initial: Record<string, string> = {}): Persistence {
  const persistence = createMemoryPersistence(initial);
  configureSettingsStore({ persistence });
  useSettingsStore.setState({
    terminalFontSize: DEFAULT_FONT_SIZE,
    renderer: DEFAULT_RENDERER,
    projectSortMode: DEFAULT_PROJECT_SORT_MODE,
    openKeyboardWithTerminal: false,
    loaded: false,
  });
  return persistence;
}

async function persisted(persistence: Persistence): Promise<unknown> {
  // Writes are fire-and-forget; let them land.
  await Promise.resolve();
  const json = await persistence.getItem(SETTINGS_KEY);
  return json === null ? null : JSON.parse(json);
}

describe('normalizeFontSize', () => {
  it('clamps to the range and snaps to half points', () => {
    expect(normalizeFontSize(2)).toBe(MIN_FONT_SIZE);
    expect(normalizeFontSize(99)).toBe(MAX_FONT_SIZE);
    expect(normalizeFontSize(11.3)).toBe(11.5);
    expect(normalizeFontSize(11.2)).toBe(11);
    expect(normalizeFontSize(Number.NaN)).toBe(DEFAULT_FONT_SIZE);
  });
});

describe('settings store', () => {
  it('defaults to a font about 20% smaller than the old 13pt', () => {
    expect(DEFAULT_FONT_SIZE).toBe(10.5);
    expect(MIN_FONT_SIZE).toBe(8);
    expect(MAX_FONT_SIZE).toBe(20);
  });

  it('loads the persisted font size', async () => {
    setup({ [SETTINGS_KEY]: JSON.stringify({ terminalFontSize: 14 }) });
    await useSettingsStore.getState().load();
    expect(useSettingsStore.getState()).toMatchObject({ terminalFontSize: 14, loaded: true });
  });

  it('normalizes an out-of-range persisted value', async () => {
    setup({ [SETTINGS_KEY]: JSON.stringify({ terminalFontSize: 64 }) });
    await useSettingsStore.getState().load();
    expect(useSettingsStore.getState().terminalFontSize).toBe(MAX_FONT_SIZE);
  });

  it('falls back to defaults on missing or corrupt data', async () => {
    setup();
    await useSettingsStore.getState().load();
    expect(useSettingsStore.getState()).toMatchObject({
      terminalFontSize: DEFAULT_FONT_SIZE,
      loaded: true,
    });

    setup({ [SETTINGS_KEY]: '{not json' });
    await useSettingsStore.getState().load();
    expect(useSettingsStore.getState().terminalFontSize).toBe(DEFAULT_FONT_SIZE);
  });

  it('persists font size changes', async () => {
    const persistence = setup();
    useSettingsStore.getState().setTerminalFontSize(12);
    expect(useSettingsStore.getState().terminalFontSize).toBe(12);
    expect(await persisted(persistence)).toEqual({
      version: SETTINGS_VERSION,
      terminalFontSize: 12,
      renderer: 'native',
      projectSortMode: 'activity',
      openKeyboardWithTerminal: false,
    });
  });

  it('steps by half a point and stops at the limits', () => {
    setup();
    const { stepTerminalFontSize } = useSettingsStore.getState();
    stepTerminalFontSize(1);
    expect(useSettingsStore.getState().terminalFontSize).toBe(11);
    stepTerminalFontSize(-2);
    expect(useSettingsStore.getState().terminalFontSize).toBe(10);
    stepTerminalFontSize(-100);
    expect(useSettingsStore.getState().terminalFontSize).toBe(MIN_FONT_SIZE);
    stepTerminalFontSize(100);
    expect(useSettingsStore.getState().terminalFontSize).toBe(MAX_FONT_SIZE);
  });

  it('resets to the default', async () => {
    const persistence = setup();
    useSettingsStore.getState().setTerminalFontSize(16);
    useSettingsStore.getState().resetTerminalFontSize();
    expect(useSettingsStore.getState().terminalFontSize).toBe(DEFAULT_FONT_SIZE);
    expect(await persisted(persistence)).toEqual({
      version: SETTINGS_VERSION,
      terminalFontSize: DEFAULT_FONT_SIZE,
      renderer: 'native',
      projectSortMode: 'activity',
      openKeyboardWithTerminal: false,
    });
  });
});

describe('renderer setting', () => {
  it('defaults to the native renderer', async () => {
    setup();
    await useSettingsStore.getState().load();
    expect(useSettingsStore.getState().renderer).toBe('native');
  });

  it('loads the persisted renderer and ignores unknown values', async () => {
    setup({
      [SETTINGS_KEY]: JSON.stringify({ version: 2, terminalFontSize: 12, renderer: 'skia' }),
    });
    await useSettingsStore.getState().load();
    expect(useSettingsStore.getState()).toMatchObject({ terminalFontSize: 12, renderer: 'skia' });

    setup({ [SETTINGS_KEY]: JSON.stringify({ version: 2, renderer: 'webgl' }) });
    await useSettingsStore.getState().load();
    expect(useSettingsStore.getState().renderer).toBe(DEFAULT_RENDERER);
  });

  it('moves version 1 settings to the native renderer once, keeping the font size', async () => {
    // Version 1 wrote the old 'skia' default on every font-size change.
    const persistence = setup({
      [SETTINGS_KEY]: JSON.stringify({ terminalFontSize: 12, renderer: 'skia' }),
    });
    await useSettingsStore.getState().load();
    expect(useSettingsStore.getState()).toMatchObject({ terminalFontSize: 12, renderer: 'native' });

    // A later explicit choice of Skia sticks across reloads.
    useSettingsStore.getState().setRenderer('skia');
    expect(await persisted(persistence)).toEqual({
      version: SETTINGS_VERSION,
      terminalFontSize: 12,
      renderer: 'skia',
      projectSortMode: 'activity',
      openKeyboardWithTerminal: false,
    });
    useSettingsStore.setState({ renderer: DEFAULT_RENDERER, loaded: false });
    await useSettingsStore.getState().load();
    expect(useSettingsStore.getState().renderer).toBe('skia');
  });

  it('persists the renderer next to the font size', async () => {
    const persistence = setup();
    useSettingsStore.getState().setTerminalFontSize(14);
    useSettingsStore.getState().setRenderer('skia');
    expect(useSettingsStore.getState().renderer).toBe('skia');
    expect(await persisted(persistence)).toEqual({
      version: SETTINGS_VERSION,
      terminalFontSize: 14,
      renderer: 'skia',
      projectSortMode: 'activity',
      openKeyboardWithTerminal: false,
    });
  });
});

describe('project sort mode setting', () => {
  it('defaults to activity', async () => {
    setup();
    await useSettingsStore.getState().load();
    expect(useSettingsStore.getState().projectSortMode).toBe('activity');
  });

  it('persists and reloads manual, keeping the renderer migration intact', async () => {
    const persistence = setup();
    useSettingsStore.getState().setProjectSortMode('manual');
    expect(await persisted(persistence)).toEqual({
      version: SETTINGS_VERSION,
      terminalFontSize: DEFAULT_FONT_SIZE,
      renderer: 'native',
      projectSortMode: 'manual',
      openKeyboardWithTerminal: false,
    });
    useSettingsStore.setState({ projectSortMode: DEFAULT_PROJECT_SORT_MODE, loaded: false });
    await useSettingsStore.getState().load();
    expect(useSettingsStore.getState().projectSortMode).toBe('manual');
  });

  it('ignores unknown values and loads version 1 objects without one', async () => {
    setup({ [SETTINGS_KEY]: JSON.stringify({ version: 2, projectSortMode: 'alpha' }) });
    await useSettingsStore.getState().load();
    expect(useSettingsStore.getState().projectSortMode).toBe(DEFAULT_PROJECT_SORT_MODE);

    setup({ [SETTINGS_KEY]: JSON.stringify({ terminalFontSize: 12, renderer: 'skia' }) });
    await useSettingsStore.getState().load();
    expect(useSettingsStore.getState()).toMatchObject({
      terminalFontSize: 12,
      renderer: 'native',
      projectSortMode: 'activity',
      openKeyboardWithTerminal: false,
    });
  });
});

describe('open keyboard with terminal setting', () => {
  it('defaults to off, today\'s behaviour, also for older settings objects', async () => {
    setup({ [SETTINGS_KEY]: JSON.stringify({ terminalFontSize: 12, renderer: 'skia' }) });
    await useSettingsStore.getState().load();
    expect(useSettingsStore.getState()).toMatchObject({
      openKeyboardWithTerminal: false,
      renderer: 'native',
    });
  });

  it('persists and reloads on, ignoring non-boolean values', async () => {
    const persistence = setup();
    useSettingsStore.getState().setOpenKeyboardWithTerminal(true);
    expect(await persisted(persistence)).toMatchObject({
      version: SETTINGS_VERSION,
      openKeyboardWithTerminal: true,
    });
    useSettingsStore.setState({ openKeyboardWithTerminal: false, loaded: false });
    await useSettingsStore.getState().load();
    expect(useSettingsStore.getState().openKeyboardWithTerminal).toBe(true);

    setup({ [SETTINGS_KEY]: JSON.stringify({ version: 2, openKeyboardWithTerminal: 'yes' }) });
    await useSettingsStore.getState().load();
    expect(useSettingsStore.getState().openKeyboardWithTerminal).toBe(false);
  });
});
