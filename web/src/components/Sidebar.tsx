import { useApp } from "../state/store";
import { SidebarProject } from "./SidebarProject";
import { buildSidebarItems } from "../utils/sidebar";

export function Sidebar({ isMobile = false }: { isMobile?: boolean }) {
  const { state } = useApp();
  const items = buildSidebarItems(state.workspace);

  return (
    <div className="bg-zinc-900 overflow-y-auto h-full">
      <div className="px-3 py-3">
        <h2 className="text-xs font-semibold text-zinc-500 uppercase tracking-wider mb-2">
          Projects
        </h2>
        <div className="space-y-0.5">
          {items.map((item) => {
            if (item.type === "folder") {
              return (
                <div key={item.folder.id} className="space-y-0.5">
                  <div className="flex items-center gap-1.5 px-2 py-1 text-[11px] font-medium text-zinc-500">
                    <span className="truncate">{item.folder.name}</span>
                    <span className="ml-auto text-zinc-600">{item.projects.length}</span>
                  </div>
                  {item.projects.map((node) => (
                    <SidebarProject
                      key={node.project.id}
                      project={node.project}
                      worktrees={node.worktrees}
                      selected={node.project.id === state.selectedProjectId}
                      isMobile={isMobile}
                      depth={1}
                    />
                  ))}
                </div>
              );
            }

            return (
              <SidebarProject
                key={item.project.id}
                project={item.project}
                worktrees={item.worktrees}
                selected={item.project.id === state.selectedProjectId}
                isMobile={isMobile}
              />
            );
          })}
        </div>
      </div>
    </div>
  );
}
