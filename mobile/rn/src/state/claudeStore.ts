/**
 * claudeStore.ts — the Claude session of one terminal (zustand).
 *
 * While a terminal is open ({@link ClaudeState.open}) the store polls the
 * session and then the transcript every {@link CLAUDE_POLL_MS}. A poll starts
 * only after the previous one finished, so slow responses never pile up. The
 * session is published as soon as it arrives, so a failing transcript never
 * freezes the dialogs. Transcript pages are appended, or replace everything on
 * `reset`; the page's `cursor` and `session_id` are sent back on the next poll.
 * Polls that change nothing leave the state objects untouched.
 *
 * The subagent list is refetched when the transcript shows an `Agent` call or
 * the session a running subagent the list does not have yet. While a subagent
 * is open ({@link ClaudeState.openSubagent}) its transcript is polled too.
 *
 * Results of a poll or an answer that finish after {@link ClaudeState.close}
 * (or after another terminal was opened) are dropped. Per-dialog state (the
 * hidden answered dialogs, {@link ClaudeState.dialogAnswers}) is trimmed after
 * every session poll to the dialogs the server still lists.
 *
 * The native module is injected via {@link configureClaudeStore}, like the
 * other stores.
 */

import { create, type StoreApi, type UseBoundStore } from 'zustand';

import {
  parseClaudePromptResponse,
  parseClaudeSession,
  parseClaudeSubagentList,
  parseClaudeTranscriptPage,
  type ApiClaudeDecision,
  type ApiClaudeEntry,
  type ApiClaudePromptDelivery,
  type ApiClaudeSession,
  type ApiClaudeSubagent,
  type ApiClaudeTranscriptPage,
} from '../native/claude';
import type { ConnId, OkenaNative, TerminalId } from '../native/okena';
import {
  CLAUDE_DIALOG_GONE_MESSAGE,
  getOkenaNative,
  nativeErrorMessage,
} from '../native/okena';

/** Delay (ms) between the end of one poll and the start of the next. */
export const CLAUDE_POLL_MS = 1000;

/** Transcript entries kept in memory; older ones are dropped. */
export const CLAUDE_MAX_ENTRIES = 1000;

/** Least time (ms) between two subagent list fetches. */
export const CLAUDE_SUBAGENTS_REFRESH_MS = 3000;

/** Tools that start a subagent. */
export const CLAUDE_AGENT_TOOLS: ReadonlySet<string> = new Set([
  'Agent',
  'Task',
]);

/** The slice of the native surface this store calls. */
export type ClaudeNative = Pick<
  OkenaNative,
  | 'claudeGetSession'
  | 'claudeGetTranscript'
  | 'claudeGetSubagents'
  | 'claudeSendPrompt'
  | 'claudeAnswerDialog'
  | 'sendSpecialKey'
>;

export interface ClaudeDeps {
  native: ClaudeNative;
}

/** An answer to a dialog that is on its way, or why the last one failed. */
export type ClaudeDialogAnswer =
  | { status: 'sending' }
  | { status: 'failed'; error: string };

export interface ClaudeState {
  connId: ConnId | null;
  terminalId: TerminalId | null;
  /** Latest session; `null` until the first poll succeeds. */
  session: ApiClaudeSession | null;
  /** Transcript entries of `sessionId`, oldest first. */
  entries: ApiClaudeEntry[];
  /** Cursor of the last transcript page; 0 asks for the first page. */
  cursor: number;
  sessionId: string | undefined;
  /** Error of the last session fetch; cleared by the next successful one. */
  sessionError: string | null;
  /** Error of the last transcript fetch; cleared by the next successful one. */
  transcriptError: string | null;
  sending: boolean;
  /** How the last prompt was delivered. */
  lastDelivery: ApiClaudePromptDelivery | null;
  /** Why the last prompt (or interrupt) was not accepted. */
  promptError: string | null;
  /** The session's subagents, oldest first; empty for a server without them. */
  subagents: ApiClaudeSubagent[];
  /** The subagent whose transcript is shown, or `null`. */
  subagent: ClaudeSubagentView | null;
  /**
   * Answers per listed dialog id; a failure other than "already answered"
   * stays until the next answer or until the dialog is gone.
   */
  dialogAnswers: Readonly<Record<string, ClaudeDialogAnswer>>;

