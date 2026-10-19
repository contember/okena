/**
 * claudeChat.ts — view models for the Claude chat: transcript rows, the
 * session's activity and `AskUserQuestion` answers.
 */

import type { ApiClaudeEntry, ApiClaudeSession, ClaudeQuestion } from '../native/claude';

type ToolUseEntry = Extract<ApiClaudeEntry, { kind: 'tool_use' }>;
type ToolResultEntry = Extract<ApiClaudeEntry, { kind: 'tool_result' }>;

/**
 * One chat row. A `tool` row is a call with its result once known; a result
 * whose call is not in the transcript (cut off by the first page) is a `tool`
 * row without `use`.
 */
export type ClaudeChatRow =
  | { kind: 'user'; key: string; text: string; viaChannel: boolean }
  | { kind: 'assistant'; key: string; text: string }
  | { kind: 'tool'; key: string; use?: ToolUseEntry; result?: ToolResultEntry };

/** Transcript entries → chat rows, with each tool result folded into its call. */
export function buildClaudeChatRows(
  entries: readonly ApiClaudeEntry[],
): ClaudeChatRow[] {
  const rows: ClaudeChatRow[] = [];
  const toolRowIndex = new Map<string, number>();
  // One transcript line can yield several entries with the same uuid, so text
  // rows are keyed by position and tool rows by their call id.
  entries.forEach((entry, position) => {
    switch (entry.kind) {
      case 'user':
        rows.push({
          kind: 'user',
          key: `${entry.uuid}#${position}`,
          text: entry.text,
          viaChannel: entry.origin === 'channel',
        });
        break;
      case 'assistant':
        rows.push({
          kind: 'assistant',
          key: `${entry.uuid}#${position}`,
          text: entry.text,
        });
        break;
      case 'tool_use':
        toolRowIndex.set(entry.tool_use_id, rows.length);
        rows.push({
          kind: 'tool',
          key: `tool:${entry.tool_use_id}#${position}`,
          use: entry,
        });
        break;
      case 'tool_result': {
        const index = toolRowIndex.get(entry.tool_use_id);
        const row = index === undefined ? undefined : rows[index];
        if (
          index !== undefined &&
          row?.kind === 'tool' &&
          row.result === undefined
        ) {
          rows[index] = { ...row, result: entry };
        } else {
          rows.push({
            kind: 'tool',
            key: `result:${entry.tool_use_id}#${position}`,
            result: entry,
          });
        }
        break;
      }
    }
  });
  return rows;
}

/** Whether two rows render the same; tool rows hold the transcript's own entry objects. */
export function sameClaudeChatRow(a: ClaudeChatRow, b: ClaudeChatRow): boolean {
  switch (a.kind) {
    case 'user':
      return (
        b.kind === 'user' &&
        a.key === b.key &&
        a.text === b.text &&
        a.viaChannel === b.viaChannel
      );
    case 'assistant':
      return b.kind === 'assistant' && a.key === b.key && a.text === b.text;
    case 'tool':
      return (
        b.kind === 'tool' &&
        a.key === b.key &&
        a.use === b.use &&
        a.result === b.result
      );
  }
}

/** What the user picked for one question. */
export interface QuestionSelection {
  labels: string[];
  /** Free text of the "Other" option; `null` while "Other" is not picked. */
  other: string | null;
}

export const EMPTY_SELECTION: QuestionSelection = { labels: [], other: null };

/** Pick or unpick an option (`null` = "Other"). Single-select keeps one pick. */
export function toggleQuestionOption(
  question: ClaudeQuestion,
  selection: QuestionSelection,
  label: string | null,
): QuestionSelection {
  if (!question.multiSelect) {
    return label === null
      ? { labels: [], other: selection.other ?? '' }
      : { labels: [label], other: null };
  }
  if (label === null) {
    return { ...selection, other: selection.other === null ? '' : null };
  }
  const labels = selection.labels.includes(label)
    ? selection.labels.filter((picked) => picked !== label)
    : [...selection.labels, label];
  return { ...selection, labels };
}

/**
 * The answer text Claude expects for one question: the label, comma-joined
 * labels for multi-select, or the "Other" text. `null` while unanswered.
 */
export function questionAnswer(selection: QuestionSelection): string | null {
  const other = selection.other?.trim() ?? '';
  const parts =
    other.length > 0 ? [...selection.labels, other] : selection.labels;
  return parts.length > 0 ? parts.join(', ') : null;
}

/** `{question text → answer}` once every question is answered, else `null`. */
export function questionAnswers(
  questions: readonly ClaudeQuestion[],
  selections: Readonly<Record<string, QuestionSelection>>,
): Record<string, string> | null {
  const answers: Record<string, string> = {};
  for (const question of questions) {
    const answer = questionAnswer(
      selections[question.question] ?? EMPTY_SELECTION,
    );
    if (answer === null) return null;
    answers[question.question] = answer;
  }
  return answers;
}

/** What the chat's status strip says about a Claude session. */
export type ClaudeActivity = 'loading' | 'needsInput' | 'unhooked' | 'working' | 'ready';

export function claudeActivity(session: ApiClaudeSession | null): ClaudeActivity {
  if (session === null) return 'loading';
  if (session.dialogs.length > 0) return 'needsInput';
  if (!session.hooks_attached) return 'unhooked';
  return session.busy === true ? 'working' : 'ready';
}
