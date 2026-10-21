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
  selectIsReconnecting,
  selectShowWorkspace,
  selectIsDisconnected,
  SAVED_SERVERS_KEY,
  FAST_POLL_MS,
  SLOW_POLL_MS,
  type ConnectionState,
  type ConnectionDeps,
  type AppLifecycleState,
} from './connectionStore';

export {
  useWorkspaceStore,
  workspaceStore,
  configureWorkspaceStore,
  WORKSPACE_POLL_MS,
  type WorkspaceState,
  type TerminalViewMode,
  type WorkspaceDeps,
} from './workspaceStore';

export {
  useSessionStore,
  sessionStore,
  configureSessionStore,
  findRestoreServer,
  restoreForServer,
  SESSION_MEMORY_KEY,
  type SessionState,
  type SessionMemory,
  type ServerSession,
} from './sessionStore';

export {
  useClaudeStore,
  configureClaudeStore,
  type ClaudeState,
  type ClaudeDialogAnswer,
} from './claudeStore';

export {
  useClaudeDialogsStore,
  configureClaudeDialogsStore,
  type ClaudeDialogsState,
  type ClaudeDialogsDeps,
} from './claudeDialogsStore';

export {
  asyncStoragePersistence,
  createMemoryPersistence,
  type Persistence,
} from './persistence';

export {
  useSettingsStore,
  settingsStore,
  configureSettingsStore,
  normalizeFontSize,
  SETTINGS_KEY,
  DEFAULT_FONT_SIZE,
  MIN_FONT_SIZE,
  MAX_FONT_SIZE,
  FONT_SIZE_STEP,
  type SettingsState,
  type SettingsDeps,
} from './settingsStore';
