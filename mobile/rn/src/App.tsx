/**
 * App.tsx — the RN app root.
 *
 * Mirrors `mobile/lib/main.dart`'s `OkenaApp` + `AppRouter`, minus Flutter's
 * MaterialApp theming (RN screens style themselves from `theme.ts`):
 *   - on mount: `initApp()`, load the persisted server list + session memory,
 *     reconnect to the server the user had open last, bind connection state →
 *     navigation, bridge connection status → workspace polling, and record the
 *     selected project / terminal as it changes.
 *   - render: the screen the {@link useNavStore} currently points at.
 *
 * The store wiring is intentionally imperative (not React context) so the
 * stores stay singletons usable from anywhere — including non-React code.
 *
 * The native module is resolved lazily by the stores. To run against a mock
 * (off-device / tests), call `configureConnectionStore({ native, persistence })`
 * `configureWorkspaceStore({ native })` and `configureSessionStore({ persistence })`
 * BEFORE mounting this component.
 */

import React, { useEffect } from 'react';
import {
  AppState,
  KeyboardAvoidingView,
  Platform,
  StatusBar,
  StyleSheet,
} from 'react-native';
import { SafeAreaProvider, SafeAreaView } from 'react-native-safe-area-context';

import { useNavStore, bindConnectionToNavigation } from './navigation';
import {
  connectionStore,
  selectShowWorkspace,
  type ConnectionState,
} from './state/connectionStore';
import { workspaceStore } from './state/workspaceStore';
import { restoreForServer, sessionStore } from './state/sessionStore';
import { bindSessionMemory, loadAndRestore } from './state/sessionRestore';
import { settingsStore } from './state/settingsStore';
import { OkenaColors } from './theme';

import { ServerListScreen } from './screens/ServerListScreen';
import { PairingScreen } from './screens/PairingScreen';
import { WorkspaceScreen } from './screens/WorkspaceScreen';

/**
 * Bridge the connection store to the workspace store's polling lifecycle:
 * start polling (with the live `connId`) when connected, stop + clear when not.
 * A reconnect keeps polling: the native side keeps the last state and
 * terminals, so the workspace stays on screen behind the reconnect banner.
 * Mirrors the Dart `WorkspaceProvider._onConnectionChanged`. Polling starts
 * with the selection remembered for the server, so a launch or a reconnect
 * reopens the same project and terminal. Returns an unsubscribe fn.
 */
function bindConnectionToWorkspace(): () => void {
  const conn = connectionStore();
  const ws = workspaceStore();

  const apply = (state: ConnectionState) => {
    if (selectShowWorkspace(state) && state.connId) {
      const restore = state.activeServer
        ? restoreForServer(sessionStore().getState().memory, state.activeServer)
        : undefined;
      ws.getState().start(state.connId, restore);
    } else {
      ws.getState().stop();
    }
  };

  apply(conn.getState());
  return conn.subscribe(apply);
}

export const App: React.FC = () => {
  const screen = useNavStore((s) => s.screen);

  useEffect(() => {
    // One-time native init, routed through the store so a configured mock is
    // honored. Resolving the native module throws if `ubrn` hasn't generated it
    // yet — expected off-device; the mock path configures the stores first.
    const conn = connectionStore().getState();
    conn.initApp();
    void settingsStore().getState().load();

    const unbindNav = bindConnectionToNavigation();
    const unbindWs = bindConnectionToWorkspace();
    const unbindSession = bindSessionMemory();
    loadAndRestore().catch(() => {
      // A failed restore just leaves the user on the server list.
    });
    // Pause reconnects in the background and retry at once on return.
    const appState = AppState.addEventListener('change', (next) =>
      connectionStore().getState().onAppStateChange(next),
    );
    return () => {
      appState.remove();
      unbindNav();
      unbindWs();
      unbindSession();
    };
  }, []);

  return (
    <SafeAreaProvider>
      <StatusBar barStyle="light-content" />
      <KeyboardAvoidingView
        style={styles.root}
        behavior={Platform.OS === 'android' ? 'padding' : undefined}
      >
        <SafeAreaView style={styles.root}>
          {screen === 'workspace' ? (
            <WorkspaceScreen />
          ) : screen === 'pairing' ? (
            <PairingScreen />
          ) : (
            <ServerListScreen />
          )}
        </SafeAreaView>
      </KeyboardAvoidingView>
    </SafeAreaProvider>
  );
};

const styles = StyleSheet.create({
  root: { flex: 1, backgroundColor: OkenaColors.background },
});

export default App;
