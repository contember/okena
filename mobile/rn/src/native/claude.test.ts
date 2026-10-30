/// <reference types="jest" />
// tsconfig pins `types` to react/react-native, so pull jest globals in here.

import {
  ClaudeWireError,
  parseClaudePromptResponse,
  parseClaudeSession,
  parseClaudeSubagentList,
  parseClaudeTranscriptPage,
  previewClaudeToolInput,
  readClaudePlan,
  readClaudeQuestions,
  summarizeClaudeDialog,
  type ApiClaudeDialog,
} from './claude';
import { nativeErrorMessage } from './okena';

const SESSION = {
  terminal_id: 't1',
  session_id: 's1',
  has_transcript: true,
  hooks_attached: true,
  channel_attached: false,
  busy: true,
  queued_prompts: 2,
  dialogs: [
    {
      id: 'd1',
      tool_name: 'Bash',
      tool_input: { command: 'ls' },
      permission_suggestions: [{ type: 'addRules' }],
      created_at_ms: 1700000000000,
    },
  ],
  running_subagents: [{ agent_id: 'a1', agent_type: 'Explore' }],
  pr_links: [
    {
      number: 582,
      url: 'https://github.com/acme/widgets/pull/582',
      repository: 'acme/widgets',
      last_seen_at: '2026-01-01T10:00:00.000Z',
    },
  ],
};

describe('parseClaudeSubagentList', () => {
  it('accepts the wire shape with optional fields left out', () => {
    const list = {
      session_id: 's1',
      subagents: [
        {
          agent_id: 'a1',
          tool_use_id: 'toolu_1',
          agent_type: 'Explore',
          description: 'Find callers',
          running: true,
        },
        { agent_id: 'b2', running: false },
      ],
    };
    expect(parseClaudeSubagentList(JSON.stringify(list))).toEqual(list);
  });

  it('rejects a subagent without its running flag', () => {
    expect(() =>
      parseClaudeSubagentList('{"subagents":[{"agent_id":"a1"}]}'),
    ).toThrow(
      new ClaudeWireError('subagents.subagents[0].running: expected a boolean'),
    );
  });
});

describe('parseClaudeSession', () => {
  it('accepts the wire shape', () => {
    expect(parseClaudeSession(JSON.stringify(SESSION))).toEqual(SESSION);
  });

  it('accepts omitted optional fields', () => {
    const minimal = {
      terminal_id: 't1',
      has_transcript: false,
      hooks_attached: false,
      channel_attached: false,
      queued_prompts: 0,
      dialogs: [
        { id: 'd1', tool_name: 'Bash', tool_input: {}, created_at_ms: 1 },
      ],
    };
    const parsed = parseClaudeSession(JSON.stringify(minimal));
    expect(parsed.session_id).toBeUndefined();
    expect(parsed.busy).toBeUndefined();
    expect(parsed.dialogs[0]!.permission_suggestions).toEqual([]);
    expect(parsed.running_subagents).toEqual([]);
    expect(parsed.pr_links).toEqual([]);
  });

  it('rejects a PR link that is not an https URL', () => {
    const bad = {
      ...SESSION,
      pr_links: [{ ...SESSION.pr_links[0], url: 'http://github.com/acme/widgets/pull/582' }],
    };
    expect(() => parseClaudeSession(JSON.stringify(bad))).toThrow(
      new ClaudeWireError('session.pr_links[0].url: expected an https URL'),
    );
  });

  it('accepts any JSON value as tool_input, but requires one', () => {
    for (const toolInput of ['ls', 42, null, ['a']]) {
      const session = {
        ...SESSION,
        dialogs: [{ ...SESSION.dialogs[0], tool_input: toolInput }],
      };
      expect(
        parseClaudeSession(JSON.stringify(session)).dialogs[0]?.tool_input,
      ).toEqual(toolInput);
    }
    const missing = {
      ...SESSION,
      dialogs: [{ ...SESSION.dialogs[0], tool_input: undefined }],
    };
    expect(() => parseClaudeSession(JSON.stringify(missing))).toThrow(
      new ClaudeWireError(
        'session.dialogs[0].tool_input: expected a JSON value',
      ),
    );
  });

  it('rejects a wrong field type with its path', () => {
    const bad = {
      ...SESSION,
      dialogs: [{ ...SESSION.dialogs[0], tool_name: 7 }],
    };
    expect(() => parseClaudeSession(JSON.stringify(bad))).toThrow(
      new ClaudeWireError('session.dialogs[0].tool_name: expected a string'),
    );
    expect(() =>
      parseClaudeSession(JSON.stringify({ ...SESSION, queued_prompts: -1 })),
    ).toThrow(ClaudeWireError);
    expect(() => parseClaudeSession('not json')).toThrow(ClaudeWireError);
  });
});

