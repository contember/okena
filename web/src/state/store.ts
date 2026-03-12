import { createContext, useContext } from "react";
import type { KruhViewState, StateResponse, WsOutbound } from "../api/types";
import { WsManager, type WsStatus } from "../api/websocket";

// ── Terminal Registry ───────────────────────────────────────────────────────

/**
 * Maps streamId → xterm.write callback, with buffering for data that
 * arrives before a handler is registered (e.g. snapshot frames).
 */
export class TerminalRegistry {
  private handlers = new Map<number, (data: Uint8Array) => void>();
  private pendingData = new Map<number, Uint8Array[]>();

  register(streamId: number, handler: (data: Uint8Array) => void): void {
    this.handlers.set(streamId, handler);
    // Flush any data that arrived before the handler was registered
    const pending = this.pendingData.get(streamId);
    if (pending) {
      for (const data of pending) {
        handler(data);
      }
      this.pendingData.delete(streamId);
    }
  }

  unregister(streamId: number): void {
    this.handlers.delete(streamId);
    this.pendingData.delete(streamId);
  }

  write(streamId: number, data: Uint8Array): void {
    const handler = this.handlers.get(streamId);
    if (handler) {
      handler(data);
    } else {
      // Buffer data until a handler is registered
      let pending = this.pendingData.get(streamId);
      if (!pending) {
        pending = [];
        this.pendingData.set(streamId, pending);
      }
      pending.push(data);
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
  /** appId → latest KruhViewState from WS app_state_changed */
  appStates: Record<string, KruhViewState>;
}

export type AppAction =
  | { type: "set_workspace"; workspace: StateResponse }
  | { type: "select_project"; projectId: string }
  | { type: "select_terminal"; terminalId: string | null }
  | { type: "set_sidebar_open"; open: boolean }
  | { type: "set_ws_status"; status: WsStatus }
  | { type: "set_stream_mappings"; mappings: Record<string, number> }
  | { type: "clear_stream_mappings" }
  | { type: "set_app_state"; appId: string; state: KruhViewState };

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
      return { ...state, streamMappings: { ...state.streamMappings, ...action.mappings } };
    case "clear_stream_mappings":
      return { ...state, streamMappings: {} };
    case "set_app_state":
      return { ...state, appStates: { ...state.appStates, [action.appId]: action.state } };
  }
}

export const initialState: AppState = {
  workspace: null,
  selectedProjectId: null,
  selectedTerminalId: null,
  sidebarOpen: false,
  wsStatus: "disconnected",
  streamMappings: {},
  appStates: {},
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
