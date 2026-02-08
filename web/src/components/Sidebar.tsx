import { useApp } from "../state/store";
import { SidebarProject } from "./SidebarProject";

export function Sidebar({ isMobile = false }: { isMobile?: boolean }) {
  const { state } = useApp();
  const projects = state.workspace?.projects ?? [];

  return (
    <div className="bg-zinc-900 overflow-y-auto h-full">
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
              isMobile={isMobile}
            />
          ))}
        </div>
      </div>
    </div>
  );
}
