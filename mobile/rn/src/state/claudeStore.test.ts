/// <reference types="jest" />
// tsconfig pins `types` to react/react-native, so pull jest globals in here.

import type {
  ApiClaudeEntry,
  ApiClaudeSession,
  ApiClaudeTranscriptPage,
} from '../native/claude';
import { CLAUDE_DIALOG_GONE_MESSAGE } from '../native/okena';
import {
  CLAUDE_MAX_ENTRIES,
  CLAUDE_POLL_MS,
  configureClaudeStore,
  useClaudeStore,
  type ClaudeNative,
} from './claudeStore';

const DIALOG = {
  id: 'd1',
  tool_name: 'Bash',
  tool_input: { command: 'ls' },
  permission_suggestions: [],
  created_at_ms: 1,
};

function session(overrides: Partial<ApiClaudeSession> = {}): ApiClaudeSession {
  return {
    terminal_id: 't1',
    session_id: 's1',
    has_transcript: true,
    hooks_attached: true,
    channel_attached: false,
    busy: false,
    queued_prompts: 0,
    dialogs: [],
    ...overrides,
  };
}

function assistant(uuid: string): ApiClaudeEntry {
  return { kind: 'assistant', uuid, text: uuid };
}

function page(
  overrides: Partial<ApiClaudeTranscriptPage>,
): ApiClaudeTranscriptPage {
  return {
    session_id: 's1',
    entries: [],
    cursor: 0,
    reset: false,
    ...overrides,
  };
}

interface Deferred<T> {
  promise: Promise<T>;
  resolve: (value: T) => void;
}

function deferred<T>(): Deferred<T> {
  let resolve: (value: T) => void = () => {};
  const promise = new Promise<T>((r) => {
    resolve = r;
  });
  return { promise, resolve };
}

/** A native stub serving the given pages in order and recording transcript calls. */
function stubNative(
  pages: ApiClaudeTranscriptPage[],
  sessionValue = session(),
) {
  const transcriptCalls: Array<{
    cursor: bigint;
    sessionId: string | undefined;
  }> = [];
  const native: ClaudeNative = {
    claudeGetSession: jest.fn(async () => JSON.stringify(sessionValue)),
    claudeGetTranscript: jest.fn(
      async (_conn, _terminal, cursor, sessionId) => {
        transcriptCalls.push({ cursor, sessionId });
        const next =
          pages[Math.min(transcriptCalls.length - 1, pages.length - 1)]!;
        return JSON.stringify(next);
      },
    ),
    claudeSendPrompt: jest.fn(async () =>
      JSON.stringify({ delivery: 'channel' }),
    ),
    claudeAnswerDialog: jest.fn(async () => {}),
    sendSpecialKey: jest.fn(async () => {}),
  };
  return { native, transcriptCalls };
}

