/// <reference types="jest" />
// tsconfig pins `types` to react/react-native, so pull jest globals in here.

import type {
  ApiClaudeDialog,
  ApiClaudeEntry,
  ApiClaudeSession,
  ClaudeQuestion,
} from '../native/claude';
import {
  claudeActivity,
  buildClaudeChatRows,
  EMPTY_SELECTION,
  questionAnswers,
  sameClaudeChatRow,
  toggleQuestionOption,
} from './claudeChat';

describe('buildClaudeChatRows', () => {
  it('folds each tool result into its call', () => {
    const rows = buildClaudeChatRows([
      { kind: 'user', uuid: 'u1', text: 'hi', origin: 'channel' },
      { kind: 'assistant', uuid: 'a1', text: 'on it' },
      {
        kind: 'tool_use',
        uuid: 'a1',
        tool_use_id: 'x',
        name: 'Bash',
        summary: 'ls',
      },
      {
        kind: 'tool_result',
        uuid: 'u2',
        tool_use_id: 'x',
        text: 'out',
        is_error: false,
      },
      {
        kind: 'tool_result',
        uuid: 'u3',
        tool_use_id: 'gone',
        text: 'orphan',
        is_error: true,
      },
    ]);

    expect(rows.map((row) => row.kind)).toEqual([
      'user',
      'assistant',
      'tool',
      'tool',
    ]);
    expect(rows[0]).toMatchObject({ viaChannel: true });
    expect(rows[2]).toMatchObject({
      use: { name: 'Bash' },
      result: { text: 'out' },
    });
    expect(rows[3]).toMatchObject({ result: { text: 'orphan' } });
    expect(rows[3]).not.toHaveProperty('use');
    expect(new Set(rows.map((row) => row.key)).size).toBe(rows.length);
  });

  it('keeps row keys when entries drop off the front', () => {
    const entries: ApiClaudeEntry[] = [
      { kind: 'user', uuid: 'u0', text: 'old', origin: 'terminal' },
      { kind: 'assistant', uuid: 'a1', text: 'first' },
      { kind: 'assistant', uuid: 'a1', text: 'second' },
      {
        kind: 'tool_use',
        uuid: 'a1',
        tool_use_id: 'x',
        name: 'Bash',
        summary: 'ls',
      },
      {
        kind: 'tool_result',
        uuid: 'u2',
        tool_use_id: 'x',
        text: 'out',
        is_error: false,
      },
    ];
    const keys = (list: ApiClaudeEntry[]) =>
      buildClaudeChatRows(list).map((row) => row.key);

    const before = keys(entries);
    const after = keys([
      ...entries.slice(1),
      { kind: 'assistant', uuid: 'a3', text: 'new' },
    ]);
    expect(after.slice(0, -1)).toEqual(before.slice(1));
    expect(new Set(before).size).toBe(before.length);

    // The call dropped off, its result remains: the same row.
    expect(keys(entries.slice(4))).toEqual([before[3]]);
  });

  it('keeps keys unique when a tool call id repeats', () => {
    const result = (text: string): ApiClaudeEntry => ({
      kind: 'tool_result',
      uuid: text,
      tool_use_id: 'x',
      text,
      is_error: false,
    });
    const rows = buildClaudeChatRows([result('r1'), result('r2')]);
    expect(rows).toHaveLength(2);
    expect(new Set(rows.map((row) => row.key)).size).toBe(2);
  });

  it('rebuilt rows of unchanged entries compare equal', () => {
    const entries = [
      { kind: 'assistant' as const, uuid: 'a1', text: 'on it' },
      {
        kind: 'tool_use' as const,
        uuid: 'a1',
        tool_use_id: 'x',
        name: 'Bash',
        summary: 'ls',
      },
    ];
    const before = buildClaudeChatRows(entries);
    const after = buildClaudeChatRows([
      ...entries,
      {
        kind: 'tool_result',
        uuid: 'u2',
        tool_use_id: 'x',
        text: 'out',
        is_error: false,
      },
    ]);
    expect(sameClaudeChatRow(before[0]!, after[0]!)).toBe(true);
    expect(sameClaudeChatRow(before[1]!, after[1]!)).toBe(false);
  });
});

describe('question answers', () => {
  const single: ClaudeQuestion = {
    question: 'Which one?',
    multiSelect: false,
    options: [{ label: 'A' }, { label: 'B' }],
  };
  const multi: ClaudeQuestion = {
    question: 'Which ones?',
    multiSelect: true,
    options: [{ label: 'A' }, { label: 'B' }],
  };

  it('single select keeps one pick, "Other" replaces it', () => {
    let selection = toggleQuestionOption(single, EMPTY_SELECTION, 'A');
    selection = toggleQuestionOption(single, selection, 'B');
    expect(selection).toEqual({ labels: ['B'], other: null });
    selection = toggleQuestionOption(single, selection, null);
    expect(selection).toEqual({ labels: [], other: '' });
  });

  it('multi select joins labels and the "Other" text', () => {
    let selection = toggleQuestionOption(multi, EMPTY_SELECTION, 'A');
    selection = toggleQuestionOption(multi, selection, 'B');
    selection = toggleQuestionOption(multi, selection, null);
    selection = { ...selection, other: ' C ' };
    expect(questionAnswers([multi], { [multi.question]: selection })).toEqual({
      'Which ones?': 'A, B, C',
    });
  });

  it('is unanswered until every question has an answer', () => {
    const picked = { labels: ['A'], other: null };
    expect(
      questionAnswers([single, multi], { [single.question]: picked }),
    ).toBeNull();
    expect(
      questionAnswers([single], {
        [single.question]: { labels: [], other: '  ' },
      }),
    ).toBeNull();
    expect(
      questionAnswers([single, multi], {
        [single.question]: picked,
        [multi.question]: picked,
      }),
    ).toEqual({ 'Which one?': 'A', 'Which ones?': 'A' });
  });
});

describe('claudeActivity', () => {
  const session: ApiClaudeSession = {
    terminal_id: 't1',
    has_transcript: true,
    hooks_attached: true,
    channel_attached: true,
    busy: false,
    queued_prompts: 0,
    dialogs: [],
    running_subagents: [],
  };
  const dialog: ApiClaudeDialog = {
    id: 'd1',
    tool_name: 'Bash',
    tool_input: {},
    permission_suggestions: [],
    created_at_ms: 0,
  };

  it('puts an open dialog before everything else', () => {
    expect(claudeActivity(null)).toBe('loading');
    expect(claudeActivity({ ...session, busy: true, dialogs: [dialog] })).toBe(
      'needsInput',
    );
    expect(claudeActivity({ ...session, hooks_attached: false })).toBe(
      'unhooked',
    );
    expect(claudeActivity({ ...session, busy: true })).toBe('working');
    expect(claudeActivity({ ...session, busy: undefined })).toBe('ready');
  });
});
