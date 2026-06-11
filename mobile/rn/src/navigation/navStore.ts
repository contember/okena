/**
 * navStore.ts — the minimal state-driven router.
 *
 * The Flutter app used a declarative `AppRouter` widget that picked a screen
 * from `ConnectionProvider`'s state (`isConnected` → workspace,
 * `activeServer != null` → pairing, else server list) inside an
 * `AnimatedSwitcher`. We deliberately do NOT pull in `react-navigation`
 * (it needs `react-native-screens` / `react-native-gesture-handler` native
 * deps that can't build in this environment — see the task constraints).
 *
 * Instead this is a tiny zustand store holding the current {@link Screen}, plus
 * {@link navigate} to set it. App.tsx renders the matching screen and also
 * SUBSCRIBES to the connection store to drive automatic transitions (the same
 * rule the Dart `AppRouter` encoded) — see {@link deriveScreen} and
 * {@link bindConnectionToNavigation}.
 */

import { create, type StoreApi, type UseBoundStore } from 'zustand';

import {
  connectionStore,
  selectIsConnected,
  type ConnectionState,
} from '../state/connectionStore';

/** The three screens of the app, in flow order. */
export type Screen = 'serverList' | 'pairing' | 'workspace';

/** Nav store state + the navigate action. */
export interface NavState {
  /** The currently-displayed screen. */
  screen: Screen;
  /**
   * Navigate to a screen. Screens call this directly (e.g. the server-list
   * screen calls `navigate('pairing')` after kicking off a connection — though
   * the connection-driven binding usually handles that automatically).
   */
  navigate(screen: Screen): void;
}

const useNavStoreImpl: UseBoundStore<StoreApi<NavState>> = create<NavState>((set) => ({
  screen: 'serverList',
  navigate: (screen) => set({ screen }),
}));

/**
 * The navigation store hook. Use it to read the active screen and to navigate:
 *
 * ```ts
 * const screen = useNavStore((s) => s.screen);
 * const navigate = useNavStore((s) => s.navigate);
 * ```
 */
export const useNavStore = useNavStoreImpl;

/** Imperative navigate (for non-React callers). Prefer the hook inside components. */
export function navigate(screen: Screen): void {
  useNavStoreImpl.getState().navigate(screen);
}

/** The current screen, imperatively. */
export function currentScreen(): Screen {
  return useNavStoreImpl.getState().screen;
}

/**
 * The screen the connection state implies — the exact rule the Dart `AppRouter`
 * used:
 *   - connected               → `workspace`
 *   - an active server chosen  → `pairing` (connecting/pairing/error)
 *   - otherwise                → `serverList`
 */
export function deriveScreen(conn: ConnectionState): Screen {
  if (selectIsConnected(conn)) return 'workspace';
  if (conn.activeServer !== null) return 'pairing';
  return 'serverList';
}

/**
 * Subscribe the navigation store to the connection store so the screen tracks
 * connection state automatically (the Dart `AppRouter` behavior). Call once at
 * app start (App.tsx). Returns an unsubscribe fn.
 *
 * Screens may still call {@link navigate} directly for in-flow moves; this just
 * guarantees the canonical transitions (e.g. → workspace on Connected, back to
 * serverList on full disconnect) happen no matter who initiated them.
 */
export function bindConnectionToNavigation(): () => void {
  const conn = connectionStore();
  // Apply once immediately for the initial state.
  navigate(deriveScreen(conn.getState()));
  return conn.subscribe((state) => {
    navigate(deriveScreen(state));
  });
}
