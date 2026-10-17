/**
 * settingsStore.ts — persisted app preferences (zustand).
 *
 * The terminal font size, the terminal renderer, the project list order and
 * whether showing a terminal opens the keyboard.
 * Stored as one JSON object
 * under {@link SETTINGS_KEY} so new preferences don't need new storage keys.
 *
 * Persistence is INJECTED via {@link configureSettingsStore} (same pattern as
 * the connection store); the default is AsyncStorage. `load()` is called once
 * at app start; until it resolves, the default size is used.
 */

import { create, type StoreApi, type UseBoundStore } from 'zustand';

import { TerminalTheme } from '../theme';
import { asyncStoragePersistence, type Persistence } from './persistence';

/** AsyncStorage key for the settings object. */
export const SETTINGS_KEY = 'app_settings';

export const DEFAULT_FONT_SIZE = TerminalTheme.defaultFontSize;
export const MIN_FONT_SIZE = TerminalTheme.minFontSize;
export const MAX_FONT_SIZE = TerminalTheme.maxFontSize;
export const FONT_SIZE_STEP = TerminalTheme.fontSizeStep;

/** Clamp to the allowed range and snap to the step grid. Non-finite → default. */
export function normalizeFontSize(size: number): number {
  if (!Number.isFinite(size)) return DEFAULT_FONT_SIZE;
  const snapped = Math.round(size / FONT_SIZE_STEP) * FONT_SIZE_STEP;
  return Math.min(MAX_FONT_SIZE, Math.max(MIN_FONT_SIZE, snapped));
}

/**
 * Which terminal renderer panes use: the native view (`okena-terminal-view`)
 * or the JS + Skia `TerminalView`. A rollout toggle, removed once the native
 * renderer replaces Skia.
 */
export type TerminalRenderer = 'native' | 'skia';

export const DEFAULT_RENDERER: TerminalRenderer = 'native';

/**
 * Version of the persisted settings object. Version 1 (no field) wrote the
 * renderer on every font-size change, so a stored 'skia' did not mean the user
 * picked Skia: it was the old default. Version 2 moved the default to 'native',
 * and a renderer is only read back from objects written at version 2 or later,
 * which moves every existing install to the native renderer once.
 */
export const SETTINGS_VERSION = 2;

function isTerminalRenderer(value: unknown): value is TerminalRenderer {
  return value === 'native' || value === 'skia';
}

/**
 * How the drawer and the switcher order projects: by activity (desktop
 * sidebar's activity view, `models/activityOrder.ts`) or in the manual order
 * with folders (`models/projectList.ts`).
 */
export type ProjectSortMode = 'activity' | 'manual';

export const DEFAULT_PROJECT_SORT_MODE: ProjectSortMode = 'activity';

function isProjectSortMode(value: unknown): value is ProjectSortMode {
  return value === 'activity' || value === 'manual';
}

/**
 * Whether showing a terminal opens the soft keyboard. Off matches the
 * behaviour before the setting existed: the keyboard opens on a tap (and stays
 * up across a switch when it already was).
 */
export const DEFAULT_OPEN_KEYBOARD_WITH_TERMINAL = false;

export interface SettingsDeps {
  persistence: Persistence;
}

export interface SettingsState {
  /** Terminal font size in pt. */
  terminalFontSize: number;
  /** Terminal renderer used by every pane. */
  renderer: TerminalRenderer;
  /** Project list order in the drawer and the switcher. */
  projectSortMode: ProjectSortMode;
  /** Open the soft keyboard whenever a terminal is shown. */
  openKeyboardWithTerminal: boolean;
  /** True once persisted settings have been read (or found absent). */
  loaded: boolean;
  /** Read persisted settings. Safe to call more than once. */
  load(): Promise<void>;
  /** Set (clamped + snapped) and persist the terminal font size. */
  setTerminalFontSize(size: number): void;
  /** Move the font size by `steps` steps (negative = smaller). */
  stepTerminalFontSize(steps: number): void;
  /** Back to {@link DEFAULT_FONT_SIZE}. */
  resetTerminalFontSize(): void;
  /** Set and persist the terminal renderer. */
  setRenderer(renderer: TerminalRenderer): void;
  /** Set and persist the project list order. */
  setProjectSortMode(mode: ProjectSortMode): void;
  /** Set and persist whether showing a terminal opens the keyboard. */
  setOpenKeyboardWithTerminal(on: boolean): void;
}

