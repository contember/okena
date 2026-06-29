import { useApp } from "../state/store";
import { SidebarProject } from "./SidebarProject";
import { buildSidebarItems } from "../utils/sidebar";

export function Sidebar({ isMobile = false }: { isMobile?: boolean }) {
  const { state } = useApp();
  const items = buildSidebarItems(state.workspace);

  return (
    <div className="h-full overflow-y-auto bg-[var(--ok-panel)]">
      <div className="border-b border-[var(--ok-border)] px-3 py-3">
        <div className="text-[13px] font-bold leading-4 text-[var(--ok-text)]">Okena</div>
        <div className="mt-1 text-[10px] text-[var(--ok-text-muted)]">remote workspace</div>
      </div>
      <div className="px-2 py-2">
        <h2 className="mb-2 px-1 text-[10px] font-bold text-[var(--ok-text-muted)]">
          projects
        </h2>
        <div className="space-y-0.5">
          {items.map((item) => {
            if (item.type === "folder") {
              return (
                <div key={item.folder.id} className="space-y-0.5">
                  <div className="flex items-center gap-1.5 px-2 py-1 text-[11px] font-medium text-[var(--ok-text-secondary)]">
                    <span className="truncate">{item.folder.name}</span>
                    <span className="ml-auto text-[var(--ok-text-muted)]">{item.projects.length}</span>
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
