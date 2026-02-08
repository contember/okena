import { useCallback, useRef, useState } from "react";
import type { ApiLayoutNode, ApiProject, SplitDirection } from "../api/types";
import { postAction } from "../api/client";
import { LayoutRenderer } from "./TerminalArea";

const MIN_PERCENT = 5;

export function SplitLayout({
  direction,
  sizes: serverSizes,
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
  const isHorizontal = direction === "horizontal";
  const containerRef = useRef<HTMLDivElement>(null);
  const [localSizes, setLocalSizes] = useState<number[] | null>(null);
  const draggingRef = useRef(false);

  const sizes = localSizes ?? serverSizes;
  const total = sizes.reduce((a, b) => a + b, 0) || 1;

  const handleMouseDown = useCallback(
    (e: React.MouseEvent, dividerIndex: number) => {
      e.preventDefault();
      const container = containerRef.current;
      if (!container) return;

      const rect = container.getBoundingClientRect();
      const containerSize = isHorizontal ? rect.width : rect.height;
      if (containerSize <= 0) return;

      const startPos = isHorizontal ? e.clientX : e.clientY;
      const currentSizes = [...(localSizes ?? serverSizes)];
      const currentTotal = currentSizes.reduce((a, b) => a + b, 0) || 1;

      draggingRef.current = true;
      document.body.style.userSelect = "none";
      document.body.style.cursor = isHorizontal ? "col-resize" : "row-resize";

      const onMouseMove = (ev: MouseEvent) => {
        const currentPos = isHorizontal ? ev.clientX : ev.clientY;
        const deltaPx = currentPos - startPos;
        const deltaPercent = (deltaPx / containerSize) * currentTotal;

        const leftIdx = dividerIndex;
        const rightIdx = dividerIndex + 1;
        let newLeft = currentSizes[leftIdx] + deltaPercent;
        let newRight = currentSizes[rightIdx] - deltaPercent;

        // Clamp both sides to minimum
        const minVal = (MIN_PERCENT / 100) * currentTotal;
        if (newLeft < minVal) {
          newRight += newLeft - minVal;
          newLeft = minVal;
        }
        if (newRight < minVal) {
          newLeft += newRight - minVal;
          newRight = minVal;
        }

        const next = [...currentSizes];
        next[leftIdx] = newLeft;
        next[rightIdx] = newRight;
        setLocalSizes(next);
      };

      const onMouseUp = () => {
        document.removeEventListener("mousemove", onMouseMove);
        document.removeEventListener("mouseup", onMouseUp);
        document.body.style.userSelect = "";
        document.body.style.cursor = "";
        draggingRef.current = false;

        // Persist to server
        setLocalSizes((final_) => {
          if (final_) {
            postAction({
              action: "update_split_sizes",
              project_id: project.id,
              path,
              sizes: final_,
            }).catch(() => {});
          }
          return final_;
        });
      };

      document.addEventListener("mousemove", onMouseMove);
      document.addEventListener("mouseup", onMouseUp);
    },
    [isHorizontal, serverSizes, localSizes, project.id, path],
  );

  // Sync local sizes with server when not dragging
  // (server pushes new sizes after persist or from another client)
  const prevServerRef = useRef(serverSizes);
  if (prevServerRef.current !== serverSizes) {
    prevServerRef.current = serverSizes;
    if (!draggingRef.current) {
      if (localSizes !== null) setLocalSizes(null);
    }
  }

  return (
    <div
      ref={containerRef}
      className={`flex h-full ${isHorizontal ? "flex-row" : "flex-col"}`}
    >
      {children.map((child, i) => (
        <div key={i} className="contents">
          {i > 0 && (
            <div
              className={`flex-none ${
                isHorizontal
                  ? "w-1 cursor-col-resize hover:bg-blue-500"
                  : "h-1 cursor-row-resize hover:bg-blue-500"
              } bg-zinc-700 transition-colors`}
              onMouseDown={(e) => handleMouseDown(e, i - 1)}
            />
          )}
          <div
            className="min-w-0 min-h-0 overflow-hidden"
            style={{ flex: `${(sizes[i] ?? 1) / total}` }}
          >
            <LayoutRenderer node={child} project={project} path={[...path, i]} />
          </div>
        </div>
      ))}
    </div>
  );
}
