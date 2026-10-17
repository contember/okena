/// <reference types="jest" />
// tsconfig pins `types` to react/react-native, so pull jest globals in here.

/**
 * Launch restore + selection recording glue between the session memory and the
 * connection / workspace stores.
 */

import { createSavedServer, listToJson } from '../models/savedServer';
import {
  configureConnectionStore,
  SAVED_SERVERS_KEY,
  useConnectionStore,
  type ConnectionNative,
} from './connectionStore';
import { createMemoryPersistence, type Persistence } from './persistence';
import { bindSessionMemory, loadAndRestore } from './sessionRestore';
import {
  configureSessionStore,
  EMPTY_SESSION_MEMORY,
  parseSessionMemory,
  SESSION_MEMORY_KEY,
  useSessionStore,
  type SessionMemory,
} from './sessionStore';
import { useWorkspaceStore } from './workspaceStore';

const SERVER = createSavedServer({ host: 'mac.lan', port: 19100, token: 'tok', fingerprint: 'fp' });

function stubNative(connect: jest.Mock): ConnectionNative {
  return {
    initApp: () => {},
    connect,
    getToken: () => 'tok',
    getCertFingerprint: () => 'fp',
    canReportCertFingerprint: () => true,
    pair: () => Promise.resolve(),
    disconnect: () => {},
    connectionStatus: () => ({ kind: 'connecting' }),
    secondsSinceActivity: () => 0,
  };
}

function setup(memory: SessionMemory): { persistence: Persistence; connect: jest.Mock } {
  const persistence = createMemoryPersistence({
    [SAVED_SERVERS_KEY]: listToJson([SERVER]),
    [SESSION_MEMORY_KEY]: JSON.stringify(memory),
  });
  const connect = jest.fn(() => 'conn-1');
  configureConnectionStore({ native: stubNative(connect), persistence });
  configureSessionStore({ persistence });
  return { persistence, connect };
}

describe('loadAndRestore', () => {
  beforeEach(() => {
    jest.useFakeTimers();
    useSessionStore.setState({ memory: EMPTY_SESSION_MEMORY, loaded: false });
  });

  afterEach(() => {
    useConnectionStore.getState().disconnect();
    jest.useRealTimers();
  });

  it('reconnects to the server that was open last', async () => {
    const { connect } = setup({ last: { host: 'mac.lan', port: 19100 }, servers: {} });
    await loadAndRestore();

    expect(connect).toHaveBeenCalledWith('mac.lan', 19100, 'tok', true, 'fp');
    expect(useConnectionStore.getState().activeServer?.host).toBe('mac.lan');
  });

  it('stays on the server list without a reconnect target', async () => {
    const { connect } = setup(EMPTY_SESSION_MEMORY);
    await loadAndRestore();

    expect(connect).not.toHaveBeenCalled();
    expect(useConnectionStore.getState().activeServer).toBeNull();
  });
});

describe('bindSessionMemory', () => {
  let unbind: () => void = () => {};

  afterEach(() => {
    unbind();
    useConnectionStore.setState({ activeServer: null, connId: null, status: { kind: 'disconnected' } });
    useWorkspaceStore.setState({ selectedProjectId: null, selectedTerminalId: null, terminalByProject: {} });
  });

  it('records selections while connected and forgets the target on disconnect', async () => {
    const { persistence } = setup(EMPTY_SESSION_MEMORY);
    await useSessionStore.getState().load();
    unbind = bindSessionMemory();

    // Not connected yet: nothing is recorded.
    useWorkspaceStore.setState({ selectedProjectId: 'p1', selectedTerminalId: 't1' });
    expect(await persistence.getItem(SESSION_MEMORY_KEY)).toBe(JSON.stringify(EMPTY_SESSION_MEMORY));

    useConnectionStore.setState({ activeServer: SERVER, connId: 'conn-1', status: { kind: 'connected' } });
    useWorkspaceStore.setState({ selectedTerminalId: 't2', terminalByProject: { p1: 't2' } });
    let stored = parseSessionMemory(await persistence.getItem(SESSION_MEMORY_KEY));
    expect(stored).toEqual({
      last: { host: 'mac.lan', port: 19100 },
      servers: { 'mac.lan:19100': { projectId: 'p1', terminalByProject: { p1: 't2' } } },
    });

    useConnectionStore.setState({ activeServer: null, connId: null, status: { kind: 'disconnected' } });
    stored = parseSessionMemory(await persistence.getItem(SESSION_MEMORY_KEY));
    expect(stored.last).toBeNull();
    expect(stored.servers['mac.lan:19100']?.projectId).toBe('p1');
  });

  it('keeps the reconnect target when the connection drops', async () => {
    const { persistence } = setup({ last: { host: 'mac.lan', port: 19100 }, servers: {} });
    await useSessionStore.getState().load();
    useConnectionStore.setState({ activeServer: SERVER, connId: 'conn-1', status: { kind: 'connected' } });
    unbind = bindSessionMemory();

    useConnectionStore.setState({ status: { kind: 'connecting' } });
    const stored = parseSessionMemory(await persistence.getItem(SESSION_MEMORY_KEY));
    expect(stored.last).toEqual({ host: 'mac.lan', port: 19100 });
  });
});
