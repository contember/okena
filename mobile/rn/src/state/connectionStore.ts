/**
 * connectionStore.ts — connection lifecycle + saved-server list (zustand).
 *
 * Ports `mobile/lib/src/providers/connection_provider.dart` (a polling
 * `ChangeNotifier`) to a zustand store. Responsibilities:
 *   - the persisted list of {@link SavedServer}s (add / remove / update),
 *   - the current connection (`connId`, `activeServer`) and its `status`,
 *   - the connect → pair → disconnect lifecycle,
 *   - `secondsSinceActivity` (staleness indicator),
 *   - status polling: ~500ms while connecting/pairing/reconnecting, ~2s once
 *     connected (matching the Dart `_startPolling(fast:)` cadence),
 *   - app lifecycle: tells the transport when the app goes to the background
 *     (pause reconnects unless a foreground service is running) and returns
 *     (retry now), and "Retry now",
 *   - persisting the auth token and its certificate pin back onto the saved
 *     server once paired.
 *
 * Dependencies — the native module and persistence — are INJECTED via
 * {@link configureConnectionStore}, mirroring `TerminalView`'s `native` prop.
 * Until `ubrn` generates the real module, `getOkenaNative()` throws; the store
 * is constructed lazily so merely importing this module never calls it. Tests
 * inject a mock `OkenaNative` + an in-memory `Persistence`.
 */

import {
  connectionService,
  type ConnectionService,
} from '../native/connectionService';

import { create, type StoreApi, type UseBoundStore } from 'zustand';

import type { ConnectionStatus, ConnId, OkenaNative } from '../native/okena';
import { getOkenaNative } from '../native/okena';
import {
  createSavedServer,
  listFromJson,
  listToJson,
  savedServerEquals,
  withSavedServer,
  type SavedServer,
} from '../models/savedServer';
import { asyncStoragePersistence, type Persistence } from './persistence';

/** AsyncStorage key for the persisted server list (matches Dart `_kSavedServersKey`). */
export const SAVED_SERVERS_KEY = 'saved_servers';

/** Fast poll interval (ms) while connecting / pairing — Dart used 500ms. */
export const FAST_POLL_MS = 500;
/** Slow poll interval (ms) once connected — Dart used 2000ms. */
export const SLOW_POLL_MS = 2000;

/** Initial, disconnected status. */
const DISCONNECTED: ConnectionStatus = { kind: 'disconnected' };

/**
 * The slice of the native surface this store calls. Narrowed from
 * {@link OkenaNative} so a test stub only has to provide these.
 */
export type ConnectionNative = Pick<
  OkenaNative,
  | 'initApp'
  | 'connect'
  | 'getToken'
  | 'getCertFingerprint'
  | 'canReportCertFingerprint'
  | 'pair'
  | 'disconnect'
  | 'connectionStatus'
  | 'secondsSinceActivity'
  | 'onAppForeground'
  | 'onAppBackground'
  | 'reconnectNow'
  | 'canReconnectNow'
>;

/** The app lifecycle states the store reacts to (RN `AppStateStatus`). */
export type AppLifecycleState =
  | 'active'
  | 'background'
  | 'inactive'
  | 'unknown'
  | 'extension';

/** Dependencies the store calls out to. Overridable for tests. */
export interface ConnectionDeps {
  native: ConnectionNative;
  persistence: Persistence;
  service: ConnectionService;
}

/**
 * The connection store's state + actions. Screen agents read fields with the
 * `useConnectionStore(selector)` hook and call the action methods.
 */
export interface ConnectionState {
  // ── state ────────────────────────────────────────────────────────────────
  /** All saved servers (persisted). */
  servers: SavedServer[];
  /** The server currently being connected to / paired / connected, if any. */
  activeServer: SavedServer | null;
  /** The live connection id from `connect`, if any. */
  connId: ConnId | null;
  /** Current connection status (polled from the native module). */
  status: ConnectionStatus;
  /** Seconds since the last WS activity; large when disconnected. */
  secondsSinceActivity: number;
  /** Whether the persisted server list has finished loading. */
  loaded: boolean;
  /**
   * Whether the current connection reached `connected` at least once. A drop
   * after that keeps the user in the workspace with a reconnect banner instead
   * of sending them back to the pairing screen.
   */
  hasConnected: boolean;
  /**
   * `Date.now()` when the current (re)connect attempt began, or null while
   * connected / idle. Drives the elapsed-time and "Retry now" UI.
   */
  connectingSince: number | null;

  // ── derived helpers (cheap, recomputed by callers via selectors) ─────────
  // (Booleans like isConnected are intentionally NOT stored; derive from
  //  `status.kind` in the component or via the exported selectors below.)

