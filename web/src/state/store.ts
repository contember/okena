import { createContext, useContext } from "react";
import type { StateResponse, WsOutbound } from "../api/types";
import { WsManager, type WsStatus } from "../api/websocket";

// ── Terminal Registry ───────────────────────────────────────────────────────

/** Maps streamId → xterm.write callback */
export class TerminalRegistry {
  private handlers = new Map<number, (data: Uint8Array) => void>();

  register(streamId: number, handler: (data: Uint8Array) => void): void {
    this.handlers.set(streamId, handler);
  }

  unregister(streamId: number): void {
    this.handlers.delete(streamId);
  }

  write(streamId: number, data: Uint8Array): void {
    this.handlers.get(streamId)?.(data);
  }
}

// ── App State ───────────────────────────────────────────────────────────────

export interface AppState {
  workspace: StateResponse | null;
  selectedProjectId: string | null;
  wsStatus: WsStatus;
  /** terminalId → streamId mapping from WS subscribe */
  streamMappings: Record<string, number>;
}

export type AppAction =
  | { type: "set_workspace"; workspace: StateResponse }
  | { type: "select_project"; projectId: string }
  | { type: "set_ws_status"; status: WsStatus }
  | { type: "set_stream_mappings"; mappings: Record<string, number> }
  | { type: "clear_stream_mappings" };

export function appReducer(state: AppState, action: AppAction): AppState {
  switch (action.type) {
    case "set_workspace":
      return { ...state, workspace: action.workspace };
    case "select_project":
      return { ...state, selectedProjectId: action.projectId };
    case "set_ws_status":
      return { ...state, wsStatus: action.status };
    case "set_stream_mappings":
      return { ...state, streamMappings: { ...state.streamMappings, ...action.mappings } };
    case "clear_stream_mappings":
      return { ...state, streamMappings: {} };
  }
}

export const initialState: AppState = {
  workspace: null,
  selectedProjectId: null,
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
