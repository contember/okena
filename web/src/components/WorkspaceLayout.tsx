import { useEffect } from "react";
import { useApp } from "../state/store";
import { postAction } from "../api/client";
import { useIsMobile } from "../hooks/useIsMobile";
import { collectTerminalIds } from "../utils/layout";
import { Sidebar } from "./Sidebar";
import { TerminalArea } from "./TerminalArea";
import { TerminalPane } from "./TerminalPane";
import { StatusBar } from "./StatusBar";

export function WorkspaceLayout() {
  const isMobile = useIsMobile();
  return isMobile ? <MobileLayout /> : <DesktopLayout />;
}

function DesktopLayout() {
  const { state } = useApp();
  const project = state.workspace?.projects.find(
    (p) => p.id === state.selectedProjectId,
  );

  return (
    <div className="flex flex-col h-screen">
      <div className="flex flex-1 min-h-0">
        <aside className="w-56 flex-shrink-0 border-r border-zinc-800">
          <Sidebar />
        </aside>
        <main className="flex-1 min-w-0">
          {project?.layout ? (
            <TerminalArea layout={project.layout} project={project} />
          ) : (
            <div className="flex items-center justify-center h-full text-zinc-500">
              {state.workspace ? (
                project ? (
                  <button
                    className="px-4 py-2 rounded bg-zinc-700 hover:bg-zinc-600 text-zinc-200 transition-colors"
                    onClick={() => postAction({ action: "create_terminal", project_id: project.id })}
                  >
                    New Terminal
                  </button>
                ) : (
                  "Select a project"
                )
              ) : (
                "Loading..."
              )}
            </div>
          )}
        </main>
      </div>
      <StatusBar />
    </div>
  );
}

function MobileLayout() {
  const { state, dispatch } = useApp();
  const project = state.workspace?.projects.find(
    (p) => p.id === state.selectedProjectId,
  );

  const terminalIds = project ? collectTerminalIds(project.layout) : [];

  // Auto-select first terminal when project changes or selected terminal disappears
  useEffect(() => {
    if (!project) return;
    const ids = collectTerminalIds(project.layout);
    if (ids.length === 0) {
      dispatch({ type: "select_terminal", terminalId: null });
      return;
    }
    if (!state.selectedTerminalId || !ids.includes(state.selectedTerminalId)) {
      dispatch({ type: "select_terminal", terminalId: ids[0] });
    }
  }, [project, state.selectedTerminalId, dispatch]);

  const selectedTerminalId = state.selectedTerminalId;
  const terminalName = selectedTerminalId && project
    ? project.terminal_names[selectedTerminalId] ?? "Terminal"
    : undefined;

  return (
    <div className="flex flex-col h-screen">
      {/* Hamburger button */}
      <button
        className="absolute top-2 left-2 z-30 p-2 rounded bg-zinc-800 text-zinc-300 hover:bg-zinc-700"
        onClick={() => dispatch({ type: "set_sidebar_open", open: true })}
      >
        <svg width="20" height="20" viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round">
          <line x1="3" y1="5" x2="17" y2="5" />
          <line x1="3" y1="10" x2="17" y2="10" />
          <line x1="3" y1="15" x2="17" y2="15" />
        </svg>
      </button>

      {/* Drawer overlay */}
      {state.sidebarOpen && (
        <>
          <div
            className="fixed inset-0 z-40 bg-black/50"
            onClick={() => dispatch({ type: "set_sidebar_open", open: false })}
          />
          <div className="fixed inset-y-0 left-0 z-50 w-72 border-r border-zinc-800 animate-slide-in-left">
            <Sidebar isMobile />
          </div>
        </>
      )}

      {/* Main terminal area */}
      <main className="flex-1 min-h-0">
        {selectedTerminalId && project ? (
          <TerminalPane
            terminalId={selectedTerminalId}
            name={terminalName}
            projectId={project.id}
            path={[]}
            hideSplitActions
          />
        ) : (
          <div className="flex items-center justify-center h-full text-zinc-500">
            {state.workspace ? (
              project ? (
                terminalIds.length === 0 ? (
                  <button
                    className="px-4 py-2 rounded bg-zinc-700 hover:bg-zinc-600 text-zinc-200 transition-colors"
                    onClick={() => postAction({ action: "create_terminal", project_id: project.id })}
                  >
                    New Terminal
                  </button>
                ) : (
                  "Select a terminal"
                )
              ) : (
                "Open the menu to select a project"
              )
            ) : (
              "Loading..."
            )}
          </div>
        )}
      </main>
      <StatusBar />
    </div>
  );
}