  // ── actions ──────────────────────────────────────────────────────────────
  /**
   * One-time native init (`OkenaNative.initApp()`). Call once at app start
   * (App.tsx) before connecting. Routed through the store so the configured
   * (possibly-mocked) native module is used. No-op-safe to call once.
   */
  initApp(): void;
  /**
   * Load the persisted server list from storage. Call once at app start (e.g.
   * in `App.tsx`). Safe to call again; it just re-reads. Mirrors the Dart
   * `_loadServers()` invoked from the provider constructor.
   */
  loadServers(): Promise<void>;
  /** Add a server (deduped by host+port) and persist. Mirrors `addServer`. */
  addServer(server: SavedServer): void;
  /** Remove a server (by host+port) and persist. Mirrors `removeServer`. */
  removeServer(server: SavedServer): void;
  /**
   * Replace an existing server (matched by host+port) with `updated` and
   * persist. Used by the add/edit sheet; not in the Dart original but the
   * contract ("add/remove/update") asks for it.
   */
  updateServer(updated: SavedServer): void;
  /**
   * Begin connecting to `server`. Disconnects any current connection first,
   * sets status to `connecting`, and starts fast polling. Mirrors `connectTo`.
   */
  connectTo(server: SavedServer): void;
  /**
   * Pair the current connection with a code (async; awaits the server). On
   * failure sets status to `error`. Mirrors `pair`.
   */
  pair(code: string): Promise<void>;
  /** Tear down the current connection and reset to disconnected. Mirrors `disconnect`. */
  disconnect(): void;
  /**
   * Retry the current connection now with a fresh attempt budget, keeping
   * terminals and cached state. Falls back to a fresh connect when the
   * binding predates `reconnect_now`.
   */
  retryNow(): void;
  /**
   * Feed RN `AppState` changes in. Without a running service, `background` pauses reconnects;
   * returning to `active` from the background retries immediately.
   */
  onAppStateChange(next: AppLifecycleState): void;
}

/**
 * Lazily-resolved deps. Resolving is deferred until the first action that needs
 * the native module / storage, so importing this file (and constructing the
 * store) never throws via `getOkenaNative()`.
 */
let injectedDeps: Partial<ConnectionDeps> = {};
let resolvedDeps: ConnectionDeps | null = null;

function deps(): ConnectionDeps {
  if (!resolvedDeps) {
    resolvedDeps = {
      native: injectedDeps.native ?? getOkenaNative(),
      persistence: injectedDeps.persistence ?? asyncStoragePersistence,
      service: injectedDeps.service ?? connectionService,
    };
  }
  return resolvedDeps;
}

/**
 * Override the store's dependencies (native module + persistence). Call BEFORE
 * the first action runs — e.g. at the top of a test, or once at app start if
 * you wire a custom native module. Subsequent calls reset the resolved cache.
 */
export function configureConnectionStore(
  overrides: Partial<ConnectionDeps>,
): void {
  injectedDeps = { ...injectedDeps, ...overrides };
  resolvedDeps = null;
}

// ── polling: the store owns a single timer (start/stop) ─────────────────────

let pollTimer: ReturnType<typeof setInterval> | null = null;
let pollFast = false;
/** Last app lifecycle state seen by {@link ConnectionState.onAppStateChange}. */
let lastAppState: AppLifecycleState = 'active';

function stopPolling(): void {
  if (pollTimer !== null) {
    clearInterval(pollTimer);
    pollTimer = null;
  }
}

function startPolling(
  get: StoreApi<ConnectionState>['getState'],
  set: StoreApi<ConnectionState>['setState'],
  fast: boolean,
): void {
  stopPolling();
  pollFast = fast;
  pollTimer = setInterval(
    () => pollStatus(get, set),
    fast ? FAST_POLL_MS : SLOW_POLL_MS,
  );
}

/** A connect or reconnect is in flight. */
function isAttempting(status: ConnectionStatus): boolean {
  return status.kind === 'connecting' || status.kind === 'reconnecting';
}

/** Persist the active server list to storage. Fire-and-forget (Dart did the same). */
function persistServers(servers: readonly SavedServer[]): void {
  void deps().persistence.setItem(SAVED_SERVERS_KEY, listToJson(servers));
}

/**
 * Whether the saved credentials for `server` rest on an established server
 * identity. Without a pin the transport accepts any certificate, so replaying
 * the bearer token would hand it to whoever answers — plaintext included.
 */
export function hasEstablishedTrust(server: SavedServer): boolean {
  return server.fingerprint !== undefined;
}

