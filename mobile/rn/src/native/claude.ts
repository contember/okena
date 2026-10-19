/**
 * claude.ts — wire types for "Claude remote" and their boundary parsers.
 *
 * The `claude*` functions of {@link OkenaNative} return JSON strings shaped
 * like the Rust types in `crates/okena-core/src/claude_remote.rs`. The types
 * below mirror them field for field (snake_case, as on the wire); the
 * `parse*` functions validate a JSON string into them and throw
 * {@link ClaudeWireError} on anything else.
 */

export type AgentLifecycle = 'working' | 'blocked' | 'done' | 'idle';

export interface ApiClaudeDialog {
  id: string;
  tool_name: string;
  /** Any JSON value, as Claude sent it; usually an object. */
  tool_input: unknown;
  permission_suggestions: unknown[];
  created_at_ms: number;
}

export interface ApiClaudeSession {
  terminal_id: string;
  session_id?: string;
  has_transcript: boolean;
  hooks_attached: boolean;
  channel_attached: boolean;
  /** `undefined` when unknown. */
  busy?: boolean;
  queued_prompts: number;
  /** Oldest first. */
  dialogs: ApiClaudeDialog[];
}

export type ApiClaudeDecision =
  | { decision: 'allow'; always?: boolean }
  | { decision: 'deny'; message?: string }
  | { decision: 'answer'; answers: Record<string, string> };

export type ApiClaudePromptDelivery = 'channel' | 'pty' | 'queued';

export interface ApiClaudePromptResponse {
  delivery: ApiClaudePromptDelivery;
}

export type ApiClaudePromptOrigin = 'terminal' | 'channel';

export type ApiClaudeEntry =
  | { kind: 'user'; uuid: string; text: string; origin: ApiClaudePromptOrigin }
  | { kind: 'assistant'; uuid: string; text: string }
  | {
      kind: 'tool_use';
      uuid: string;
      tool_use_id: string;
      name: string;
      summary: string;
    }
  | {
      kind: 'tool_result';
      uuid: string;
      tool_use_id: string;
      text: string;
      is_error: boolean;
    };

export interface ApiClaudeTranscriptPage {
  session_id?: string;
  entries: ApiClaudeEntry[];
  cursor: number;
  reset: boolean;
}

/** One question of an `AskUserQuestion` dialog (`tool_input.questions[]`). */
export interface ClaudeQuestion {
  question: string;
  header?: string;
  multiSelect: boolean;
  options: ClaudeQuestionOption[];
}

export interface ClaudeQuestionOption {
  label: string;
  description?: string;
}

export class ClaudeWireError extends Error {}

// ── primitives ──────────────────────────────────────────────────────────────

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function fail(path: string, expected: string): never {
  throw new ClaudeWireError(`${path}: expected ${expected}`);
}

function record(value: unknown, path: string): Record<string, unknown> {
  return isRecord(value) ? value : fail(path, 'an object');
}

function str(value: unknown, path: string): string {
  return typeof value === 'string' ? value : fail(path, 'a string');
}

function optionalStr(value: unknown, path: string): string | undefined {
  return value === undefined || value === null ? undefined : str(value, path);
}

function bool(value: unknown, path: string): boolean {
  return typeof value === 'boolean' ? value : fail(path, 'a boolean');
}

function optionalBool(value: unknown, path: string): boolean | undefined {
  return value === undefined || value === null ? undefined : bool(value, path);
}

function count(value: unknown, path: string): number {
  return typeof value === 'number' && Number.isSafeInteger(value) && value >= 0
    ? value
    : fail(path, 'a non-negative integer');
}

function present(value: unknown, path: string): unknown {
  return value === undefined ? fail(path, 'a JSON value') : value;
}

function array(value: unknown, path: string): unknown[] {
  return Array.isArray(value) ? value : fail(path, 'an array');
}

function oneOf<T extends string>(
  value: unknown,
  allowed: readonly T[],
  path: string,
): T {
  const found = allowed.find((candidate) => candidate === value);
  return found ?? fail(path, allowed.join(' | '));
}

function parseJson(json: string, what: string): unknown {
  try {
    return JSON.parse(json);
  } catch (error) {
    throw new ClaudeWireError(`${what}: invalid JSON (${String(error)})`);
  }
}

// ── wire records ────────────────────────────────────────────────────────────

const AGENT_LIFECYCLES: readonly AgentLifecycle[] = [
  'working',
  'blocked',
  'done',
  'idle',
];
const DELIVERIES: readonly ApiClaudePromptDelivery[] = [
  'channel',
  'pty',
  'queued',
];
const ORIGINS: readonly ApiClaudePromptOrigin[] = ['terminal', 'channel'];

/** The lifecycle token, or `undefined` for a value this client doesn't know. */
export function toAgentLifecycle(value: unknown): AgentLifecycle | undefined {
  return AGENT_LIFECYCLES.find((lifecycle) => lifecycle === value);
}

function parseDialog(value: unknown, path: string): ApiClaudeDialog {
  const dialog = record(value, path);
  return {
    id: str(dialog.id, `${path}.id`),
    tool_name: str(dialog.tool_name, `${path}.tool_name`),
    tool_input: present(dialog.tool_input, `${path}.tool_input`),
    permission_suggestions:
      dialog.permission_suggestions === undefined
        ? []
        : array(
            dialog.permission_suggestions,
            `${path}.permission_suggestions`,
          ),
    created_at_ms: count(dialog.created_at_ms, `${path}.created_at_ms`),
  };
}

