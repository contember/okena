import { useCallback, useState } from "react";
import type { ApiProject } from "../api/types";
import { useApp } from "../state/store";
import { postAction } from "../api/client";
import { collectTerminalIds } from "../utils/layout";

export function SidebarProject({
  project,
  selected,
  isMobile,
}: {
  project: ApiProject;
  selected: boolean;
  isMobile: boolean;
}) {
  const { state, dispatch } = useApp();
  const [expanded, setExpanded] = useState(selected);

  const terminalIds = collectTerminalIds(project.layout);

  const handleProjectClick = useCallback(() => {
    dispatch({ type: "select_project", projectId: project.id });
    setExpanded((prev) => !prev);
  }, [dispatch, project.id]);

  const handleTerminalClick = useCallback(
    (terminalId: string) => {
      dispatch({ type: "select_project", projectId: project.id });
      dispatch({ type: "select_terminal", terminalId });
      if (isMobile) {
        dispatch({ type: "set_sidebar_open", open: false });
      } else {
        postAction({ action: "focus_terminal", project_id: project.id, terminal_id: terminalId }).catch(() => {});
      }
    },
    [dispatch, project.id, isMobile],
  );

  const handleCreateTerminal = useCallback(
    (e: React.MouseEvent) => {
      e.stopPropagation();
      postAction({ action: "create_terminal", project_id: project.id }).catch(() => {});
    },
    [project.id],
  );

  const isExpanded = expanded || selected;

  return (
    <div>
      <button
        onClick={handleProjectClick}
        className={`w-full text-left px-2 py-1.5 rounded text-sm truncate transition-colors flex items-center gap-1
          ${selected ? "bg-zinc-700 text-zinc-100" : "text-zinc-400 hover:bg-zinc-800 hover:text-zinc-200"}`}
      >
        <span
          className="text-[10px] transition-transform duration-150 flex-shrink-0"
          style={{ transform: isExpanded ? "rotate(90deg)" : "rotate(0deg)" }}
        >
          ▶
        </span>
        <span className="truncate">{project.name}</span>
        <button
          onClick={handleCreateTerminal}
          className="ml-auto flex-shrink-0 p-0.5 text-zinc-500 hover:text-zinc-300 hover:bg-zinc-600 rounded text-xs leading-none"
          title="New terminal"
        >
          +
        </button>
      </button>

      {isExpanded && terminalIds.length > 0 && (
        <div className="ml-4 mt-0.5 space-y-0.5">
          {terminalIds.map((tid) => {
            const name = project.terminal_names[tid] ?? "Terminal";
            const isSelected = tid === state.selectedTerminalId && selected;
            return (
              <button
                key={tid}
                onClick={() => handleTerminalClick(tid)}
                className={`w-full text-left px-2 py-1 rounded text-xs truncate transition-colors
                  ${isSelected ? "bg-zinc-600 text-zinc-100" : "text-zinc-500 hover:bg-zinc-800 hover:text-zinc-300"}`}
              >
                {name}
              </button>
            );
          })}
        </div>
      )}
    </div>
  );
}
