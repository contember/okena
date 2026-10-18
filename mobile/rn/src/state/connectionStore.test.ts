/// <reference types="jest" />
// tsconfig pins `types` to react/react-native, so pull jest globals in here.

/**
 * Connection-store credential-trust behaviour (SEC-9), plus a guard that the
 * poll loop survives a slow handshake. (CORR-13 itself is fixed and tested in
 * Rust — `ConnectionManager::connect`; these cases only cover the JS side.)
 */

import { createSavedServer, listFromJson, type SavedServer } from '../models/savedServer';
import type { ConnectionStatus } from '../native/okena';
import { createMemoryPersistence, type Persistence } from './persistence';
import {
  configureConnectionStore,
  connectSavedServer,
  FAST_POLL_MS,
  SAVED_SERVERS_KEY,
  SLOW_POLL_MS,
  selectIsReconnecting,
  useConnectionStore,
  type ConnectionNative,
} from './connectionStore';
import { deriveScreen } from '../navigation/navStore';

function stubNative(overrides: Partial<ConnectionNative> = {}): ConnectionNative {
  return {
    initApp: () => {},
    connect: () => 'conn-1',
    getToken: () => undefined,
    getCertFingerprint: () => undefined,
    canReportCertFingerprint: () => true,
    pair: () => Promise.resolve(),
    disconnect: () => {},
    connectionStatus: () => ({ kind: 'disconnected' }),
    secondsSinceActivity: () => 0,
    onAppForeground: () => {},
    onAppBackground: () => {},
    reconnectNow: () => {},
    canReconnectNow: () => true,
    ...overrides,
  };
}

/** A native whose status walks `script`, holding on the last entry. */
function scriptedStatus(script: ConnectionStatus[]): () => ConnectionStatus {
  let tick = 0;
  return () => script[Math.min(tick++, script.length - 1)]!;
}

function setup(native: ConnectionNative): Persistence {
  const persistence = createMemoryPersistence();
  configureConnectionStore({ native, persistence });
  return persistence;
}

async function persistedServers(persistence: Persistence): Promise<SavedServer[]> {
  const json = await persistence.getItem(SAVED_SERVERS_KEY);
  return json === null ? [] : listFromJson(json);
}

describe('connectSavedServer', () => {
  it('withholds a saved token when trust was never established', () => {
    const connect = jest.fn(() => 'conn-1');
    const server = createSavedServer({
      host: 'okena.lan',
      port: 19100,
      token: 'saved-token',
      tls: true,
    });

    connectSavedServer({ connect }, server);
    expect(connect).toHaveBeenCalledWith('okena.lan', 19100, undefined, true, undefined);
  });

  it('withholds a saved token on a plaintext server too', () => {
    const connect = jest.fn(() => 'conn-1');
    const server = createSavedServer({
      host: 'okena.lan',
      port: 19100,
      token: 'saved-token',
      tls: false,
    });

    connectSavedServer({ connect }, server);
    expect(connect).toHaveBeenCalledWith('okena.lan', 19100, undefined, false, undefined);
  });

  it('replays an unpinned token when the binding cannot read pins', () => {
    const connect = jest.fn(() => 'conn-1');
    const server = createSavedServer({
      host: 'okena.lan',
      port: 19100,
      token: 'saved-token',
      tls: true,
    });

    connectSavedServer({ connect }, server, /* enforcePin */ false);
    expect(connect).toHaveBeenCalledWith('okena.lan', 19100, 'saved-token', true, undefined);
  });

  it('replays a saved token once the certificate is pinned', () => {
    const connect = jest.fn(() => 'conn-1');
    const server = createSavedServer({
      host: 'okena.lan',
      port: 19100,
      token: 'saved-token',
      tls: true,
      fingerprint: 'fp-abc',
    });

    connectSavedServer({ connect }, server);
    expect(connect).toHaveBeenCalledWith('okena.lan', 19100, 'saved-token', true, 'fp-abc');
  });
});

