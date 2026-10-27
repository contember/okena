/// <reference types="jest" />
// tsconfig pins `types` to react/react-native, so pull jest globals in here.

import type { ApiClaudeDialog, ApiClaudeSession } from '../native/claude';
import { CLAUDE_POLL_MS } from './claudeStore';
import {
  configureClaudeDialogsStore,
  useClaudeDialogsStore,
} from './claudeDialogsStore';

function dialog(id: string): ApiClaudeDialog {
  return {
    id,
    tool_name: 'Bash',
    tool_input: { command: 'ls' },
    permission_suggestions: [],
    created_at_ms: 1,
  };
}

function session(terminalId: string, dialogs: ApiClaudeDialog[]): string {
  const value: ApiClaudeSession = {
    terminal_id: terminalId,
    has_transcript: false,
    hooks_attached: true,
    channel_attached: false,
    queued_prompts: 0,
    dialogs,
    running_subagents: [],
  };
  return JSON.stringify(value);
}

describe('claude dialogs store', () => {
  beforeEach(() => {
    jest.useFakeTimers();
  });

  afterEach(() => {
    useClaudeDialogsStore.getState().close();
    jest.useRealTimers();
  });

  it('reads the dialogs of every terminal with one and drops the rest', async () => {
    let terminals = ['t1', 't2'];
    const sessions: Record<string, ApiClaudeDialog[]> = {
      t1: [dialog('d1')],
      t2: [dialog('d2'), dialog('d3')],
    };
    configureClaudeDialogsStore({
      dialogTerminals: () => terminals,
      native: {
        claudeGetSession: jest.fn(async (_conn: string, tid: string) =>
          session(tid, sessions[tid] ?? []),
        ),
      },
    });

    useClaudeDialogsStore.getState().open('conn-1');
    await jest.advanceTimersByTimeAsync(0);
    const { dialogs } = useClaudeDialogsStore.getState();
    expect(Object.keys(dialogs)).toEqual(['t1', 't2']);
    expect(dialogs.t2!.map((d) => d.id)).toEqual(['d2', 'd3']);

    terminals = ['t2'];
    await jest.advanceTimersByTimeAsync(CLAUDE_POLL_MS);
    expect(Object.keys(useClaudeDialogsStore.getState().dialogs)).toEqual(['t2']);
  });

  it('keeps the last dialogs of a terminal whose fetch fails', async () => {
    let fail = false;
    configureClaudeDialogsStore({
      dialogTerminals: () => ['t1'],
      native: {
        claudeGetSession: jest.fn(async () => {
          if (fail) throw new Error('offline');
          return session('t1', [dialog('d1')]);
        }),
      },
    });

    useClaudeDialogsStore.getState().open('conn-1');
    await jest.advanceTimersByTimeAsync(0);
    fail = true;
    await jest.advanceTimersByTimeAsync(CLAUDE_POLL_MS);
    expect(useClaudeDialogsStore.getState().dialogs.t1!.map((d) => d.id)).toEqual(['d1']);
  });

  it('drops a poll that finishes after close', async () => {
    let resolve: (json: string) => void = () => {};
    configureClaudeDialogsStore({
      dialogTerminals: () => ['t1'],
      native: {
        claudeGetSession: jest.fn(
          () =>
            new Promise<string>((r) => {
              resolve = r;
            }),
        ),
      },
    });

    useClaudeDialogsStore.getState().open('conn-1');
    await jest.advanceTimersByTimeAsync(0);
    useClaudeDialogsStore.getState().close();
    resolve(session('t1', [dialog('d1')]));
    await jest.advanceTimersByTimeAsync(0);
    expect(useClaudeDialogsStore.getState().dialogs).toEqual({});
  });
});
