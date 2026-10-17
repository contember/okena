/**
 * sessionStore.ts — what the user had open last (zustand, persisted).
 *
 * Remembers, per saved server, the last selected project and the selected
 * terminal of every project visited, plus which server to reconnect to on the
 * next launch. App.tsx wires it:
 *   - on launch: {@link SessionState.load}, then reconnect to
 *     {@link findRestoreServer} if there is one;
 *   - on connect: hand {@link restoreForServer} to the workspace store so it
 *     reopens the remembered project + terminal;
 *   - while connected: {@link SessionState.recordSelection} on every selection
 *     change;
 *   - on an explicit disconnect: {@link SessionState.clearAutoReconnect}, so a
 *     user who left a server is not dropped back into it on the next launch.
 *
 * Persistence is injected via {@link configureSessionStore}, same as the
 * connection store.
 */

import { create, type StoreApi, type UseBoundStore } from 'zustand';

import type { ProjectId, TerminalId } from '../native/okena';
import type { SavedServer } from '../models/savedServer';
import type { SelectionMemory } from '../models/terminalSelection';
import { asyncStoragePersistence, type Persistence } from './persistence';

/** AsyncStorage key for the persisted session memory. */
export const SESSION_MEMORY_KEY = 'session_memory';

/**
 * What is remembered for one server: the project open last and the terminal
 * selected in each visited project.
 */
export type ServerSession = SelectionMemory;

/** The whole persisted memory. */
export interface SessionMemory {
  /** Server (by host + port) to reconnect to on launch, or `null`. */
  last: { host: string; port: number } | null;
  /** Per-server memory keyed by {@link serverKey}. */
  servers: Record<string, ServerSession>;
}

export const EMPTY_SESSION_MEMORY: SessionMemory = { last: null, servers: {} };

/** Identity key for a server — host + port, like `savedServerEquals`. */
export function serverKey(server: Pick<SavedServer, 'host' | 'port'>): string {
  return `${server.host}:${server.port}`;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function parseServerSession(raw: unknown): ServerSession | null {
  if (!isRecord(raw)) return null;
  const terminalByProject: Record<ProjectId, TerminalId> = {};
  if (isRecord(raw.terminalByProject)) {
    for (const [projectId, terminalId] of Object.entries(raw.terminalByProject)) {
      if (typeof terminalId === 'string') terminalByProject[projectId] = terminalId;
    }
  }
  return {
    projectId: typeof raw.projectId === 'string' ? raw.projectId : null,
    terminalByProject,
  };
}

/**
 * Parse persisted memory, dropping anything malformed. Corrupt JSON yields the
 * empty memory, so a bad write can never block app start.
 */
export function parseSessionMemory(json: string | null): SessionMemory {
  if (!json) return EMPTY_SESSION_MEMORY;
  let raw: unknown;
  try {
    raw = JSON.parse(json);
  } catch {
    return EMPTY_SESSION_MEMORY;
  }
  if (!isRecord(raw)) return EMPTY_SESSION_MEMORY;

  const lastRaw = raw.last;
  const last =
    isRecord(lastRaw) && typeof lastRaw.host === 'string' && typeof lastRaw.port === 'number'
      ? { host: lastRaw.host, port: lastRaw.port }
      : null;

  const servers: Record<string, ServerSession> = {};
  if (isRecord(raw.servers)) {
    for (const [key, value] of Object.entries(raw.servers)) {
      const session = parseServerSession(value);
      if (session) servers[key] = session;
    }
  }
  return { last, servers };
}

/**
 * The saved server to reconnect to on launch: the last one, provided it is
 * still saved and already paired (has a token). Without a token a reconnect
 * would only land on the pairing screen, which is no restore at all.
 */
export function findRestoreServer(
  servers: readonly SavedServer[],
  memory: SessionMemory,
): SavedServer | null {
  const last = memory.last;
  if (!last) return null;
  const server = servers.find((s) => s.host === last.host && s.port === last.port);
  return server?.token ? server : null;
}

/** The remembered selection for `server` (empty when there is none). */
export function restoreForServer(
  memory: SessionMemory,
  server: Pick<SavedServer, 'host' | 'port'>,
): ServerSession {
  return memory.servers[serverKey(server)] ?? { projectId: null, terminalByProject: {} };
}

/** Dependencies the store calls out to. Overridable for tests. */
export interface SessionDeps {
  persistence: Persistence;
}

export interface SessionState {
  memory: SessionMemory;
  /** Whether {@link load} has finished. */
  loaded: boolean;
  /** Read the persisted memory. Call once at app start. */
  load(): Promise<void>;
  /**
   * Remember the current selection on `server` and mark it as the server to
   * reconnect to. A no-op when nothing changed, so it is cheap to call from a
   * store subscription.
   */
  recordSelection(server: SavedServer, session: ServerSession): void;
  /** Forget which server to reconnect to (per-server memory is kept). */
  clearAutoReconnect(): void;
}

let injectedDeps: Partial<SessionDeps> = {};
let resolvedDeps: SessionDeps | null = null;

function deps(): SessionDeps {
  if (!resolvedDeps) {
    resolvedDeps = { persistence: injectedDeps.persistence ?? asyncStoragePersistence };
  }
  return resolvedDeps;
}

/** Override the store's persistence. Call before the first action runs. */
export function configureSessionStore(overrides: Partial<SessionDeps>): void {
  injectedDeps = { ...injectedDeps, ...overrides };
  resolvedDeps = null;
}

function persist(memory: SessionMemory): void {
  void deps().persistence.setItem(SESSION_MEMORY_KEY, JSON.stringify(memory));
}

function sameSession(a: ServerSession | undefined, b: ServerSession): boolean {
  if (!a || a.projectId !== b.projectId) return false;
  const aKeys = Object.keys(a.terminalByProject);
  if (aKeys.length !== Object.keys(b.terminalByProject).length) return false;
  return aKeys.every((k) => a.terminalByProject[k] === b.terminalByProject[k]);
}

export const useSessionStore: UseBoundStore<StoreApi<SessionState>> = create<SessionState>(
  (set, get) => ({
    memory: EMPTY_SESSION_MEMORY,
    loaded: false,

    async load() {
      let memory = EMPTY_SESSION_MEMORY;
      try {
        memory = parseSessionMemory(await deps().persistence.getItem(SESSION_MEMORY_KEY));
      } catch {
        // Storage unavailable: start with nothing remembered.
      }
      set({ memory, loaded: true });
    },

    recordSelection(server, session) {
      const { memory } = get();
      const key = serverKey(server);
      const isLast =
        memory.last !== null && memory.last.host === server.host && memory.last.port === server.port;
      if (isLast && sameSession(memory.servers[key], session)) return;
      const next: SessionMemory = {
        last: { host: server.host, port: server.port },
        servers: {
          ...memory.servers,
          [key]: { projectId: session.projectId, terminalByProject: { ...session.terminalByProject } },
        },
      };
      set({ memory: next });
      persist(next);
    },

    clearAutoReconnect() {
      const { memory } = get();
      if (memory.last === null) return;
      const next: SessionMemory = { ...memory, last: null };
      set({ memory: next });
      persist(next);
    },
  }),
);

/** Imperative store handle for non-React consumers (App.tsx wiring). */
export function sessionStore(): StoreApi<SessionState> {
  return useSessionStore;
}
