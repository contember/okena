import type {
  StateResponse,
  PairResponse,
  ActionRequest,
  ApiClaudeSession,
  ApiClaudeTranscriptPage,
  ApiClaudeDecision,
  ApiClaudePromptRequest,
  ApiClaudePromptResponse,
} from "./types";
import { loadToken, saveToken } from "../auth/token";

export class AuthError extends Error {
  constructor(status: number) {
    super(`HTTP ${status}`);
    this.name = "AuthError";
  }
}

function baseUrl(): string {
  return window.location.origin;
}

function authHeaders(): Record<string, string> {
  const token = loadToken();
  if (!token) return {};
  return { Authorization: `Bearer ${token}` };
}

export async function pair(code: string): Promise<PairResponse> {
  const res = await fetch(`${baseUrl()}/v1/pair`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ code }),
  });
  if (!res.ok) {
    const body = await res.json().catch(() => ({ error: "Pairing failed" }));
    throw new Error(body.error || `HTTP ${res.status}`);
  }
  const data: PairResponse = await res.json();
  saveToken(data.token, data.expires_in);
  return data;
}

export async function getState(): Promise<StateResponse> {
  const res = await fetch(`${baseUrl()}/v1/state`, {
    headers: authHeaders(),
  });
  if (res.status === 401) throw new AuthError(res.status);
  if (!res.ok) throw new Error(`HTTP ${res.status}`);
  return res.json();
}

export async function postAction(action: ActionRequest): Promise<unknown> {
  const res = await fetch(`${baseUrl()}/v1/actions`, {
    method: "POST",
    headers: { "Content-Type": "application/json", ...authHeaders() },
    body: JSON.stringify(action),
  });
  if (res.status === 401) throw new AuthError(res.status);
  if (!res.ok) {
    const body = await res.json().catch(() => ({ error: "Action failed" }));
    throw new Error(body.error || `HTTP ${res.status}`);
  }
  return res.json();
}

export async function refresh(): Promise<void> {
  const res = await fetch(`${baseUrl()}/v1/refresh`, {
    method: "POST",
    headers: authHeaders(),
  });
  if (!res.ok) throw new Error(`HTTP ${res.status}`);
  const data: PairResponse = await res.json();
  saveToken(data.token, data.expires_in);
}

function claudeTerminalUrl(terminalId: string): string {
  return `${baseUrl()}/v1/claude/terminals/${encodeURIComponent(terminalId)}`;
}

async function errorText(res: Response): Promise<string> {
  const text = (await res.text().catch(() => "")).trim();
  return text || `HTTP ${res.status}`;
}

export async function getClaudeSession(terminalId: string): Promise<ApiClaudeSession> {
  const res = await fetch(claudeTerminalUrl(terminalId), { headers: authHeaders() });
  if (res.status === 401) throw new AuthError(res.status);
  if (!res.ok) throw new Error(await errorText(res));
  return res.json();
}

/** Resolves to `null` when the pane has no readable transcript (404). */
export async function getClaudeTranscript(
  terminalId: string,
  cursor: number | null,
  sessionId: string | null,
): Promise<ApiClaudeTranscriptPage | null> {
  const params = new URLSearchParams();
  if (cursor !== null) params.set("cursor", String(cursor));
  if (sessionId !== null) params.set("session_id", sessionId);
  const queryString = params.toString();
  const query = queryString ? `?${queryString}` : "";
  const res = await fetch(`${claudeTerminalUrl(terminalId)}/transcript${query}`, {
    headers: authHeaders(),
  });
  if (res.status === 401) throw new AuthError(res.status);
  if (res.status === 404) return null;
  if (!res.ok) throw new Error(await errorText(res));
  return res.json();
}

export type ClaudePromptResult =
  | { status: "accepted"; response: ApiClaudePromptResponse }
  | { status: "rejected"; reason: string };

/** A 400/409 is a normal outcome (empty text, Claude busy) and comes back as `rejected`. */
export async function postClaudePrompt(terminalId: string, text: string): Promise<ClaudePromptResult> {
  const body: ApiClaudePromptRequest = { text };
  const res = await fetch(`${claudeTerminalUrl(terminalId)}/prompt`, {
    method: "POST",
    headers: { "Content-Type": "application/json", ...authHeaders() },
    body: JSON.stringify(body),
  });
  if (res.status === 401) throw new AuthError(res.status);
  if (res.status === 400 || res.status === 409) {
    return { status: "rejected", reason: await errorText(res) };
  }
  if (!res.ok) throw new Error(await errorText(res));
  return { status: "accepted", response: await res.json() };
}

/** Resolves to `"gone"` when the dialog is no longer open (answered elsewhere). */
export async function postClaudeDecision(
  terminalId: string,
  dialogId: string,
  decision: ApiClaudeDecision,
): Promise<"answered" | "gone"> {
  const res = await fetch(
    `${claudeTerminalUrl(terminalId)}/dialogs/${encodeURIComponent(dialogId)}`,
    {
      method: "POST",
      headers: { "Content-Type": "application/json", ...authHeaders() },
      body: JSON.stringify(decision),
    },
  );
  if (res.status === 401) throw new AuthError(res.status);
  if (res.status === 404) return "gone";
  if (!res.ok) throw new Error(await errorText(res));
  return "answered";
}
