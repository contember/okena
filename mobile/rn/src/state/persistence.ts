/**
 * persistence.ts — a tiny async key-value store abstraction.
 *
 * The Flutter app persisted the saved-server list via `shared_preferences`
 * (see `connection_provider.dart`). The RN equivalent is
 * `@react-native-async-storage/async-storage`.
 *
 * The connection store talks to this {@link Persistence} interface rather than
 * AsyncStorage directly, so tests can inject an in-memory implementation (same
 * injection philosophy as `TerminalView`'s `native` prop). The default used by
 * the app is {@link asyncStoragePersistence}.
 */

/**
 * Minimal async key-value contract. A subset of the AsyncStorage API — just the
 * three calls the stores need.
 */
export interface Persistence {
  /** Read a string value, or `null` if the key is absent. */
  getItem(key: string): Promise<string | null>;
  /** Write a string value. */
  setItem(key: string, value: string): Promise<void>;
  /** Remove a key. */
  removeItem(key: string): Promise<void>;
}

/**
 * The real implementation, backed by
 * `@react-native-async-storage/async-storage`.
 *
 * The import is intentionally lazy (inside each method) so that:
 *   - merely importing this module doesn't pull in the native AsyncStorage
 *     module (which throws off-device), and
 *   - `tsc` / tests that never touch the real persistence don't need the native
 *     side present.
 */
export const asyncStoragePersistence: Persistence = {
  async getItem(key) {
    const AsyncStorage = (await import('@react-native-async-storage/async-storage'))
      .default;
    return AsyncStorage.getItem(key);
  },
  async setItem(key, value) {
    const AsyncStorage = (await import('@react-native-async-storage/async-storage'))
      .default;
    await AsyncStorage.setItem(key, value);
  },
  async removeItem(key) {
    const AsyncStorage = (await import('@react-native-async-storage/async-storage'))
      .default;
    await AsyncStorage.removeItem(key);
  },
};

/**
 * A simple in-memory {@link Persistence} — handy for tests and Storybook. Not
 * used by the app at runtime.
 */
export function createMemoryPersistence(
  initial: Record<string, string> = {},
): Persistence {
  const map = new Map<string, string>(Object.entries(initial));
  return {
    getItem: (key) => Promise.resolve(map.get(key) ?? null),
    setItem: (key, value) => {
      map.set(key, value);
      return Promise.resolve();
    },
    removeItem: (key) => {
      map.delete(key);
      return Promise.resolve();
    },
  };
}
