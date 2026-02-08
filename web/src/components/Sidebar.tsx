import { useApp } from "../state/store";
import { SidebarProject } from "./SidebarProject";

export function Sidebar() {
  const { state } = useApp();
  const projects = state.workspace?.projects ?? [];

  return (
    <aside className="w-56 flex-shrink-0 bg-zinc-900 border-r border-zinc-800 overflow-y-auto">
      <div className="px-3 py-3">
        <h2 className="text-xs font-semibold text-zinc-500 uppercase tracking-wider mb-2">
          Projects
        </h2>
        <div className="space-y-0.5">
          {projects.map((p) => (
            <SidebarProject
              key={p.id}
              project={p}
              selected={p.id === state.selectedProjectId}
            />
          ))}
        </div>
      </div>
    </aside>
  );
}