  /** Start following a terminal. Re-opening the same terminal is a no-op. */
  open(connId: ConnId, terminalId: TerminalId): void;
  /** Stop polling and clear everything. */
  close(): void;
  /** Send a prompt; resolves to whether the server accepted it. */
  sendPrompt(text: string): Promise<boolean>;
  /**
   * Answer a dialog; the dialog leaves the session once answered or gone.
   * Ignored while an answer to the same dialog is still sending.
   */
  answerDialog(dialogId: string, decision: ApiClaudeDecision): Promise<void>;
  /** Press Escape in the terminal; a failure lands in `promptError`. */
  interrupt(): Promise<void>;
  /** Show a subagent's transcript; polled with the session. */
  openSubagent(agentId: string): void;
  closeSubagent(): void;
}

export interface ClaudeSubagentView {
  agentId: string;
  /** Oldest first. */
  entries: ApiClaudeEntry[];
  cursor: number;
  sessionId: string | undefined;
  error: string | null;
}

let injectedDeps: Partial<ClaudeDeps> = {};
let resolvedDeps: ClaudeDeps | null = null;

function deps(): ClaudeDeps {
  if (!resolvedDeps) {
    resolvedDeps = { native: injectedDeps.native ?? getOkenaNative() };
  }
  return resolvedDeps;
}

export function configureClaudeStore(overrides: Partial<ClaudeDeps>): void {
  injectedDeps = { ...injectedDeps, ...overrides };
  resolvedDeps = null;
}

const CLOSED = {
  connId: null,
  terminalId: null,
  session: null,
  entries: [],
  cursor: 0,
  sessionId: undefined,
  sessionError: null,
  transcriptError: null,
  sending: false,
  lastDelivery: null,
  promptError: null,
  dialogAnswers: {},
  subagents: [],
  subagent: null,
} satisfies Partial<ClaudeState>;

// ── polling: one chain per open terminal ────────────────────────────────────

/** Bumped by open/close; work started under an older generation is dropped. */
let generation = 0;
let pollTimer: ReturnType<typeof setTimeout> | null = null;
/** Dialogs answered (or found gone) in this generation, hidden until the server drops them. */
let answeredDialogIds = new Set<string>();
/** When the subagent list was last requested (ms since epoch). */
let subagentsFetchedAt = 0;

function stopPolling(): void {
  generation += 1;
  if (pollTimer !== null) {
    clearTimeout(pollTimer);
    pollTimer = null;
  }
  answeredDialogIds = new Set();
  subagentsFetchedAt = 0;
}

/** Whether the transcript or the session name a subagent the list lacks. */
function subagentsOutdated(
  subagents: readonly ApiClaudeSubagent[],
  entries: readonly ApiClaudeEntry[],
  session: ApiClaudeSession,
): boolean {
  const agentIds = new Set(subagents.map((subagent) => subagent.agent_id));
  const toolUseIds = new Set(subagents.map((subagent) => subagent.tool_use_id));
  return (
    session.running_subagents.some(
      (running) => !agentIds.has(running.agent_id),
    ) ||
    entries.some(
      (entry) =>
        entry.kind === 'tool_use' &&
        CLAUDE_AGENT_TOOLS.has(entry.name) &&
        !toolUseIds.has(entry.tool_use_id),
    )
  );
}

async function pollSubagents(
  pollGeneration: number,
  session: ApiClaudeSession,
  get: StoreApi<ClaudeState>['getState'],
  set: StoreApi<ClaudeState>['setState'],
): Promise<void> {
  const { connId, terminalId, subagents, entries } = get();
  if (connId === null || terminalId === null) return;
  const now = Date.now();
  if (
    !subagentsOutdated(subagents, entries, session) ||
    now - subagentsFetchedAt < CLAUDE_SUBAGENTS_REFRESH_MS
  )
    return;
  subagentsFetchedAt = now;
  try {
    const list = parseClaudeSubagentList(
      await deps().native.claudeGetSubagents(connId, terminalId),
    );
    if (pollGeneration === generation) set({ subagents: list.subagents });
  } catch {
    // An older server has no subagent list; the chat then shows plain tool rows.
  }
}

