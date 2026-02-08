import { useCallback } from "react";
import type { ApiProject } from "../api/types";
import { useApp } from "../state/store";

export function SidebarProject({
  project,
  selected,
}: {
  project: ApiProject;
  selected: boolean;
}) {
  const { dispatch } = useApp();

  const handleClick = useCallback(() => {
    dispatch({ type: "select_project", projectId: project.id });
  }, [dispatch, project.id]);

  return (
    <button
      onClick={handleClick}
      className={`w-full text-left px-2 py-1.5 rounded text-sm truncate transition-colors
        ${selected ? "bg-zinc-700 text-zinc-100" : "text-zinc-400 hover:bg-zinc-800 hover:text-zinc-200"}`}
    >
      {project.name}
    </button>
  );
}
