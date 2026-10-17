/// <reference types="jest" />

import { createMemoryPersistence, type Persistence } from './persistence';
import {
  configureSettingsStore,
  DEFAULT_FONT_SIZE,
  DEFAULT_RENDERER,
  MAX_FONT_SIZE,
  MIN_FONT_SIZE,
  normalizeFontSize,
  SETTINGS_KEY,
  useSettingsStore,
} from './settingsStore';

function setup(initial: Record<string, string> = {}): Persistence {
  const persistence = createMemoryPersistence(initial);
  configureSettingsStore({ persistence });
  useSettingsStore.setState({
    terminalFontSize: DEFAULT_FONT_SIZE,
    renderer: DEFAULT_RENDERER,
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
    expect(await persisted(persistence)).toEqual({ terminalFontSize: 12, renderer: 'skia' });
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
      terminalFontSize: DEFAULT_FONT_SIZE,
      renderer: 'skia',
    });
  });
});

describe('renderer setting', () => {
  it('defaults to the Skia renderer', async () => {
    setup();
    await useSettingsStore.getState().load();
    expect(useSettingsStore.getState().renderer).toBe('skia');
  });

  it('loads the persisted renderer and ignores unknown values', async () => {
    setup({ [SETTINGS_KEY]: JSON.stringify({ terminalFontSize: 12, renderer: 'native' }) });
    await useSettingsStore.getState().load();
    expect(useSettingsStore.getState()).toMatchObject({ terminalFontSize: 12, renderer: 'native' });

    setup({ [SETTINGS_KEY]: JSON.stringify({ renderer: 'webgl' }) });
    await useSettingsStore.getState().load();
    expect(useSettingsStore.getState().renderer).toBe(DEFAULT_RENDERER);
  });

  it('persists the renderer next to the font size', async () => {
    const persistence = setup();
    useSettingsStore.getState().setTerminalFontSize(14);
    useSettingsStore.getState().setRenderer('native');
    expect(useSettingsStore.getState().renderer).toBe('native');
    expect(await persisted(persistence)).toEqual({ terminalFontSize: 14, renderer: 'native' });
  });
});
