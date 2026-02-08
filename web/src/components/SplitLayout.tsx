import type { ApiLayoutNode, ApiProject, SplitDirection } from "../api/types";
import { LayoutRenderer } from "./TerminalArea";

export function SplitLayout({
  direction,
  sizes,
  children,
  project,
  path,
}: {
  direction: SplitDirection;
  sizes: number[];
  children: ApiLayoutNode[];
  project: ApiProject;
  path: number[];
}) {
  const total = sizes.reduce((a, b) => a + b, 0) || 1;

  return (
    <div
      className={`flex h-full ${direction === "horizontal" ? "flex-row" : "flex-col"}`}
    >
      {children.map((child, i) => (
        <div
          key={i}
          className="min-w-0 min-h-0 overflow-hidden"
          style={{ flex: `${(sizes[i] ?? 1) / total}` }}
        >
          <LayoutRenderer node={child} project={project} path={[...path, i]} />
        </div>
      ))}
    </div>
  );
}
