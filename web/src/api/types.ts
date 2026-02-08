// Types matching crates/okena-core exactly

// ── REST types ──────────────────────────────────────────────────────────────

export interface HealthResponse {
  status: string;
  version: string;
  uptime_secs: number;
}

export interface StateResponse {
  state_version: number;
  projects: ApiProject[];
  focused_project_id: string | null;
  fullscreen_terminal: ApiFullscreen | null;
}

export interface ApiProject {
  id: string;
  name: string;
  path: string;
  is_visible: boolean;
  layout: ApiLayoutNode | null;
  terminal_names: Record<string, string>;
}

// serde(tag = "type", rename_all = "lowercase")
export type ApiLayoutNode =
  | { type: "terminal"; terminal_id: string | null; minimized: boolean; detached: boolean }
  | { type: "split"; direction: SplitDirection; sizes: number[]; children: ApiLayoutNode[] }
  | { type: "tabs"; children: ApiLayoutNode[]; active_tab: number };

// serde(rename_all = "lowercase")
export type SplitDirection = "horizontal" | "vertical";

export interface ApiFullscreen {
  project_id: string;
  terminal_id: string;
}

// serde(tag = "action", rename_all = "snake_case")
export type ActionRequest =
  | { action: "send_text"; terminal_id: string; text: string }
  | { action: "run_command"; terminal_id: string; command: string }
  | { action: "send_special_key"; terminal_id: string; key: SpecialKey }
  | { action: "split_terminal"; project_id: string; path: number[]; direction: SplitDirection }
  | { action: "close_terminal"; project_id: string; terminal_id: string }
  | { action: "focus_terminal"; project_id: string; terminal_id: string }
  | { action: "read_content"; terminal_id: string }
  | { action: "resize"; terminal_id: string; cols: number; rows: number };

export interface PairRequest {
  code: string;
}

export interface PairResponse {
  token: string;
  expires_in: number;
}

export interface ErrorResponse {
  error: string;
}

// ── WebSocket types ─────────────────────────────────────────────────────────

// serde(tag = "type", rename_all = "snake_case")
export type WsInbound =
  | { type: "auth"; token: string }
  | { type: "subscribe"; terminal_ids: string[] }
  | { type: "unsubscribe"; terminal_ids: string[] }
  | { type: "send_text"; terminal_id: string; text: string }
  | { type: "send_special_key"; terminal_id: string; key: SpecialKey }
  | { type: "resize"; terminal_id: string; cols: number; rows: number }
  | { type: "ping" };

// serde(tag = "type", rename_all = "snake_case")
export type WsOutbound =
  | { type: "auth_ok" }
  | { type: "auth_failed"; error: string }
  | { type: "subscribed"; mappings: Record<string, number> }
  | { type: "state_changed"; state_version: number }
  | { type: "dropped"; count: number }
  | { type: "pong" }
  | { type: "error"; error: string };

// Default PascalCase serialization
export type SpecialKey =
  | "Enter"
  | "Escape"
  | "CtrlC"
  | "CtrlD"
  | "CtrlZ"
  | "Tab"
  | "ArrowUp"
  | "ArrowDown"
  | "ArrowLeft"
  | "ArrowRight"
  | "Home"
  | "End"
  | "PageUp"
  | "PageDown";

// ── Binary frame parsing ────────────────────────────────────────────────────

/** Parse a binary PTY frame: [proto=1][type=1][streamId:u32BE][data...] */
export function parsePtyFrame(data: ArrayBuffer): { streamId: number; payload: Uint8Array } | null {
  const view = new DataView(data);
  if (data.byteLength < 6 || view.getUint8(0) !== 1 || view.getUint8(1) !== 1) {
    return null;
  }
  const streamId = view.getUint32(2, false); // big-endian
  const payload = new Uint8Array(data, 6);
  return { streamId, payload };
}