describe('connection lifecycle', () => {
  beforeEach(() => {
    jest.useFakeTimers();
    // The store is a module singleton; drop what earlier cases left behind.
    useConnectionStore.setState({ servers: [], activeServer: null, connId: null });
  });

  afterEach(() => {
    useConnectionStore.getState().disconnect();
    jest.useRealTimers();
  });

  it('keeps polling across ticks that still report connecting', () => {
    setup(
      stubNative({
        connectionStatus: scriptedStatus([
          { kind: 'connecting' },
          { kind: 'connecting' },
          { kind: 'connected' },
        ]),
      }),
    );
    const store = useConnectionStore.getState();
    store.addServer(createSavedServer({ host: 'okena.lan', port: 19100 }));
    store.connectTo(createSavedServer({ host: 'okena.lan', port: 19100 }));

    jest.advanceTimersByTime(FAST_POLL_MS * 2);
    expect(useConnectionStore.getState().status).toEqual({ kind: 'connecting' });

    jest.advanceTimersByTime(FAST_POLL_MS);
    expect(useConnectionStore.getState().status).toEqual({ kind: 'connected' });
  });

  it('persists the certificate pin alongside the token it was obtained under', async () => {
    const persistence = setup(
      stubNative({
        connectionStatus: scriptedStatus([{ kind: 'connecting' }, { kind: 'connected' }]),
        getToken: () => 'fresh-token',
        getCertFingerprint: () => 'fp-abc',
      }),
    );
    const server = createSavedServer({ host: 'okena.lan', port: 19100, tls: true });
    const store = useConnectionStore.getState();
    store.addServer(server);
    store.connectTo(server);

    jest.advanceTimersByTime(FAST_POLL_MS * 2);

    expect(useConnectionStore.getState().activeServer).toMatchObject({
      token: 'fresh-token',
      fingerprint: 'fp-abc',
      tls: true,
    });
    expect(await persistedServers(persistence)).toEqual([
      expect.objectContaining({ token: 'fresh-token', fingerprint: 'fp-abc', tls: true }),
    ]);
  });

  it('records the TLS upgrade a pin implies for a server saved as plaintext', async () => {
    const persistence = setup(
      stubNative({
        connectionStatus: scriptedStatus([{ kind: 'connecting' }, { kind: 'connected' }]),
        getToken: () => 'fresh-token',
        getCertFingerprint: () => 'fp-abc',
      }),
    );
    const server = createSavedServer({ host: 'okena.lan', port: 19100, tls: false });
    const store = useConnectionStore.getState();
    store.addServer(server);
    store.connectTo(server);

    jest.advanceTimersByTime(FAST_POLL_MS * 2);

    expect(await persistedServers(persistence)).toEqual([
      expect.objectContaining({ tls: true, fingerprint: 'fp-abc' }),
    ]);
  });

  it('never re-pairs in a loop when the binding cannot read pins', async () => {
    const persistence = setup(
      stubNative({
        connectionStatus: scriptedStatus([{ kind: 'connecting' }, { kind: 'connected' }]),
        getToken: () => 'saved-token',
        canReportCertFingerprint: () => false,
        getCertFingerprint: () => {
          throw new Error('a stale binding has no getCertFingerprint');
        },
      }),
    );
    const server = createSavedServer({
      host: 'okena.lan',
      port: 19100,
      token: 'saved-token',
      tls: true,
    });
    const store = useConnectionStore.getState();
    store.addServer(server);
    store.connectTo(server);

    // The token is still sent, so the server never bounces back to pairing.
    expect(useConnectionStore.getState().connId).not.toBeNull();
    jest.advanceTimersByTime(FAST_POLL_MS * 2);
    expect(useConnectionStore.getState().status).toEqual({ kind: 'connected' });
    expect(await persistedServers(persistence)).toEqual([
      expect.objectContaining({ token: 'saved-token', fingerprint: undefined }),
    ]);
  });

  it('leaves the saved server alone when nothing was negotiated', async () => {
    const persistence = setup(
      stubNative({
        connectionStatus: scriptedStatus([{ kind: 'connecting' }, { kind: 'connected' }]),
      }),
    );
    const server = createSavedServer({
      host: 'okena.lan',
      port: 19100,
      token: 'saved-token',
      tls: true,
      fingerprint: 'fp-abc',
    });
    const store = useConnectionStore.getState();
    store.addServer(server);
    store.connectTo(server);

    jest.advanceTimersByTime(FAST_POLL_MS * 2);

    expect(await persistedServers(persistence)).toEqual([
      expect.objectContaining({ token: 'saved-token', fingerprint: 'fp-abc' }),
    ]);
  });
});

