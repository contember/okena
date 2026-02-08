import { useState } from "react";
import type { ApiLayoutNode, ApiProject } from "../api/types";
import { LayoutRenderer } from "./TerminalArea";

export function TabLayout({
  activeTab: initialActive,
  children,
  project,
}: {
  activeTab: number;
  children: ApiLayoutNode[];
  project: ApiProject;
}) {
  const [activeIdx, setActiveIdx] = useState(initialActive);
  const clamped = Math.min(activeIdx, children.length - 1);

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
              onClick={() => setActiveIdx(i)}
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
      </div>

      {/* Active tab content */}
      <div className="flex-1 min-h-0">
        {children[clamped] && (
          <LayoutRenderer node={children[clamped]} project={project} />
        )}
      </div>
    </div>
  );
}
