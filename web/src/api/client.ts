import type { StateResponse, PairResponse, ActionRequest } from "./types";
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

export async function postAction(action: ActionRequest): Promise<Record<string, unknown>> {
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