let injectedDeps: Partial<SettingsDeps> = {};

function deps(): SettingsDeps {
  return { persistence: injectedDeps.persistence ?? asyncStoragePersistence };
}

/** Override the store's dependencies (tests). */
export function configureSettingsStore(overrides: Partial<SettingsDeps>): void {
  injectedDeps = { ...injectedDeps, ...overrides };
}

interface PersistedSettings {
  version?: unknown;
  terminalFontSize?: unknown;
  renderer?: unknown;
  projectSortMode?: unknown;
  openKeyboardWithTerminal?: unknown;
}

type PersistedFields = Pick<
  SettingsState,
  'terminalFontSize' | 'renderer' | 'projectSortMode' | 'openKeyboardWithTerminal'
>;

function persist(state: PersistedFields): void {
  const payload: PersistedSettings = {
    version: SETTINGS_VERSION,
    terminalFontSize: state.terminalFontSize,
    renderer: state.renderer,
    projectSortMode: state.projectSortMode,
    openKeyboardWithTerminal: state.openKeyboardWithTerminal,
  };
  void deps()
    .persistence.setItem(SETTINGS_KEY, JSON.stringify(payload))
    .catch(() => {
      // Losing a preference write is not worth surfacing.
    });
}

export const useSettingsStore: UseBoundStore<StoreApi<SettingsState>> = create<SettingsState>(
  (set, get) => ({
    terminalFontSize: DEFAULT_FONT_SIZE,
    renderer: DEFAULT_RENDERER,
    projectSortMode: DEFAULT_PROJECT_SORT_MODE,
    openKeyboardWithTerminal: DEFAULT_OPEN_KEYBOARD_WITH_TERMINAL,
    loaded: false,

    async load() {
      let { terminalFontSize, renderer, projectSortMode, openKeyboardWithTerminal } = get();
      try {
        const json = await deps().persistence.getItem(SETTINGS_KEY);
        if (json) {
          const parsed = JSON.parse(json) as PersistedSettings;
          if (typeof parsed.terminalFontSize === 'number') {
            terminalFontSize = normalizeFontSize(parsed.terminalFontSize);
          }
          const version = typeof parsed.version === 'number' ? parsed.version : 1;
          if (version >= SETTINGS_VERSION && isTerminalRenderer(parsed.renderer)) {
            renderer = parsed.renderer;
          }
          // Introduced at version 2, so every stored value was written by a
          // build that knew the field: no version gate.
          if (isProjectSortMode(parsed.projectSortMode)) {
            projectSortMode = parsed.projectSortMode;
          }
          if (typeof parsed.openKeyboardWithTerminal === 'boolean') {
            openKeyboardWithTerminal = parsed.openKeyboardWithTerminal;
          }
        }
      } catch {
        // Corrupt or unreadable settings fall back to defaults.
      }
      set({ terminalFontSize, renderer, projectSortMode, openKeyboardWithTerminal, loaded: true });
    },

    setTerminalFontSize(size) {
      const next = normalizeFontSize(size);
      if (next === get().terminalFontSize) return;
      set({ terminalFontSize: next });
      persist(get());
    },

    stepTerminalFontSize(steps) {
      get().setTerminalFontSize(get().terminalFontSize + steps * FONT_SIZE_STEP);
    },

    resetTerminalFontSize() {
      get().setTerminalFontSize(DEFAULT_FONT_SIZE);
    },

    setRenderer(renderer) {
      if (renderer === get().renderer) return;
      set({ renderer });
      persist(get());
    },

    setProjectSortMode(projectSortMode) {
      if (projectSortMode === get().projectSortMode) return;
      set({ projectSortMode });
      persist(get());
    },

    setOpenKeyboardWithTerminal(openKeyboardWithTerminal) {
      if (openKeyboardWithTerminal === get().openKeyboardWithTerminal) return;
      set({ openKeyboardWithTerminal });
      persist(get());
    },
  }),
);

/** Imperative store handle for non-React callers (App start-up, tests). */
export function settingsStore(): StoreApi<SettingsState> {
  return useSettingsStore;
}
