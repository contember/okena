import { useCallback, useEffect, useState } from "react";
import type { ApiLayoutNode, ApiProject } from "../api/types";
import { postAction } from "../api/client";
import { LayoutRenderer } from "./TerminalArea";

function firstTerminalId(node: ApiLayoutNode): string | null {
  if (node.type === "terminal") return node.terminal_id;
  for (const child of node.children) {
    const terminalId = firstTerminalId(child);
    if (terminalId) return terminalId;
  }
  return null;
}

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
  const initialIdx = Math.min(initialActive, Math.max(children.length - 1, 0));
  const [activeTerminalId, setActiveTerminalId] = useState(() =>
    children[initialIdx] ? firstTerminalId(children[initialIdx]) : null,
  );
  const identityIdx = activeTerminalId
    ? children.findIndex((child) => firstTerminalId(child) === activeTerminalId)
    : -1;
  const clamped = identityIdx >= 0 ? identityIdx : initialIdx;

  useEffect(() => {
    if (identityIdx < 0) {
      setActiveTerminalId(children[initialIdx] ? firstTerminalId(children[initialIdx]) : null);
    }
  }, [children, identityIdx, initialIdx]);

  const selectTab = useCallback(
    (index: number) => {
      setActiveTerminalId(children[index] ? firstTerminalId(children[index]) : null);
    },
    [children],
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
              key={firstTerminalId(child) ?? `tab-${i}`}
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