describe('reconnect handling', () => {
  const server = createSavedServer({ host: 'okena.lan', port: 19100 });

  beforeEach(() => {
    jest.useFakeTimers();
    useConnectionStore.setState({ servers: [], activeServer: null, connId: null });
  });

  afterEach(() => {
    useConnectionStore.getState().onAppStateChange('active');
    useConnectionStore.getState().disconnect();
    jest.useRealTimers();
  });

  it('keeps a once-connected user in the workspace while reconnecting', () => {
    setup(
      stubNative({
        connectionStatus: scriptedStatus([
          { kind: 'connected' },
          { kind: 'reconnecting', attempt: 1 },
        ]),
      }),
    );
    useConnectionStore.getState().connectTo(server);

    jest.advanceTimersByTime(FAST_POLL_MS);
    expect(deriveScreen(useConnectionStore.getState())).toBe('workspace');

    // Connected polls slowly; the drop shows up on the next slow tick.
    jest.advanceTimersByTime(SLOW_POLL_MS);
    const state = useConnectionStore.getState();
    expect(state.status).toEqual({ kind: 'reconnecting', attempt: 1 });
    expect(selectIsReconnecting(state)).toBe(true);
    expect(state.connectingSince).not.toBeNull();
    expect(deriveScreen(state)).toBe('workspace');
  });

  it('switches back to fast polling after a drop', () => {
    const connectionStatus = jest.fn(
      scriptedStatus([{ kind: 'connected' }, { kind: 'reconnecting', attempt: 1 }]),
    );
    setup(stubNative({ connectionStatus }));
    useConnectionStore.getState().connectTo(server);
    jest.advanceTimersByTime(FAST_POLL_MS + SLOW_POLL_MS);

    const before = connectionStatus.mock.calls.length;
    jest.advanceTimersByTime(FAST_POLL_MS * 2);
    expect(connectionStatus.mock.calls.length - before).toBe(2);
  });

  it('shows the pairing screen for a first connect that has not succeeded yet', () => {
    setup(stubNative({ connectionStatus: () => ({ kind: 'reconnecting', attempt: 2 }) }));
    useConnectionStore.getState().connectTo(server);
    jest.advanceTimersByTime(FAST_POLL_MS);
    expect(deriveScreen(useConnectionStore.getState())).toBe('pairing');
  });

  it('leaves the workspace when reconnecting gives up', () => {
    setup(
      stubNative({
        connectionStatus: scriptedStatus([
          { kind: 'connected' },
          { kind: 'error', message: 'Connection lost after 8 attempts' },
        ]),
      }),
    );
    useConnectionStore.getState().connectTo(server);
    jest.advanceTimersByTime(FAST_POLL_MS + SLOW_POLL_MS);
    expect(deriveScreen(useConnectionStore.getState())).toBe('pairing');
  });

  it('retries in place through reconnectNow and stays in the workspace', () => {
    const reconnectNow = jest.fn();
    setup(
      stubNative({
        reconnectNow,
        connectionStatus: scriptedStatus([
          { kind: 'connected' },
          { kind: 'reconnecting', attempt: 3 },
        ]),
      }),
    );
    useConnectionStore.getState().connectTo(server);
    jest.advanceTimersByTime(FAST_POLL_MS + SLOW_POLL_MS);

    useConnectionStore.getState().retryNow();
    expect(reconnectNow).toHaveBeenCalledWith('conn-1');
    expect(deriveScreen(useConnectionStore.getState())).toBe('workspace');
  });

  it('falls back to a fresh connect when the binding has no reconnectNow', () => {
    const connect = jest.fn(() => 'conn-2');
    setup(
      stubNative({
        connect,
        canReconnectNow: () => false,
        connectionStatus: () => ({ kind: 'connecting' }),
      }),
    );
    useConnectionStore.getState().connectTo(server);
    useConnectionStore.getState().retryNow();
    expect(connect).toHaveBeenCalledTimes(2);
    expect(useConnectionStore.getState().connId).toBe('conn-2');
  });

  it('pauses in the background and retries on return to the foreground', () => {
    const onAppBackground = jest.fn();
    const onAppForeground = jest.fn();
    setup(stubNative({ onAppBackground, onAppForeground }));
    const store = useConnectionStore.getState();

    store.onAppStateChange('inactive');
    store.onAppStateChange('active');
    expect(onAppForeground).not.toHaveBeenCalled();

    store.onAppStateChange('background');
    expect(onAppBackground).toHaveBeenCalledTimes(1);
    store.onAppStateChange('active');
    expect(onAppForeground).toHaveBeenCalledTimes(1);
  });

  it('resumes polling on foreground when the transport restarted an errored connect', () => {
    const connectionStatus = jest.fn(
      scriptedStatus([
        { kind: 'error', message: 'boom' },
        { kind: 'connecting' },
        { kind: 'connecting' },
      ]),
    );
    setup(stubNative({ connectionStatus }));
    useConnectionStore.getState().connectTo(server);
    jest.advanceTimersByTime(FAST_POLL_MS);
    expect(useConnectionStore.getState().status.kind).toBe('error');

    useConnectionStore.getState().onAppStateChange('background');
    useConnectionStore.getState().onAppStateChange('active');
    expect(useConnectionStore.getState().status.kind).toBe('connecting');
    const before = connectionStatus.mock.calls.length;
    jest.advanceTimersByTime(FAST_POLL_MS);
    expect(connectionStatus.mock.calls.length).toBe(before + 1);
  });
});
