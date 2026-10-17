/**
 * settingsStore.ts — persisted app preferences (zustand).
 *
 * Currently only the terminal font size. Stored as one JSON object under
 * {@link SETTINGS_KEY} so new preferences don't need new storage keys.
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

export interface SettingsDeps {
  persistence: Persistence;
}

export interface SettingsState {
  /** Terminal font size in pt. */
  terminalFontSize: number;
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
  terminalFontSize?: unknown;
}

function persist(state: Pick<SettingsState, 'terminalFontSize'>): void {
  const payload: PersistedSettings = { terminalFontSize: state.terminalFontSize };
  void deps()
    .persistence.setItem(SETTINGS_KEY, JSON.stringify(payload))
    .catch(() => {
      // Losing a preference write is not worth surfacing.
    });
}

export const useSettingsStore: UseBoundStore<StoreApi<SettingsState>> = create<SettingsState>(
  (set, get) => ({
    terminalFontSize: DEFAULT_FONT_SIZE,
    loaded: false,

    async load() {
      let terminalFontSize = get().terminalFontSize;
      try {
        const json = await deps().persistence.getItem(SETTINGS_KEY);
        if (json) {
          const parsed = JSON.parse(json) as PersistedSettings;
          if (typeof parsed.terminalFontSize === 'number') {
            terminalFontSize = normalizeFontSize(parsed.terminalFontSize);
          }
        }
      } catch {
        // Corrupt or unreadable settings fall back to defaults.
      }
      set({ terminalFontSize, loaded: true });
    },

    setTerminalFontSize(size) {
      const next = normalizeFontSize(size);
      if (next === get().terminalFontSize) return;
      set({ terminalFontSize: next });
      persist({ terminalFontSize: next });
    },

    stepTerminalFontSize(steps) {
      get().setTerminalFontSize(get().terminalFontSize + steps * FONT_SIZE_STEP);
    },

    resetTerminalFontSize() {
      get().setTerminalFontSize(DEFAULT_FONT_SIZE);
    },
  }),
);

/** Imperative store handle for non-React callers (App start-up, tests). */
export function settingsStore(): StoreApi<SettingsState> {
  return useSettingsStore;
}
