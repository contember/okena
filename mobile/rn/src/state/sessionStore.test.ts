/// <reference types="jest" />
// tsconfig pins `types` to react/react-native, so pull jest globals in here.

import { createSavedServer } from '../models/savedServer';
import { createMemoryPersistence } from './persistence';
import {
  configureSessionStore,
  EMPTY_SESSION_MEMORY,
  findRestoreServer,
  parseSessionMemory,
  restoreForServer,
  SESSION_MEMORY_KEY,
  useSessionStore,
  type SessionMemory,
} from './sessionStore';

const PAIRED = createSavedServer({ host: 'mac.lan', port: 19100, token: 'tok', fingerprint: 'fp' });
const UNPAIRED = createSavedServer({ host: 'other.lan', port: 19100 });

function memoryWithLast(host: string, port: number): SessionMemory {
  return { last: { host, port }, servers: {} };
}

describe('parseSessionMemory', () => {
  it('round-trips a valid memory', () => {
    const memory: SessionMemory = {
      last: { host: 'mac.lan', port: 19100 },
      servers: { 'mac.lan:19100': { projectId: 'p1', terminalByProject: { p1: 't2' } } },
    };
    expect(parseSessionMemory(JSON.stringify(memory))).toEqual(memory);
  });

  it('yields the empty memory for missing or corrupt data', () => {
    expect(parseSessionMemory(null)).toEqual(EMPTY_SESSION_MEMORY);
    expect(parseSessionMemory('{not json')).toEqual(EMPTY_SESSION_MEMORY);
    expect(parseSessionMemory('[1,2]')).toEqual(EMPTY_SESSION_MEMORY);
  });

  it('drops malformed fields but keeps the valid ones', () => {
    const parsed = parseSessionMemory(
      JSON.stringify({
        last: { host: 'mac.lan', port: '19100' },
        servers: {
          a: { projectId: 7, terminalByProject: { p1: 't1', p2: 3 } },
          b: 'nope',
        },
      }),
    );
    expect(parsed).toEqual({
      last: null,
      servers: { a: { projectId: null, terminalByProject: { p1: 't1' } } },
    });
  });
});

describe('findRestoreServer', () => {
  it('returns the last server when it is still saved and paired', () => {
    expect(findRestoreServer([UNPAIRED, PAIRED], memoryWithLast('mac.lan', 19100))).toBe(PAIRED);
  });

  it('skips a server that was removed or never paired', () => {
    expect(findRestoreServer([UNPAIRED], memoryWithLast('mac.lan', 19100))).toBeNull();
    expect(findRestoreServer([UNPAIRED], memoryWithLast('other.lan', 19100))).toBeNull();
    expect(findRestoreServer([PAIRED], EMPTY_SESSION_MEMORY)).toBeNull();
  });
});

describe('restoreForServer', () => {
  it('returns an empty selection for an unknown server', () => {
    expect(restoreForServer(EMPTY_SESSION_MEMORY, PAIRED)).toEqual({
      projectId: null,
      terminalByProject: {},
    });
  });
});

describe('session store', () => {
  it('persists the selection and the reconnect target', async () => {
    const persistence = createMemoryPersistence();
    configureSessionStore({ persistence });
    await useSessionStore.getState().load();

    useSessionStore
      .getState()
      .recordSelection(PAIRED, { projectId: 'p1', terminalByProject: { p1: 't2' } });

    const stored = parseSessionMemory(await persistence.getItem(SESSION_MEMORY_KEY));
    expect(stored.last).toEqual({ host: 'mac.lan', port: 19100 });
    expect(restoreForServer(stored, PAIRED)).toEqual({
      projectId: 'p1',
      terminalByProject: { p1: 't2' },
    });

    // A fresh load reads it back.
    useSessionStore.setState({ memory: EMPTY_SESSION_MEMORY, loaded: false });
    await useSessionStore.getState().load();
    expect(useSessionStore.getState().memory).toEqual(stored);
  });

  it('skips writes when nothing changed', async () => {
    const persistence = createMemoryPersistence();
    const setItem = jest.spyOn(persistence, 'setItem');
    configureSessionStore({ persistence });
    await useSessionStore.getState().load();

    const session = { projectId: 'p1', terminalByProject: { p1: 't1' } };
    useSessionStore.getState().recordSelection(PAIRED, session);
    useSessionStore.getState().recordSelection(PAIRED, { ...session, terminalByProject: { p1: 't1' } });
    expect(setItem).toHaveBeenCalledTimes(1);
  });

  it('forgets the reconnect target but keeps per-server memory', async () => {
    const persistence = createMemoryPersistence();
    configureSessionStore({ persistence });
    await useSessionStore.getState().load();

    useSessionStore
      .getState()
      .recordSelection(PAIRED, { projectId: 'p1', terminalByProject: { p1: 't1' } });
    useSessionStore.getState().clearAutoReconnect();

    const stored = parseSessionMemory(await persistence.getItem(SESSION_MEMORY_KEY));
    expect(stored.last).toBeNull();
    expect(restoreForServer(stored, PAIRED).projectId).toBe('p1');
  });
});
