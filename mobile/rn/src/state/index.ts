/**
 * state/index.ts — public surface of the state layer.
 *
 * Screen agents import the store hooks + selectors from here.
 */

export {
  useConnectionStore,
  connectionStore,
  configureConnectionStore,
  createSavedServer,
  selectIsConnected,
  selectIsPairing,
  selectIsConnecting,
  selectIsDisconnected,
  SAVED_SERVERS_KEY,
  FAST_POLL_MS,
  SLOW_POLL_MS,
  type ConnectionState,
  type ConnectionDeps,
} from './connectionStore';

export {
  useWorkspaceStore,
  workspaceStore,
  configureWorkspaceStore,
  WORKSPACE_POLL_MS,
  type WorkspaceState,
  type WorkspaceDeps,
} from './workspaceStore';

export {
  asyncStoragePersistence,
  createMemoryPersistence,
  type Persistence,
} from './persistence';
