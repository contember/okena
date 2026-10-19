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
  selectShowWorkspace,
  type ConnectionState,
} from '../state/connectionStore';

/**
 * The screens of the app, in flow order. `claude` is the Claude chat of one
 * terminal ({@link NavState.claudeTerminalId}), entered from the workspace.
 */
export type Screen = 'serverList' | 'pairing' | 'workspace' | 'claude';

/** Nav store state + the navigate action. */
export interface NavState {
  /** The currently-displayed screen. */
  screen: Screen;
  /** Terminal shown by the `claude` screen; `null` on every other screen. */
  claudeTerminalId: string | null;
  /**
   * Navigate to a screen. Screens call this directly (e.g. the server-list
   * screen calls `navigate('pairing')` after kicking off a connection — though
   * the connection-driven binding usually handles that automatically).
   */
  navigate(screen: Screen): void;
  /** Open the Claude chat of a terminal. */
  openClaude(terminalId: string): void;
}

const useNavStoreImpl: UseBoundStore<StoreApi<NavState>> = create<NavState>((set) => ({
  screen: 'serverList',
  claudeTerminalId: null,
  navigate: (screen) =>
    set((state) => ({
      screen,
      claudeTerminalId: screen === 'claude' ? state.claudeTerminalId : null,
    })),
  openClaude: (terminalId) => set({ screen: 'claude', claudeTerminalId: terminalId }),
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

/** Open the Claude chat of a terminal (for non-React callers). */
export function openClaude(terminalId: string): void {
  useNavStoreImpl.getState().openClaude(terminalId);
}

/** The current screen, imperatively. */
export function currentScreen(): Screen {
  return useNavStoreImpl.getState().screen;
}

/**
 * The screen the connection state implies — the Dart `AppRouter` rule, plus
 * staying in the workspace while a once-established connection reconnects:
 *   - connected, or reconnecting after having connected → `workspace`
 *   - an active server chosen  → `pairing` (connecting/pairing/error)
 *   - otherwise                → `serverList`
 */
export function deriveScreen(conn: ConnectionState): Screen {
  if (selectShowWorkspace(conn)) return 'workspace';
  if (conn.activeServer !== null) return 'pairing';
  return 'serverList';
}

/**
 * The screen to show after a connection change: the derived one, except that
 * a connected app stays on the Claude chat it was opened into.
 */
export function nextScreen(current: Screen, derived: Screen): Screen {
  return current === 'claude' && derived === 'workspace' ? current : derived;
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
  const apply = (state: ConnectionState) => {
    navigate(nextScreen(currentScreen(), deriveScreen(state)));
  };
  // Apply once immediately for the initial state.
  apply(conn.getState());
  return conn.subscribe(apply);
}
