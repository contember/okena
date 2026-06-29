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
      {/* Tab bar */}
      <div className="flex bg-zinc-900 border-b border-zinc-800 flex-shrink-0">
        {children.map((child, i) => {
          const label = child.type === "terminal" && child.terminal_id
            ? (project.terminal_names[child.terminal_id] ?? `Terminal ${i + 1}`)
            : `Tab ${i + 1}`;
          return (
            <button
              key={i}
              onClick={() => selectTab(i)}
              className={`px-3 py-1.5 text-xs truncate max-w-32 transition-colors
                ${i === clamped
                  ? "bg-zinc-800 text-zinc-100 border-b-2 border-blue-500"
                  : "text-zinc-500 hover:text-zinc-300 hover:bg-zinc-800/50"
                }`}
            >
              {label}
            </button>
          );
        })}
        <button
          onClick={addTab}
          className="ml-auto px-2 py-1.5 text-xs text-zinc-500 hover:text-zinc-200 hover:bg-zinc-800 transition-colors"
          title="New tab"
        >
          +
        </button>
      </div>

      {/* Active tab content */}
      <div className="flex-1 min-h-0">
        {children[clamped] && (
          <LayoutRenderer node={children[clamped]} project={project} path={[...path, clamped]} />
        )}
      </div>
    </div>
  );
}