export function parseClaudeSession(json: string): ApiClaudeSession {
  const session = record(parseJson(json, 'session'), 'session');
  return {
    terminal_id: str(session.terminal_id, 'session.terminal_id'),
    session_id: optionalStr(session.session_id, 'session.session_id'),
    has_transcript: bool(session.has_transcript, 'session.has_transcript'),
    hooks_attached: bool(session.hooks_attached, 'session.hooks_attached'),
    channel_attached: bool(
      session.channel_attached,
      'session.channel_attached',
    ),
    busy: optionalBool(session.busy, 'session.busy'),
    queued_prompts: count(session.queued_prompts, 'session.queued_prompts'),
    dialogs: array(session.dialogs, 'session.dialogs').map((dialog, index) =>
      parseDialog(dialog, `session.dialogs[${index}]`),
    ),
  };
}

/** `null` for an entry kind this client doesn't know (a newer server). */
function parseEntry(value: unknown, path: string): ApiClaudeEntry | null {
  const entry = record(value, path);
  const uuid = str(entry.uuid, `${path}.uuid`);
  switch (entry.kind) {
    case 'user':
      return {
        kind: 'user',
        uuid,
        text: str(entry.text, `${path}.text`),
        origin: oneOf(entry.origin, ORIGINS, `${path}.origin`),
      };
    case 'assistant':
      return { kind: 'assistant', uuid, text: str(entry.text, `${path}.text`) };
    case 'tool_use':
      return {
        kind: 'tool_use',
        uuid,
        tool_use_id: str(entry.tool_use_id, `${path}.tool_use_id`),
        name: str(entry.name, `${path}.name`),
        summary: str(entry.summary, `${path}.summary`),
      };
    case 'tool_result':
      return {
        kind: 'tool_result',
        uuid,
        tool_use_id: str(entry.tool_use_id, `${path}.tool_use_id`),
        text: str(entry.text, `${path}.text`),
        is_error: bool(entry.is_error, `${path}.is_error`),
      };
    default:
      return null;
  }
}

export function parseClaudeTranscriptPage(
  json: string,
): ApiClaudeTranscriptPage {
  const page = record(parseJson(json, 'transcript'), 'transcript');
  const entries: ApiClaudeEntry[] = [];
  array(page.entries, 'transcript.entries').forEach((value, index) => {
    const entry = parseEntry(value, `transcript.entries[${index}]`);
    if (entry !== null) entries.push(entry);
  });
  return {
    session_id: optionalStr(page.session_id, 'transcript.session_id'),
    entries,
    cursor: count(page.cursor, 'transcript.cursor'),
    reset: bool(page.reset, 'transcript.reset'),
  };
}

export function parseClaudePromptResponse(
  json: string,
): ApiClaudePromptResponse {
  const response = record(
    parseJson(json, 'prompt response'),
    'prompt response',
  );
  return {
    delivery: oneOf(response.delivery, DELIVERIES, 'prompt response.delivery'),
  };
}

// ── tool_input readers (dialog UI) ──────────────────────────────────────────

/**
 * The questions of an `AskUserQuestion` dialog, or `null` when `tool_input`
 * doesn't have the expected shape (the UI then falls back to allow / deny).
 */
export function readClaudeQuestions(
  toolInput: unknown,
): ClaudeQuestion[] | null {
  try {
    const input = record(toolInput, 'tool_input');
    const questions = array(input.questions, 'questions').map(
      (value, index) => {
        const path = `questions[${index}]`;
        const question = record(value, path);
        return {
          question: str(question.question, `${path}.question`),
          header: optionalStr(question.header, `${path}.header`),
          multiSelect:
            optionalBool(question.multiSelect, `${path}.multiSelect`) ?? false,
          options: array(question.options, `${path}.options`).map(
            (option, optionIndex) => {
              const optionPath = `${path}.options[${optionIndex}]`;
              const parsed = record(option, optionPath);
              return {
                label: str(parsed.label, `${optionPath}.label`),
                description: optionalStr(
                  parsed.description,
                  `${optionPath}.description`,
                ),
              };
            },
          ),
        };
      },
    );
    return questions.length > 0 ? questions : null;
  } catch (error) {
    if (error instanceof ClaudeWireError) return null;
    throw error;
  }
}

/** `tool_input.plan` of an `ExitPlanMode` dialog, if it is a string. */
export function readClaudePlan(toolInput: unknown): string | undefined {
  return isRecord(toolInput) && typeof toolInput.plan === 'string'
    ? toolInput.plan
    : undefined;
}

const PREVIEW_FIELDS = [
  'command',
  'file_path',
  'path',
  'url',
  'pattern',
  'query',
  'description',
];

/** A readable preview of a tool call: its most telling field, else pretty JSON. */
export function previewClaudeToolInput(toolInput: unknown): string {
  if (isRecord(toolInput)) {
    for (const field of PREVIEW_FIELDS) {
      const value = toolInput[field];
      if (typeof value === 'string' && value.length > 0) return value;
    }
  }
  return JSON.stringify(toolInput, null, 2);
}
