import type { JsonValue } from "../api/types";

type JsonObject = { [key: string]: JsonValue };

export interface ClaudeQuestionOption {
  label: string;
  description: string;
}

export interface ClaudeQuestion {
  question: string;
  header: string;
  multiSelect: boolean;
  options: ClaudeQuestionOption[];
}

/** Fields shown verbatim as the headline of a permission dialog, in priority order. */
const PREVIEW_FIELDS = ["command", "file_path", "notebook_path", "path", "url", "pattern", "query", "prompt"];

function asObject(value: JsonValue): JsonObject | null {
  return value !== null && typeof value === "object" && !Array.isArray(value) ? value : null;
}

function stringField(object: JsonObject, key: string): string | null {
  const value = object[key];
  return typeof value === "string" ? value : null;
}

/** The single field that best describes a tool call, or `null` when none is present. */
export function toolInputHeadline(toolInput: JsonValue): string | null {
  const object = asObject(toolInput);
  if (!object) return null;
  for (const key of PREVIEW_FIELDS) {
    const value = stringField(object, key);
    if (value) return value;
  }
  return null;
}

export function toolInputDescription(toolInput: JsonValue): string | null {
  const object = asObject(toolInput);
  return object ? stringField(object, "description") : null;
}

export function toolInputPlan(toolInput: JsonValue): string | null {
  const object = asObject(toolInput);
  return object ? stringField(object, "plan") : null;
}

/** `AskUserQuestion` questions; `null` when the input does not have the expected shape. */
export function toolInputQuestions(toolInput: JsonValue): ClaudeQuestion[] | null {
  const object = asObject(toolInput);
  const rawQuestions = object?.questions;
  if (!Array.isArray(rawQuestions)) return null;
  const questions: ClaudeQuestion[] = [];
  for (const raw of rawQuestions) {
    const question = parseQuestion(raw);
    if (!question) return null;
    questions.push(question);
  }
  return questions.length > 0 ? questions : null;
}

function parseQuestion(raw: JsonValue): ClaudeQuestion | null {
  const object = asObject(raw);
  if (!object) return null;
  const question = stringField(object, "question");
  const rawOptions = object.options;
  if (!question || !Array.isArray(rawOptions)) return null;
  const options: ClaudeQuestionOption[] = [];
  for (const rawOption of rawOptions) {
    const option = asObject(rawOption);
    const label = option ? stringField(option, "label") : null;
    if (!option || !label) return null;
    options.push({ label, description: stringField(option, "description") ?? "" });
  }
  return {
    question,
    header: stringField(object, "header") ?? "",
    multiSelect: object.multiSelect === true,
    options,
  };
}
