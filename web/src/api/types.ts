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
  | { action: "resize"; terminal_id: string; cols: number; rows: number }
  | { action: "create_terminal"; project_id: string }
  | { action: "update_split_sizes"; project_id: string; path: number[]; sizes: number[] };

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

// ── Binary frame protocol ───────────────────────────────────────────────────

export const FRAME_TYPE_PTY = 1; // server → client: live PTY output
export const FRAME_TYPE_SNAPSHOT = 2; // server → client: full screen redraw
export const FRAME_TYPE_INPUT = 3; // client → server: terminal input

/** Parse a generic binary frame: [proto=1][frameType][streamId:u32BE][payload...] */
export function parseBinaryFrame(data: ArrayBuffer): { frameType: number; streamId: number; payload: Uint8Array } | null {
  const view = new DataView(data);
  if (data.byteLength < 6 || view.getUint8(0) !== 1) {
    return null;
  }
  const frameType = view.getUint8(1);
  const streamId = view.getUint32(2, false); // big-endian
  const payload = new Uint8Array(data, 6);
  return { frameType, streamId, payload };
}

/** Build a binary frame: [proto=1][frameType][streamId:u32BE][payload...] */
export function buildBinaryFrame(frameType: number, streamId: number, payload: Uint8Array): ArrayBuffer {
  const frame = new ArrayBuffer(6 + payload.length);
  const view = new DataView(frame);
  view.setUint8(0, 1); // proto version
  view.setUint8(1, frameType);
  view.setUint32(2, streamId, false); // big-endian
  new Uint8Array(frame, 6).set(payload);
  return frame;
}