describe('parseClaudeTranscriptPage', () => {
  const ENTRIES = [
    {
      kind: 'user',
      uuid: 'u1',
      timestamp: '2026-10-30T10:00:00.000Z',
      text: 'hi',
      origin: 'channel',
    },
    { kind: 'assistant', uuid: 'a1', text: 'hello' },
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

  it('accepts every entry kind', () => {
    const page = {
      session_id: 's1',
      entries: ENTRIES,
      cursor: 42,
      reset: true,
    };
    expect(parseClaudeTranscriptPage(JSON.stringify(page))).toEqual(page);
  });

  it('reads a null timestamp as absent and rejects a non-string one', () => {
    const page = (timestamp: unknown) =>
      JSON.stringify({
        entries: [{ kind: 'assistant', uuid: 'a1', text: 'x', timestamp }],
        cursor: 1,
        reset: false,
      });
    expect(parseClaudeTranscriptPage(page(null)).entries[0].timestamp).toBe(
      undefined,
    );
    expect(() => parseClaudeTranscriptPage(page(5))).toThrow();
  });

  it('skips entry kinds it does not know', () => {
    const page = {
      entries: [{ kind: 'thinking', uuid: 'z' }, ENTRIES[1]],
      cursor: 1,
      reset: false,
    };
    expect(parseClaudeTranscriptPage(JSON.stringify(page)).entries).toEqual([
      ENTRIES[1],
    ]);
  });

  it('rejects malformed known entries', () => {
    const page = {
      entries: [{ ...ENTRIES[0], origin: 'phone' }],
      cursor: 1,
      reset: false,
    };
    expect(() => parseClaudeTranscriptPage(JSON.stringify(page))).toThrow(
      'transcript.entries[0].origin: expected terminal | channel',
    );
  });
});

describe('parseClaudePromptResponse', () => {
  it('accepts the three deliveries only', () => {
    expect(parseClaudePromptResponse('{"delivery":"pty"}')).toEqual({
      delivery: 'pty',
    });
    expect(() => parseClaudePromptResponse('{"delivery":"email"}')).toThrow(
      ClaudeWireError,
    );
  });
});

describe('readClaudeQuestions', () => {
  it('reads AskUserQuestion input', () => {
    const questions = readClaudeQuestions({
      questions: [
        {
          question: 'Which?',
          header: 'Pick',
          multiSelect: true,
          options: [{ label: 'A', description: 'first' }, { label: 'B' }],
        },
      ],
    });
    expect(questions).toEqual([
      {
        question: 'Which?',
        header: 'Pick',
        multiSelect: true,
        options: [
          { label: 'A', description: 'first' },
          { label: 'B', description: undefined },
        ],
      },
    ]);
  });

  it('returns null for input it cannot render', () => {
    expect(readClaudeQuestions('not an object')).toBeNull();
    expect(readClaudeQuestions({})).toBeNull();
    expect(readClaudeQuestions({ questions: [] })).toBeNull();
    expect(
      readClaudeQuestions({ questions: [{ question: 'Q', options: [{}] }] }),
    ).toBeNull();
  });
});

describe('previewClaudeToolInput', () => {
  it('prefers the telling field and falls back to JSON', () => {
    expect(
      previewClaudeToolInput({ command: 'rm -rf build', description: 'clean' }),
    ).toBe('rm -rf build');
    expect(previewClaudeToolInput({ n: 1 })).toBe('{\n  "n": 1\n}');
  });

  it('previews a non-object input as JSON', () => {
    expect(previewClaudeToolInput('ls')).toBe('"ls"');
    expect(previewClaudeToolInput([1])).toBe('[\n  1\n]');
  });
});

describe('readClaudePlan', () => {
  it('reads the plan of an object input only', () => {
    expect(readClaudePlan({ plan: '# Plan' })).toBe('# Plan');
    expect(readClaudePlan('# Plan')).toBeUndefined();
  });
});

describe('nativeErrorMessage', () => {
  it('reads the uniffi error payload, then plain errors', () => {
    expect(
      nativeErrorMessage({ inner: { message: 'dialog already answered' } }),
    ).toBe('dialog already answered');
    expect(nativeErrorMessage(new Error('boom'))).toBe('boom');
    expect(nativeErrorMessage('text')).toBe('text');
  });
});

describe('summarizeClaudeDialog', () => {
  const dialog = (tool_name: string, tool_input: unknown): ApiClaudeDialog => ({
    id: 'd1',
    tool_name,
    tool_input,
    permission_suggestions: [],
    created_at_ms: 1,
  });

  it('shows the first question of AskUserQuestion', () => {
    const input = {
      questions: [
        { question: 'Which layout?', options: [{ label: 'A' }] },
        { question: 'Second?', options: [{ label: 'B' }] },
      ],
    };
    expect(summarizeClaudeDialog(dialog('AskUserQuestion', input))).toBe('Which layout?');
  });

  it('shows the first plan line without its heading marks', () => {
    const input = { plan: '\n# Move the differ\n\n1. Step' };
    expect(summarizeClaudeDialog(dialog('ExitPlanMode', input))).toBe('Plan: Move the differ');
    expect(summarizeClaudeDialog(dialog('ExitPlanMode', {}))).toBe('Plan ready to approve');
  });

  it('shows any other tool call on one line', () => {
    expect(summarizeClaudeDialog(dialog('Bash', { command: 'ls\n  -la' }))).toBe('Bash: ls -la');
  });
});
