import { useReducer, useEffect, useRef, useCallback, useState } from "react";
import { appReducer, initialState, AppContext, TerminalRegistry } from "./state/store";
import { WsManager, type WsStatus } from "./api/websocket";
import type { WsOutbound } from "./api/types";
import { getState, refresh } from "./api/client";
import { loadToken, clearToken, tokenTtlSecs } from "./auth/token";
import { PairingScreen } from "./components/PairingScreen";
import { WorkspaceLayout } from "./components/WorkspaceLayout";

export function App() {
  const [state, dispatch] = useReducer(appReducer, initialState);
  const [authed, setAuthed] = useState<boolean | null>(null); // null = checking
  const wsRef = useRef<WsManager>(null!);
  const registryRef = useRef<TerminalRegistry>(null!);

  if (!wsRef.current) wsRef.current = new WsManager();
  if (!registryRef.current) registryRef.current = new TerminalRegistry();

  const fetchState = useCallback(async () => {
    try {
      const ws = await getState();
      dispatch({ type: "set_workspace", workspace: ws });
      // Auto-select focused project or first project
      if (!state.selectedProjectId) {
        const projectId = ws.focused_project_id ?? ws.projects[0]?.id ?? null;
        if (projectId) dispatch({ type: "select_project", projectId });
      }
    } catch {
      // 401 → clear token and go to pairing
      clearToken();
      setAuthed(false);
    }
  }, [state.selectedProjectId]);

  const handleWsMessage = useCallback(
    (msg: WsOutbound) => {
      switch (msg.type) {
        case "subscribed":
          dispatch({ type: "set_stream_mappings", mappings: msg.mappings });
          break;
        case "state_changed":
          fetchState();
          break;
        case "auth_failed":
          clearToken();
          setAuthed(false);
          break;
      }
    },
    [fetchState],
  );

  // Check auth on mount
  useEffect(() => {
    const token = loadToken();
    if (!token) {
      setAuthed(false);
      return;
    }
    // Verify token by fetching state
    getState()
      .then((ws) => {
        dispatch({ type: "set_workspace", workspace: ws });
        const projectId = ws.focused_project_id ?? ws.projects[0]?.id ?? null;
        if (projectId) dispatch({ type: "select_project", projectId });
        setAuthed(true);
      })
      .catch(() => {
        clearToken();
        setAuthed(false);
      });
  }, []);

  // Connect WS when authed
  useEffect(() => {
    if (!authed) return;
    const ws = wsRef.current;
    const registry = registryRef.current;

    ws.onPtyData = (streamId, data) => registry.write(streamId, data);
    ws.onJson = handleWsMessage;
    ws.onStatus = (status: WsStatus) => dispatch({ type: "set_ws_status", status });
    ws.connect();

    return () => ws.dispose();
  }, [authed, handleWsMessage]);

  // Token refresh scheduler
  useEffect(() => {
    if (!authed) return;
    const ttl = tokenTtlSecs();
    if (ttl <= 0) return;

    // Refresh at 75% of TTL, or immediately if < 6h remaining
    const refreshIn = ttl < 6 * 3600 ? 1000 : ttl * 0.75 * 1000;
    const timer = setTimeout(async () => {
      try {
        await refresh();
      } catch {
        // Will try again on next cycle
      }
    }, refreshIn);
    return () => clearTimeout(timer);
  }, [authed]);

  const handlePaired = useCallback(() => {
    setAuthed(true);
    fetchState();
  }, [fetchState]);

  if (authed === null) {
    return (
      <div className="flex items-center justify-center h-screen">
        <div className="text-zinc-500">Connecting...</div>
      </div>
    );
  }

  if (!authed) {
    return <PairingScreen onPaired={handlePaired} />;
  }

  return (
    <AppContext.Provider
      value={{
        state,
        dispatch,
        ws: wsRef.current,
        registry: registryRef.current,
        handleWsMessage,
      }}
    >
      <WorkspaceLayout />
    </AppContext.Provider>
  );
}
