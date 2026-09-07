import { createContext, useContext } from "react";
import type { StateResponse, WsOutbound } from "../api/types";
import { WsManager, type WsStatus } from "../api/websocket";

// ── Terminal Registry ───────────────────────────────────────────────────────

/**
 * Maps streamId → xterm.write callback, with buffering for data that
 * arrives before a handler is registered (e.g. snapshot frames).
 */
type TerminalFrame = {
  data: Uint8Array;
  isSnapshot: boolean;
};

type TerminalDataHandler = (data: Uint8Array, isSnapshot: boolean) => void;

export class TerminalRegistry {
  private handlers = new Map<number, TerminalDataHandler>();
  private pendingData = new Map<number, TerminalFrame[]>();

  register(streamId: number, handler: TerminalDataHandler): void {
    this.handlers.set(streamId, handler);
    // Flush any data that arrived before the handler was registered.
    const pending = this.pendingData.get(streamId);
    if (pending) {
      for (const frame of pending) {
        handler(frame.data, frame.isSnapshot);
      }
      this.pendingData.delete(streamId);
    }
  }

  unregister(streamId: number): void {
    this.handlers.delete(streamId);
    this.pendingData.delete(streamId);
  }

  write(streamId: number, data: Uint8Array, isSnapshot: boolean): void {
    const handler = this.handlers.get(streamId);
    if (handler) {
      handler(data, isSnapshot);
      return;
    }

    const frame = { data, isSnapshot };
    if (isSnapshot) {
      // A full snapshot supersedes anything buffered before it.
      this.pendingData.set(streamId, [frame]);
      return;
    }

    const pending = this.pendingData.get(streamId);
    if (pending) {
      pending.push(frame);
    } else {
      this.pendingData.set(streamId, [frame]);
    }
  }
}

// ── App State ───────────────────────────────────────────────────────────────

export interface AppState {
  workspace: StateResponse | null;
  selectedProjectId: string | null;
  selectedTerminalId: string | null;
  sidebarOpen: boolean;
  wsStatus: WsStatus;
  /** terminalId → streamId mapping from WS subscribe */
  streamMappings: Record<string, number>;
}

export type AppAction =
  | { type: "set_workspace"; workspace: StateResponse }
  | { type: "select_project"; projectId: string }
  | { type: "select_terminal"; terminalId: string | null }
  | { type: "set_sidebar_open"; open: boolean }
  | { type: "set_ws_status"; status: WsStatus }
  | { type: "set_stream_mappings"; mappings: Record<string, number> }
  | { type: "clear_stream_mappings" };

export function appReducer(state: AppState, action: AppAction): AppState {
  switch (action.type) {
    case "set_workspace":
      return { ...state, workspace: action.workspace };
    case "select_project":
      return { ...state, selectedProjectId: action.projectId };
    case "select_terminal":
      return { ...state, selectedTerminalId: action.terminalId };
    case "set_sidebar_open":
      return { ...state, sidebarOpen: action.open };
    case "set_ws_status":
      return { ...state, wsStatus: action.status };
    case "set_stream_mappings":
      return {
        ...state,
        streamMappings: { ...state.streamMappings, ...action.mappings },
      };
    case "clear_stream_mappings":
      return { ...state, streamMappings: {} };
  }
}

export const initialState: AppState = {
  workspace: null,
  selectedProjectId: null,
  selectedTerminalId: null,
  sidebarOpen: false,
  wsStatus: "disconnected",
  streamMappings: {},
};

// ── Context ─────────────────────────────────────────────────────────────────

export interface AppContextValue {
  state: AppState;
  dispatch: React.Dispatch<AppAction>;
  ws: WsManager;
  registry: TerminalRegistry;
  /** Handle a WS JSON message (called from App after dispatch) */
  handleWsMessage: (msg: WsOutbound) => void;
}

export const AppContext = createContext<AppContextValue>(null!);

export function useApp(): AppContextValue {
  return useContext(AppContext);
}