async function pollSubagentTranscript(
  pollGeneration: number,
  get: StoreApi<ClaudeState>['getState'],
  set: StoreApi<ClaudeState>['setState'],
): Promise<void> {
  const { connId, terminalId, subagent } = get();
  if (connId === null || terminalId === null || subagent === null) return;
  const { agentId } = subagent;
  const stillOpen = () =>
    pollGeneration === generation && get().subagent?.agentId === agentId;
  try {
    const page = parseClaudeTranscriptPage(
      await deps().native.claudeGetTranscript(
        connId,
        terminalId,
        BigInt(subagent.cursor),
        subagent.sessionId,
        agentId,
      ),
    );
    if (!stillOpen()) return;
    const current = get().subagent;
    if (current === null) return;
    const entries = nextEntries(current.entries, page);
    if (
      entries !== current.entries ||
      page.cursor !== current.cursor ||
      page.session_id !== current.sessionId ||
      current.error !== null
    ) {
      set({
        subagent: {
          agentId,
          entries,
          cursor: page.cursor,
          sessionId: page.session_id,
          error: null,
        },
      });
    }
  } catch (error) {
    const current = get().subagent;
    if (stillOpen() && current !== null)
      set({ subagent: { ...current, error: nativeErrorMessage(error) } });
  }
}

/** `answers` without the dialogs the server no longer lists; the same object when nothing goes. */
function listedAnswers(
  answers: ClaudeState['dialogAnswers'],
  listedIds: ReadonlySet<string>,
): ClaudeState['dialogAnswers'] {
  const ids = Object.keys(answers);
  if (ids.every((id) => listedIds.has(id))) return answers;
  return Object.fromEntries(
    Object.entries(answers).filter(([id]) => listedIds.has(id)),
  );
}

function withAnswer(
  answers: ClaudeState['dialogAnswers'],
  dialogId: string,
  answer: ClaudeDialogAnswer | null,
): ClaudeState['dialogAnswers'] {
  if (answer !== null) return { ...answers, [dialogId]: answer };
  return Object.fromEntries(
    Object.entries(answers).filter(([id]) => id !== dialogId),
  );
}

function withoutAnswered(session: ApiClaudeSession): ApiClaudeSession {
  if (answeredDialogIds.size === 0) return session;
  return {
    ...session,
    dialogs: session.dialogs.filter(
      (dialog) => !answeredDialogIds.has(dialog.id),
    ),
  };
}

function keepLatest(entries: ApiClaudeEntry[]): ApiClaudeEntry[] {
  return entries.length > CLAUDE_MAX_ENTRIES
    ? entries.slice(-CLAUDE_MAX_ENTRIES)
    : entries;
}

/** The parsed records are built field by field in a fixed order, so their JSON compares structure. */
function sameSession(a: ApiClaudeSession | null, b: ApiClaudeSession): boolean {
  return a !== null && JSON.stringify(a) === JSON.stringify(b);
}

function nextEntries(
  previous: ApiClaudeEntry[],
  page: ApiClaudeTranscriptPage,
): ApiClaudeEntry[] {
  if (page.entries.length === 0)
    return page.reset && previous.length > 0 ? [] : previous;
  return keepLatest(page.reset ? page.entries : [...previous, ...page.entries]);
}

async function poll(
  pollGeneration: number,
  get: StoreApi<ClaudeState>['getState'],
  set: StoreApi<ClaudeState>['setState'],
): Promise<void> {
  const { connId, terminalId } = get();
  if (connId === null || terminalId === null) return;
  const native = deps().native;

  let listed: ApiClaudeSession;
  try {
    listed = parseClaudeSession(
      await native.claudeGetSession(connId, terminalId),
    );
  } catch (error) {
    if (pollGeneration === generation)
      set({ sessionError: nativeErrorMessage(error) });
    return;
  }
  if (pollGeneration !== generation) return;
  const listedIds = new Set(listed.dialogs.map((dialog) => dialog.id));
  answeredDialogIds = new Set(
    [...answeredDialogIds].filter((id) => listedIds.has(id)),
  );
  const session = withoutAnswered(listed);
  const previous = get();
  const dialogAnswers = listedAnswers(previous.dialogAnswers, listedIds);
  if (
    !sameSession(previous.session, session) ||
    previous.sessionError !== null ||
    dialogAnswers !== previous.dialogAnswers
  ) {
    set({
      session: sameSession(previous.session, session)
        ? previous.session
        : session,
      sessionError: null,
      dialogAnswers,
    });
  }

  if (!session.has_transcript) {
    if (
      previous.entries.length > 0 ||
      previous.cursor !== 0 ||
      previous.sessionId !== undefined
    ) {
      set({ entries: [], cursor: 0, sessionId: undefined });
    }
    return;
  }

  try {
    const page = parseClaudeTranscriptPage(
      await native.claudeGetTranscript(
        connId,
        terminalId,
        BigInt(previous.cursor),
        previous.sessionId,
        undefined,
      ),
    );
    if (pollGeneration !== generation) return;
    const current = get();
    const entries = nextEntries(current.entries, page);
    if (
      entries !== current.entries ||
      page.cursor !== current.cursor ||
      page.session_id !== current.sessionId ||
      current.transcriptError !== null
    ) {
      set({
        entries,
        cursor: page.cursor,
        sessionId: page.session_id,
        transcriptError: null,
      });
    }
  } catch (error) {
    if (pollGeneration === generation)
      set({ transcriptError: nativeErrorMessage(error) });
  }

  if (pollGeneration !== generation) return;
  // Both swallow their own failures: they must not fail the poll.
  await Promise.all([
    pollSubagents(pollGeneration, session, get, set),
    pollSubagentTranscript(pollGeneration, get, set),
  ]);
}

