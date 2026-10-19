/// <reference types="jest" />
// tsconfig pins `types` to react/react-native, so pull jest globals in here.

import {
  ClaudeWireError,
  parseClaudePromptResponse,
  parseClaudeSession,
  parseClaudeTranscriptPage,
  previewClaudeToolInput,
  readClaudePlan,
  readClaudeQuestions,
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
};

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
    { kind: 'user', uuid: 'u1', text: 'hi', origin: 'channel' },
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