/**
 * Connect using every transport-security field persisted with the server. A
 * token saved without a pin is withheld, so the connection falls back to
 * pairing and the user re-establishes trust out of band.
 *
 * Pass `enforcePin: false` when the binding cannot read pins at all: the
 * withholding would then re-pair on every launch and never record a pin.
 */
export function connectSavedServer(
  native: Pick<OkenaNative, 'connect'>,
  server: SavedServer,
  enforcePin = true,
): ConnId {
  const trusted = !enforcePin || hasEstablishedTrust(server);
  return native.connect(
    server.host,
    server.port,
    trusted ? server.token : undefined,
    server.tls,
    server.fingerprint,
  );
}

/**
 * After connecting, copy the freshly-negotiated auth token AND the certificate
 * pin it was obtained under back onto the active server, then persist. Mirrors
 * `_persistToken`.
 */
function persistCredentials(
  get: StoreApi<ConnectionState>['getState'],
  set: StoreApi<ConnectionState>['setState'],
): void {
  const { connId, activeServer } = get();
  if (!connId || !activeServer) return;
  const native = deps().native;
  const fingerprint = native.canReportCertFingerprint()
    ? native.getCertFingerprint(connId)
    : undefined;
  const updated = withSavedServer(activeServer, {
    token: native.getToken(connId),
    fingerprint,
    // A fingerprint can only come from a TLS handshake, so store the upgrade
    // with it — otherwise the next connect may still fall back to plaintext.
    tls: fingerprint === undefined ? undefined : true,
  });
  if (
    updated.token === activeServer.token &&
    updated.fingerprint === activeServer.fingerprint &&
    updated.tls === activeServer.tls
  ) {
    return;
  }
  const servers = get().servers.map((s) =>
    savedServerEquals(s, activeServer) ? updated : s,
  );
  set({ servers, activeServer: updated });
  persistServers(servers);
}

/**
 * One poll tick: refresh `status` + `secondsSinceActivity`. On the
 * connecting→connected edge, switch to slow polling and persist credentials; on
 * disconnect/error, stop polling. Mirrors `_pollStatus`.
 */
function pollStatus(
  get: StoreApi<ConnectionState>['getState'],
  set: StoreApi<ConnectionState>['setState'],
): void {
  const { connId, status: oldStatus } = get();
  if (!connId) return;

  const native = deps().native;
  const newStatus = native.connectionStatus(connId);
  const activity = native.secondsSinceActivity(connId);

  deps().service.setActive(
    newStatus.kind === 'connected' || isAttempting(newStatus),
  );
  set({ status: newStatus, secondsSinceActivity: activity });

  if (newStatus.kind === 'connected' && oldStatus.kind !== 'connected') {
    // Connected edge: slow down polling and capture the credentials.
    set({ hasConnected: true, connectingSince: null });
    startPolling(get, set, /* fast */ false);
    persistCredentials(get, set);
  }

  if (
    isAttempting(newStatus) &&
    (!isAttempting(oldStatus) || get().connectingSince === null)
  ) {
    // Dropped (or restarted): follow the reconnect closely.
    set({ connectingSince: Date.now() });
  }
  if (isAttempting(newStatus) && !pollFast) {
    startPolling(get, set, /* fast */ true);
  }

  if (newStatus.kind === 'disconnected' || newStatus.kind === 'error') {
    stopPolling();
  }
}

/**
 * The connection store hook + bound store.
 *
 * Constructing it is side-effect-free w.r.t. the native module: `create`'s
 * initializer only builds the state object + action closures; none of them call
 * `deps()` (and thus `getOkenaNative()`) until an action actually runs. So this
 * can be a normal module-level zustand store — no lazy wrapper needed.
 *
 * Use it like any zustand hook, and `.getState()` / `.subscribe()` for
 * imperative access:
 *
 * ```ts
 * const status = useConnectionStore((s) => s.status);
 * const connectTo = useConnectionStore((s) => s.connectTo);
 * const isConn = selectIsConnected(useConnectionStore.getState());
 * ```
 */
