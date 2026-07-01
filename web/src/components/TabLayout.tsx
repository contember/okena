import { useCallback, useEffect, useState } from "react";
import type { ApiLayoutNode, ApiProject } from "../api/types";
import { postAction } from "../api/client";
import { LayoutRenderer } from "./TerminalArea";

export function TabLayout({
  activeTab: initialActive,
  children,
  project,
  path,
}: {
  activeTab: number;
  children: ApiLayoutNode[];
  project: ApiProject;
  path: number[];
}) {
  const [activeIdx, setActiveIdx] = useState(initialActive);
  const clamped = Math.min(activeIdx, children.length - 1);

  useEffect(() => {
    setActiveIdx(initialActive);
  }, [initialActive]);

  const selectTab = useCallback(
    (index: number) => {
      setActiveIdx(index);
      postAction({
        action: "set_active_tab",
        project_id: project.id,
        path,
        index,
      }).catch(() => {});
    },
    [project.id, path],
  );

  const addTab = useCallback(() => {
    postAction({
      action: "add_tab",
      project_id: project.id,
      path,
      in_group: true,
    }).catch(() => {});
  }, [project.id, path]);

  return (
    <div className="flex flex-col h-full">
      <div className="terminal-header flex flex-shrink-0 border-b">
        {children.map((child, i) => {
          const label = child.type === "terminal" && child.terminal_id
            ? (project.terminal_names[child.terminal_id] ?? `Terminal ${i + 1}`)
            : `Tab ${i + 1}`;
          return (
            <button
              key={i}
              onClick={() => selectTab(i)}
              className={`max-w-32 truncate border-r border-[var(--ok-border)] px-3 py-1.5 text-[11px] transition-colors
                ${i === clamped
                  ? "bg-[var(--ok-selection)] text-white"
                  : "text-[var(--ok-text-muted)] hover:bg-[var(--ok-hover)] hover:text-[var(--ok-text)]"
                }`}
            >
              {label}
            </button>
          );
        })}
        <button
          onClick={addTab}
          className="icon-button ml-auto h-[30px] w-[30px]"
          title="New tab"
          aria-label="New tab"
        >
          +
        </button>
      </div>

      <div className="flex-1 min-h-0">
        {children[clamped] && (
          <LayoutRenderer node={children[clamped]} project={project} path={[...path, clamped]} />
        )}
      </div>
    </div>
  );
}
