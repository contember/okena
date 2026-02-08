import { useApp } from "../state/store";
import { postAction } from "../api/client";
import { Sidebar } from "./Sidebar";
import { TerminalArea } from "./TerminalArea";
import { StatusBar } from "./StatusBar";

export function WorkspaceLayout() {
  const { state } = useApp();
  const project = state.workspace?.projects.find(
    (p) => p.id === state.selectedProjectId,
  );

  return (
    <div className="flex flex-col h-screen">
      <div className="flex flex-1 min-h-0">
        <Sidebar />
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
