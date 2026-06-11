/**
 * App.tsx — the RN app root.
 *
 * Mirrors `mobile/lib/main.dart`'s `OkenaApp` + `AppRouter`, minus Flutter's
 * MaterialApp theming (RN screens style themselves from `theme.ts`):
 *   - on mount: `initApp()`, load the persisted server list, bind connection
 *     state → navigation, and bridge connection status → workspace polling.
 *   - render: the screen the {@link useNavStore} currently points at.
 *
 * The store wiring is intentionally imperative (not React context) so the
 * stores stay singletons usable from anywhere — including non-React code.
 *
 * The native module is resolved lazily by the stores. To run against a mock
 * (off-device / tests), call `configureConnectionStore({ native, persistence })`
 * and `configureWorkspaceStore({ native })` BEFORE mounting this component.
 */

import React, { useEffect } from 'react';
import { View, StyleSheet } from 'react-native';

import { useNavStore, bindConnectionToNavigation } from './navigation';
import {
  connectionStore,
  selectIsConnected,
  type ConnectionState,
} from './state/connectionStore';
import { workspaceStore } from './state/workspaceStore';
import { OkenaColors } from './theme';

import { ServerListScreen } from './screens/ServerListScreen';
import { PairingScreen } from './screens/PairingScreen';
import { WorkspaceScreen } from './screens/WorkspaceScreen';

/**
 * Bridge the connection store to the workspace store's polling lifecycle:
 * start polling (with the live `connId`) when connected, stop + clear when not.
 * Mirrors the Dart `WorkspaceProvider._onConnectionChanged`. Returns an
 * unsubscribe fn.
 */
function bindConnectionToWorkspace(): () => void {
  const conn = connectionStore();
  const ws = workspaceStore();

  const apply = (state: ConnectionState) => {
    if (selectIsConnected(state) && state.connId) {
      ws.getState().start(state.connId);
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
    void conn.loadServers();

    const unbindNav = bindConnectionToNavigation();
    const unbindWs = bindConnectionToWorkspace();
    return () => {
      unbindNav();
      unbindWs();
    };
  }, []);

  return (
    <View style={styles.root}>
      {screen === 'workspace' ? (
        <WorkspaceScreen />
      ) : screen === 'pairing' ? (
        <PairingScreen />
      ) : (
        <ServerListScreen />
      )}
    </View>
  );
};

const styles = StyleSheet.create({
  root: { flex: 1, backgroundColor: OkenaColors.background },
});

export default App;