export const useConnectionStore: UseBoundStore<StoreApi<ConnectionState>> =
  create<ConnectionState>((set, get) => ({
    servers: [],
    activeServer: null,
    connId: null,
    status: DISCONNECTED,
    secondsSinceActivity: Number.MAX_SAFE_INTEGER,
    loaded: false,
    hasConnected: false,
    connectingSince: null,

    initApp() {
      deps().native.initApp();
    },

    async loadServers() {
      let servers: SavedServer[] = [];
      try {
        const json = await deps().persistence.getItem(SAVED_SERVERS_KEY);
        if (json) servers = listFromJson(json);
      } catch {
        // Corrupted data — start fresh (matches Dart's silent catch).
        servers = [];
      }
      set({ servers, loaded: true });
    },

    addServer(server) {
      const { servers } = get();
      if (servers.some((s) => savedServerEquals(s, server))) return;
      const next = [...servers, server];
      set({ servers: next });
      persistServers(next);
    },

    removeServer(server) {
      const next = get().servers.filter((s) => !savedServerEquals(s, server));
      set({ servers: next });
      persistServers(next);
    },

    updateServer(updated) {
      const next = get().servers.map((s) =>
        savedServerEquals(s, updated) ? updated : s,
      );
      set({ servers: next });
      persistServers(next);
    },

    connectTo(server) {
      const native = deps().native;
      const { connId } = get();
      // Disconnect any existing connection first (matches Dart).
      if (connId) {
        native.disconnect(connId);
        stopPolling();
      }
      deps().service.setActive(true);
      const newConnId = connectSavedServer(
        native,
        server,
        native.canReportCertFingerprint(),
      );
      set({
        activeServer: server,
        connId: newConnId,
        status: { kind: 'connecting' },
        secondsSinceActivity: Number.MAX_SAFE_INTEGER,
        hasConnected: false,
        connectingSince: Date.now(),
      });
      startPolling(get, set, /* fast */ true);
    },

    async pair(code) {
      const { connId } = get();
      if (!connId) return;
      try {
        await deps().native.pair(connId, code);
      } catch (e) {
        set({
          status: {
            kind: 'error',
            message: e instanceof Error ? e.message : String(e),
          },
        });
      }
    },

    disconnect() {
      const { connId } = get();
      if (connId) deps().native.disconnect(connId);
      stopPolling();
      deps().service.setActive(false);
      set({
        connId: null,
        activeServer: null,
        status: DISCONNECTED,
        secondsSinceActivity: Number.MAX_SAFE_INTEGER,
        hasConnected: false,
        connectingSince: null,
      });
    },

    retryNow() {
      const { connId, activeServer } = get();
      if (!connId) return;
      const native = deps().native;
      if (!native.canReconnectNow()) {
        if (activeServer) get().connectTo(activeServer);
        return;
      }
      deps().service.setActive(true);
      native.reconnectNow(connId);
      // `hasConnected` is kept, so a reconnecting user stays in the workspace.
      set({
        status: native.connectionStatus(connId),
        connectingSince: Date.now(),
      });
      startPolling(get, set, /* fast */ true);
    },

    onAppStateChange(next) {
      const previous = lastAppState;
      lastAppState = next;
      const native = deps().native;
      if (next === 'background') {
        if (!deps().service.isRunning()) native.onAppBackground();
        return;
      }
      if (next !== 'active' || previous !== 'background') return;
      native.onAppForeground();
      const { connId } = get();
      if (!connId) return;
      // The transport may have restarted an errored connect: resume polling.
      pollStatus(get, set);
      if (get().status.kind !== 'connected')
        startPolling(get, set, /* fast */ true);
    },
  }));

/**
 * Imperative store handle (`getState` / `setState` / `subscribe`) for non-React
 * consumers — e.g. {@link import('../navigation/navStore')} subscribing to
 * status changes to drive navigation. Same instance as the hook.
 */
export function connectionStore(): StoreApi<ConnectionState> {
  return useConnectionStore;
}

// ── selectors (derive the Dart `is*` getters from `status`) ─────────────────

/** True once the connection is established. Mirrors Dart `isConnected`. */
export const selectIsConnected = (s: ConnectionState): boolean =>
  s.status.kind === 'connected';
/** True while pairing. Mirrors Dart `isPairing`. */
export const selectIsPairing = (s: ConnectionState): boolean =>
  s.status.kind === 'pairing';
/** True while connecting. Mirrors Dart `isConnecting`. */
export const selectIsConnecting = (s: ConnectionState): boolean =>
  s.status.kind === 'connecting';
/**
 * True while a once-established connection is being re-established. The user
 * stays in the workspace (with a banner) during this.
 */
export const selectIsReconnecting = (s: ConnectionState): boolean =>
  s.hasConnected && isAttempting(s.status);
/** True when the workspace should be on screen: connected, or reconnecting. */
export const selectShowWorkspace = (s: ConnectionState): boolean =>
  selectIsConnected(s) || selectIsReconnecting(s);
/**
 * True when fully idle: disconnected AND no active server selected. Mirrors
 * Dart `isDisconnected` (used to decide the server-list screen is shown).
 */
export const selectIsDisconnected = (s: ConnectionState): boolean =>
  s.status.kind === 'disconnected' && s.activeServer === null;

/** Re-export the convenience constructor so callers don't reach into the model. */
export { createSavedServer };