describe('claude store', () => {
  beforeEach(() => {
    jest.useFakeTimers();
  });

  afterEach(() => {
    useClaudeStore.getState().close();
    jest.useRealTimers();
  });

  it('appends pages, replaces on reset and sends back cursor + session', async () => {
    const { native, transcriptCalls } = stubNative([
      page({ entries: [assistant('a')], cursor: 10, reset: true }),
      page({ entries: [assistant('b')], cursor: 20 }),
      page({
        session_id: 's2',
        entries: [assistant('c')],
        cursor: 5,
        reset: true,
      }),
    ]);
    configureClaudeStore({ native });

    useClaudeStore.getState().open('conn', 't1');
    await jest.advanceTimersByTimeAsync(0);
    expect(useClaudeStore.getState().entries.map((e) => e.uuid)).toEqual(['a']);

    await jest.advanceTimersByTimeAsync(CLAUDE_POLL_MS);
    expect(useClaudeStore.getState().entries.map((e) => e.uuid)).toEqual([
      'a',
      'b',
    ]);

    await jest.advanceTimersByTimeAsync(CLAUDE_POLL_MS);
    expect(useClaudeStore.getState().entries.map((e) => e.uuid)).toEqual(['c']);
    expect(useClaudeStore.getState().sessionId).toBe('s2');

    expect(transcriptCalls).toEqual([
      { cursor: 0n, sessionId: undefined },
      { cursor: 10n, sessionId: 's1' },
      { cursor: 20n, sessionId: 's1' },
    ]);
  });

  it('does not start a poll while the previous one is still running', async () => {
    const pending = deferred<string>();
    const { native } = stubNative([page({ reset: true })]);
    const getSession = jest.fn(() => pending.promise);
    configureClaudeStore({
      native: { ...native, claudeGetSession: getSession },
    });

    useClaudeStore.getState().open('conn', 't1');
    await jest.advanceTimersByTimeAsync(10 * CLAUDE_POLL_MS);
    expect(getSession).toHaveBeenCalledTimes(1);

    pending.resolve(JSON.stringify(session()));
    await jest.advanceTimersByTimeAsync(0);
    expect(getSession).toHaveBeenCalledTimes(1);
    await jest.advanceTimersByTimeAsync(CLAUDE_POLL_MS);
    expect(getSession).toHaveBeenCalledTimes(2);
  });

  it('stops polling on close and drops results that arrive afterwards', async () => {
    const pending = deferred<string>();
    const { native } = stubNative([page({ reset: true })]);
    const getSession = jest.fn(() => pending.promise);
    configureClaudeStore({
      native: { ...native, claudeGetSession: getSession },
    });

    useClaudeStore.getState().open('conn', 't1');
    await jest.advanceTimersByTimeAsync(0);
    useClaudeStore.getState().close();
    pending.resolve(JSON.stringify(session({ dialogs: [DIALOG] })));
    await jest.advanceTimersByTimeAsync(5 * CLAUDE_POLL_MS);

    expect(useClaudeStore.getState().session).toBeNull();
    expect(getSession).toHaveBeenCalledTimes(1);
  });

  it('skips the transcript when the pane has none', async () => {
    const { native } = stubNative(
      [page({ reset: true })],
      session({ has_transcript: false }),
    );
    configureClaudeStore({ native });

    useClaudeStore.getState().open('conn', 't1');
    await jest.advanceTimersByTimeAsync(0);

    expect(useClaudeStore.getState().session?.has_transcript).toBe(false);
    expect(native.claudeGetTranscript).not.toHaveBeenCalled();
  });

  it('reports a malformed response as a poll error', async () => {
    const { native } = stubNative([page({ reset: true })]);
    configureClaudeStore({
      native: { ...native, claudeGetSession: async () => '{"terminal_id":1}' },
    });

    useClaudeStore.getState().open('conn', 't1');
    await jest.advanceTimersByTimeAsync(0);

    expect(useClaudeStore.getState().sessionError).toBe(
      'session.terminal_id: expected a string',
    );
  });

  it('updates the session even when the transcript fetch fails', async () => {
    const { native } = stubNative([page({ reset: true })]);
    const getSession = jest
      .fn()
      .mockResolvedValueOnce(JSON.stringify(session()))
      .mockResolvedValue(JSON.stringify(session({ dialogs: [DIALOG] })));
    const getTranscript = jest
      .fn()
      .mockRejectedValueOnce({ inner: { message: 'Server returned 500' } })
      .mockRejectedValueOnce({ inner: { message: 'Server returned 500' } })
      .mockResolvedValue(JSON.stringify(page({ reset: true, cursor: 3 })));
    configureClaudeStore({
      native: {
        ...native,
        claudeGetSession: getSession,
        claudeGetTranscript: getTranscript,
      },
    });

    useClaudeStore.getState().open('conn', 't1');
    await jest.advanceTimersByTimeAsync(0);
    expect(useClaudeStore.getState().session?.dialogs).toEqual([]);
    expect(useClaudeStore.getState().transcriptError).toBe(
      'Server returned 500',
    );

    await jest.advanceTimersByTimeAsync(CLAUDE_POLL_MS);
    expect(useClaudeStore.getState().session?.dialogs).toEqual([DIALOG]);
    expect(useClaudeStore.getState().sessionError).toBeNull();
    expect(useClaudeStore.getState().transcriptError).toBe(
      'Server returned 500',
    );

    await jest.advanceTimersByTimeAsync(CLAUDE_POLL_MS);
    expect(useClaudeStore.getState().transcriptError).toBeNull();
    expect(useClaudeStore.getState().cursor).toBe(3);
  });

  it('leaves the state objects untouched when a poll brings nothing new', async () => {
    const { native } = stubNative([
      page({ entries: [assistant('a')], cursor: 1, reset: true }),
      page({ cursor: 1 }),
    ]);
    configureClaudeStore({ native });

    useClaudeStore.getState().open('conn', 't1');
    await jest.advanceTimersByTimeAsync(0);
    const before = useClaudeStore.getState();

    await jest.advanceTimersByTimeAsync(CLAUDE_POLL_MS);
    const after = useClaudeStore.getState();
    expect(after.entries).toBe(before.entries);
    expect(after.session).toBe(before.session);
  });

  it('keeps only the newest entries', async () => {
    const many = Array.from({ length: CLAUDE_MAX_ENTRIES }, (_, i) =>
      assistant(`a${i}`),
    );
    const { native } = stubNative([
      page({ entries: many, cursor: 1, reset: true }),
      page({ entries: [assistant('new')], cursor: 2 }),
    ]);
    configureClaudeStore({ native });

    useClaudeStore.getState().open('conn', 't1');
    await jest.advanceTimersByTimeAsync(0);
    await jest.advanceTimersByTimeAsync(CLAUDE_POLL_MS);

    const entries = useClaudeStore.getState().entries;
    expect(entries).toHaveLength(CLAUDE_MAX_ENTRIES);
    expect(entries[0]!.uuid).toBe('a1');
    expect(entries[entries.length - 1]!.uuid).toBe('new');
  });

  it('clears the transcript when the pane stops having one', async () => {
    const { native } = stubNative([
      page({ entries: [assistant('a')], cursor: 4, reset: true }),
    ]);
    const getSession = jest
      .fn()
      .mockResolvedValueOnce(JSON.stringify(session()))
      .mockResolvedValue(JSON.stringify(session({ has_transcript: false })));
    configureClaudeStore({
      native: { ...native, claudeGetSession: getSession },
    });

    useClaudeStore.getState().open('conn', 't1');
    await jest.advanceTimersByTimeAsync(0);
    expect(useClaudeStore.getState().entries).toHaveLength(1);

    await jest.advanceTimersByTimeAsync(CLAUDE_POLL_MS);
    expect(useClaudeStore.getState()).toMatchObject({
      entries: [],
      cursor: 0,
      sessionId: undefined,
    });
  });

  it('drops a dialog card answered elsewhere (404) and keeps it hidden', async () => {
    const { native } = stubNative(
      [page({ reset: true })],
      session({ dialogs: [DIALOG] }),
    );
    const answer = jest.fn(async () => {
      throw { inner: { message: CLAUDE_DIALOG_GONE_MESSAGE } };
    });
    configureClaudeStore({ native: { ...native, claudeAnswerDialog: answer } });

    useClaudeStore.getState().open('conn', 't1');
    await jest.advanceTimersByTimeAsync(0);
    expect(useClaudeStore.getState().session?.dialogs).toHaveLength(1);

    await useClaudeStore.getState().answerDialog('d1', { decision: 'allow' });
    expect(answer).toHaveBeenCalledWith(
      'conn',
      't1',
      'd1',
      '{"decision":"allow"}',
    );
    expect(useClaudeStore.getState().session?.dialogs).toEqual([]);
    expect(useClaudeStore.getState().dialogAnswers).toEqual({});

    // The server still lists it on the next poll; the card stays dropped.
    await jest.advanceTimersByTimeAsync(CLAUDE_POLL_MS);
    expect(useClaudeStore.getState().session?.dialogs).toEqual([]);
  });

  it('keeps the card and reports other answer failures', async () => {
    const { native } = stubNative(
      [page({ reset: true })],
      session({ dialogs: [DIALOG] }),
    );
    configureClaudeStore({
      native: {
        ...native,
        claudeAnswerDialog: async () => {
          throw new Error('HTTP request failed: refused');
        },
      },
    });

    useClaudeStore.getState().open('conn', 't1');
    await jest.advanceTimersByTimeAsync(0);
    await useClaudeStore
      .getState()
      .answerDialog('d1', { decision: 'deny', message: 'no' });

    expect(useClaudeStore.getState().session?.dialogs).toHaveLength(1);
    expect(useClaudeStore.getState().dialogAnswers).toEqual({
      d1: { status: 'failed', error: 'HTTP request failed: refused' },
    });
  });

  it('sends one answer at a time per dialog', async () => {
    const pending = deferred<void>();
    const { native } = stubNative(
      [page({ reset: true })],
      session({ dialogs: [DIALOG] }),
    );
    const answer = jest
      .fn()
      .mockReturnValueOnce(pending.promise)
      .mockResolvedValue(undefined);
    configureClaudeStore({ native: { ...native, claudeAnswerDialog: answer } });

    useClaudeStore.getState().open('conn', 't1');
    await jest.advanceTimersByTimeAsync(0);
    const first = useClaudeStore
      .getState()
      .answerDialog('d1', { decision: 'allow' });
    expect(useClaudeStore.getState().dialogAnswers).toEqual({
      d1: { status: 'sending' },
    });

    await useClaudeStore.getState().answerDialog('d1', { decision: 'deny' });
    expect(answer).toHaveBeenCalledTimes(1);

    pending.resolve();
    await first;
    expect(useClaudeStore.getState().dialogAnswers).toEqual({});
    expect(useClaudeStore.getState().session?.dialogs).toEqual([]);
  });

  it('lets a failed answer be sent again', async () => {
    const { native } = stubNative(
      [page({ reset: true })],
      session({ dialogs: [DIALOG] }),
    );
    const answer = jest
      .fn()
      .mockRejectedValueOnce(new Error('HTTP request failed: refused'))
      .mockResolvedValue(undefined);
    configureClaudeStore({ native: { ...native, claudeAnswerDialog: answer } });

    useClaudeStore.getState().open('conn', 't1');
    await jest.advanceTimersByTimeAsync(0);
    await useClaudeStore.getState().answerDialog('d1', { decision: 'allow' });
    await useClaudeStore.getState().answerDialog('d1', { decision: 'allow' });

    expect(answer).toHaveBeenCalledTimes(2);
    expect(useClaudeStore.getState().session?.dialogs).toEqual([]);
    expect(useClaudeStore.getState().dialogAnswers).toEqual({});
  });

  it('forgets a dialog once the server stops listing it', async () => {
    const { native } = stubNative([page({ reset: true })]);
    const getSession = jest
      .fn()
      .mockResolvedValueOnce(JSON.stringify(session({ dialogs: [DIALOG] })))
      .mockResolvedValueOnce(JSON.stringify(session({ dialogs: [] })))
      .mockResolvedValue(JSON.stringify(session({ dialogs: [DIALOG] })));
    const answer = jest.fn(async () => {
      throw { inner: { message: CLAUDE_DIALOG_GONE_MESSAGE } };
    });
    configureClaudeStore({
      native: {
        ...native,
        claudeGetSession: getSession,
        claudeAnswerDialog: answer,
      },
    });

    useClaudeStore.getState().open('conn', 't1');
    await jest.advanceTimersByTimeAsync(0);
    await useClaudeStore.getState().answerDialog('d1', { decision: 'allow' });
    expect(useClaudeStore.getState().session?.dialogs).toEqual([]);

    await jest.advanceTimersByTimeAsync(CLAUDE_POLL_MS);
    // A later dialog with the same id is a new one and shows.
    await jest.advanceTimersByTimeAsync(CLAUDE_POLL_MS);
    expect(useClaudeStore.getState().session?.dialogs).toEqual([DIALOG]);
  });

  it('drops an answer error once its dialog is gone', async () => {
    const { native } = stubNative([page({ reset: true })]);
    const other = { ...DIALOG, id: 'd2' };
    const getSession = jest
      .fn()
      .mockResolvedValueOnce(
        JSON.stringify(session({ dialogs: [DIALOG, other] })),
      )
      .mockResolvedValue(JSON.stringify(session({ dialogs: [other] })));
    configureClaudeStore({
      native: {
        ...native,
        claudeGetSession: getSession,
        claudeAnswerDialog: async () => {
          throw new Error('HTTP request failed: refused');
        },
      },
    });

    useClaudeStore.getState().open('conn', 't1');
    await jest.advanceTimersByTimeAsync(0);
    await useClaudeStore.getState().answerDialog('d1', { decision: 'allow' });
    await useClaudeStore.getState().answerDialog('d2', { decision: 'allow' });
    expect(Object.keys(useClaudeStore.getState().dialogAnswers)).toEqual([
      'd1',
      'd2',
    ]);

    await jest.advanceTimersByTimeAsync(CLAUDE_POLL_MS);
    expect(useClaudeStore.getState().dialogAnswers).toEqual({
      d2: { status: 'failed', error: 'HTTP request failed: refused' },
    });
  });

  it('exposes the delivery of a sent prompt and the reason of a rejected one', async () => {
    const { native } = stubNative([page({ reset: true })]);
    const sendPrompt = jest
      .fn()
      .mockResolvedValueOnce(JSON.stringify({ delivery: 'queued' }))
      .mockRejectedValueOnce({ inner: { message: 'Claude is busy' } });
    configureClaudeStore({
      native: { ...native, claudeSendPrompt: sendPrompt },
    });

    useClaudeStore.getState().open('conn', 't1');

    await expect(useClaudeStore.getState().sendPrompt('hello')).resolves.toBe(
      true,
    );
    expect(sendPrompt).toHaveBeenCalledWith('conn', 't1', 'hello');
    expect(useClaudeStore.getState().lastDelivery).toBe('queued');

    await expect(useClaudeStore.getState().sendPrompt('again')).resolves.toBe(
      false,
    );
    expect(useClaudeStore.getState().lastDelivery).toBeNull();
    expect(useClaudeStore.getState().promptError).toBe('Claude is busy');

    await expect(useClaudeStore.getState().sendPrompt('   ')).resolves.toBe(
      false,
    );
    expect(sendPrompt).toHaveBeenCalledTimes(2);
  });

  it('interrupts with Escape', async () => {
    const { native } = stubNative([page({ reset: true })]);
    configureClaudeStore({ native });

    useClaudeStore.getState().open('conn', 't1');
    await useClaudeStore.getState().interrupt();

    expect(native.sendSpecialKey).toHaveBeenCalledWith('conn', 't1', 'Escape');
  });
});