function schedulePoll(
  pollGeneration: number,
  delayMs: number,
  get: StoreApi<ClaudeState>['getState'],
  set: StoreApi<ClaudeState>['setState'],
): void {
  pollTimer = setTimeout(() => {
    pollTimer = null;
    void poll(pollGeneration, get, set).finally(() => {
      if (pollGeneration === generation)
        schedulePoll(pollGeneration, CLAUDE_POLL_MS, get, set);
    });
  }, delayMs);
}

export const useClaudeStore: UseBoundStore<StoreApi<ClaudeState>> =
  create<ClaudeState>((set, get) => ({
    ...CLOSED,

    open(connId, terminalId) {
      const current = get();
      if (current.connId === connId && current.terminalId === terminalId)
        return;
      stopPolling();
      set({ ...CLOSED, connId, terminalId });
      schedulePoll(generation, 0, get, set);
    },

    close() {
      stopPolling();
      set(CLOSED);
    },

    async sendPrompt(text) {
      const { connId, terminalId } = get();
      if (connId === null || terminalId === null || text.trim().length === 0)
        return false;
      const sendGeneration = generation;
      set({ sending: true, promptError: null, lastDelivery: null });
      try {
        const response = parseClaudePromptResponse(
          await deps().native.claudeSendPrompt(connId, terminalId, text),
        );
        if (sendGeneration === generation)
          set({ sending: false, lastDelivery: response.delivery });
        return true;
      } catch (error) {
        if (sendGeneration === generation) {
          set({ sending: false, promptError: nativeErrorMessage(error) });
        }
        return false;
      }
    },

    async answerDialog(dialogId, decision) {
      const { connId, terminalId, dialogAnswers } = get();
      if (connId === null || terminalId === null) return;
      if (dialogAnswers[dialogId]?.status === 'sending') return;
      const answerGeneration = generation;
      set({
        dialogAnswers: withAnswer(dialogAnswers, dialogId, {
          status: 'sending',
        }),
      });
      let failure: string | null = null;
      try {
        await deps().native.claudeAnswerDialog(
          connId,
          terminalId,
          dialogId,
          JSON.stringify(decision),
        );
      } catch (error) {
        const message = nativeErrorMessage(error);
        if (message !== CLAUDE_DIALOG_GONE_MESSAGE) failure = message;
      }
      if (answerGeneration !== generation) return;
      const current = get();
      if (failure !== null) {
        set({
          dialogAnswers: withAnswer(current.dialogAnswers, dialogId, {
            status: 'failed',
            error: failure,
          }),
        });
        return;
      }
      answeredDialogIds.add(dialogId);
      set({
        session:
          current.session === null ? null : withoutAnswered(current.session),
        dialogAnswers: withAnswer(current.dialogAnswers, dialogId, null),
      });
    },

    openSubagent(agentId) {
      if (get().subagent?.agentId === agentId) return;
      set({
        subagent: {
          agentId,
          entries: [],
          cursor: 0,
          sessionId: undefined,
          error: null,
        },
      });
      // Load it now rather than at the next poll.
      void pollSubagentTranscript(generation, get, set);
    },

    closeSubagent() {
      set({ subagent: null });
    },

    async interrupt() {
      const { connId, terminalId } = get();
      if (connId === null || terminalId === null) return;
      try {
        await deps().native.sendSpecialKey(connId, terminalId, 'Escape');
      } catch (error) {
        set({ promptError: nativeErrorMessage(error) });
      }
    },
  }));
