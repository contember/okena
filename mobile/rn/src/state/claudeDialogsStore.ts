/**
 * claudeDialogsStore.ts — the open Claude dialogs of every terminal that has
 * one, for the dashboard (zustand).
 *
 * The state snapshot only counts dialogs per terminal. While open
 * ({@link ClaudeDialogsState.open}) this store fetches the session of each
 * terminal with a dialog every {@link CLAUDE_POLL_MS} to read what the
 * dialogs ask. A poll starts only after the previous one finished. A terminal
 * whose fetch fails keeps its last dialogs; a terminal without dialogs is
 * dropped. Answering stays with {@link useClaudeStore}, which follows one
 * terminal.
 *
 * The native module is injected via {@link configureClaudeDialogsStore}, like
 * the other stores.
 */

import { create, type StoreApi, type UseBoundStore } from 'zustand';

import { parseClaudeSession, type ApiClaudeDialog } from '../native/claude';
import type { ConnId, OkenaNative, TerminalId } from '../native/okena';
import { getOkenaNative } from '../native/okena';
import { CLAUDE_POLL_MS } from './claudeStore';
import { useWorkspaceStore } from './workspaceStore';

export type ClaudeDialogsNative = Pick<OkenaNative, 'claudeGetSession'>;

export interface ClaudeDialogsDeps {
  native: ClaudeDialogsNative;
  /** Terminals with an open dialog, from the state snapshot. */
  dialogTerminals: () => TerminalId[];
}

export interface ClaudeDialogsState {
  /** Open dialogs per terminal; terminals without one are absent. */
  dialogs: Readonly<Record<TerminalId, ApiClaudeDialog[]>>;
  /** Start polling. Re-opening the same connection is a no-op. */
  open(connId: ConnId): void;
  /** Stop polling and clear everything. */
  close(): void;
}

let injectedDeps: Partial<ClaudeDialogsDeps> = {};
let resolvedDeps: ClaudeDialogsDeps | null = null;

function deps(): ClaudeDialogsDeps {
  if (!resolvedDeps) {
    resolvedDeps = {
      native: injectedDeps.native ?? getOkenaNative(),
      dialogTerminals:
        injectedDeps.dialogTerminals ??
        (() => Object.keys(useWorkspaceStore.getState().claudeDialogs)),
    };
  }
  return resolvedDeps;
}

export function configureClaudeDialogsStore(
  overrides: Partial<ClaudeDialogsDeps>,
): void {
  injectedDeps = { ...injectedDeps, ...overrides };
  resolvedDeps = null;
}

/** Bumped by open/close; work started under an older generation is dropped. */
let generation = 0;
let activeConnId: ConnId | null = null;
let pollTimer: ReturnType<typeof setTimeout> | null = null;

function stopPolling(): void {
  generation += 1;
  activeConnId = null;
  if (pollTimer !== null) {
    clearTimeout(pollTimer);
    pollTimer = null;
  }
}

async function poll(
  pollGeneration: number,
  connId: ConnId,
  get: StoreApi<ClaudeDialogsState>['getState'],
  set: StoreApi<ClaudeDialogsState>['setState'],
): Promise<void> {
  const terminalIds = deps().dialogTerminals();
  const results = await Promise.allSettled(
    terminalIds.map(async (terminalId) =>
      parseClaudeSession(await deps().native.claudeGetSession(connId, terminalId)),
    ),
  );
  if (pollGeneration !== generation) return;
  const previous = get().dialogs;
  const next: Record<TerminalId, ApiClaudeDialog[]> = {};
  terminalIds.forEach((terminalId, i) => {
    const result = results[i]!;
    const dialogs =
      result.status === 'fulfilled' ? result.value.dialogs : previous[terminalId];
    if (dialogs && dialogs.length > 0) next[terminalId] = dialogs;
  });
  // The parsed records are built field by field in a fixed order, so their JSON compares structure.
  if (JSON.stringify(next) !== JSON.stringify(previous)) set({ dialogs: next });
}

function schedulePoll(
  pollGeneration: number,
  connId: ConnId,
  delayMs: number,
  get: StoreApi<ClaudeDialogsState>['getState'],
  set: StoreApi<ClaudeDialogsState>['setState'],
): void {
  pollTimer = setTimeout(() => {
    pollTimer = null;
    void poll(pollGeneration, connId, get, set).finally(() => {
      if (pollGeneration === generation)
        schedulePoll(pollGeneration, connId, CLAUDE_POLL_MS, get, set);
    });
  }, delayMs);
}

export const useClaudeDialogsStore: UseBoundStore<StoreApi<ClaudeDialogsState>> =
  create<ClaudeDialogsState>((set, get) => ({
    dialogs: {},

    open(connId) {
      if (activeConnId === connId) return;
      stopPolling();
      activeConnId = connId;
      set({ dialogs: {} });
      schedulePoll(generation, connId, 0, get, set);
    },

    close() {
      stopPolling();
      set({ dialogs: {} });
    },
  }));
