/**
 * sessionRestore.ts — glue between the session memory and the live stores.
 *
 * Kept out of App.tsx so the wiring is unit-testable with injected stores.
 */

import { connectionStore, selectIsConnected } from './connectionStore';
import { findRestoreServer, sessionStore } from './sessionStore';
import { workspaceStore } from './workspaceStore';

/**
 * Keep the session memory current: record the selected project / terminal of
 * the connected server, and forget the auto-reconnect target when the user
 * disconnects from a live connection (a dropped connection keeps it). Returns
 * an unsubscribe fn.
 */
export function bindSessionMemory(): () => void {
  const conn = connectionStore();
  const session = sessionStore();

  const unbindWs = workspaceStore().subscribe((ws, prev) => {
    if (
      ws.selectedProjectId === prev.selectedProjectId &&
      ws.selectedTerminalId === prev.selectedTerminalId &&
      ws.terminalByProject === prev.terminalByProject
    ) {
      return;
    }
    const c = conn.getState();
    // The screen falls back to the first project while nothing is explicitly
    // selected (no focused project on the server), so record what is shown.
    const project = ws.getSelectedProject();
    if (!selectIsConnected(c) || !c.activeServer || project === null) return;
    session.getState().recordSelection(c.activeServer, {
      projectId: project.id,
      terminalByProject: ws.terminalByProject,
    });
  });
  const unbindConn = conn.subscribe((c, prev) => {
    if (
      c.activeServer === null &&
      prev.activeServer !== null &&
      selectIsConnected(prev)
    ) {
      session.getState().clearAutoReconnect();
    }
  });
  return () => {
    unbindWs();
    unbindConn();
  };
}

/**
 * Load the saved servers + session memory, then reconnect to the server the
 * user had open last (if it is still saved and paired). Skipped when the user
 * already picked a server in the meantime.
 */
export async function loadAndRestore(): Promise<void> {
  const conn = connectionStore();
  await Promise.all([
    conn.getState().loadServers(),
    sessionStore().getState().load(),
  ]);
  const state = conn.getState();
  if (state.activeServer !== null) return;
  const server = findRestoreServer(
    state.servers,
    sessionStore().getState().memory,
  );
  if (server) state.connectTo(server);
}
