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
  const isHorizontalSplit = direction === "horizontal";
  const containerRef = useRef<HTMLDivElement>(null);
  const [localSizes, setLocalSizes] = useState<number[] | null>(null);
  const [activeDivider, setActiveDivider] = useState<number | null>(null);
  const draggingRef = useRef(false);
  const liveSizesRef = useRef<number[]>(serverSizes);

  const sizes = localSizes ?? serverSizes;
  const total = sizes.reduce((a, b) => a + b, 0) || 1;
  liveSizesRef.current = sizes;

  const handlePointerDown = useCallback(
    (event: React.PointerEvent<HTMLDivElement>, dividerIndex: number) => {
      event.preventDefault();
      event.stopPropagation();
      const container = containerRef.current;
      if (!container) return;

      const rect = container.getBoundingClientRect();
      const containerSize = isHorizontalSplit ? rect.height : rect.width;
      if (containerSize <= 0) return;

      const startPos = isHorizontalSplit ? event.clientY : event.clientX;
      const currentSizes = [...(localSizes ?? serverSizes)];
      const currentTotal = currentSizes.reduce((a, b) => a + b, 0) || 1;
      const pointerId = event.pointerId;

      draggingRef.current = true;
      setActiveDivider(dividerIndex);
      event.currentTarget.setPointerCapture(pointerId);
      document.body.style.userSelect = "none";
      document.body.style.cursor = isHorizontalSplit ? "row-resize" : "col-resize";

      const onPointerMove = (ev: PointerEvent) => {
        if (ev.pointerId !== pointerId) return;
        const currentPos = isHorizontalSplit ? ev.clientY : ev.clientX;
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
        liveSizesRef.current = next;
        setLocalSizes(next);
      };

      const onPointerUp = (ev: PointerEvent) => {
        if (ev.pointerId !== pointerId) return;
        window.removeEventListener("pointermove", onPointerMove);
        window.removeEventListener("pointerup", onPointerUp);
        window.removeEventListener("pointercancel", onPointerUp);
        document.body.style.userSelect = "";
        document.body.style.cursor = "";
        draggingRef.current = false;
        setActiveDivider(null);

        postAction({
          action: "update_split_sizes",
          project_id: project.id,
          path,
          sizes: liveSizesRef.current,
        }).catch(() => {});
      };

      window.addEventListener("pointermove", onPointerMove);
      window.addEventListener("pointerup", onPointerUp);
      window.addEventListener("pointercancel", onPointerUp);
    },
    [isHorizontalSplit, serverSizes, localSizes, project.id, path],
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
      className={`flex h-full ${isHorizontalSplit ? "flex-col" : "flex-row"}`}
    >
      {children.map((child, i) => (
        <div key={i} className="contents">
          {i > 0 && (
            <div
              className={`split-handle ${
                isHorizontalSplit ? "split-handle-vertical" : "split-handle-horizontal"
              }`}
              data-active={activeDivider === i - 1}
              role="separator"
              aria-orientation={isHorizontalSplit ? "horizontal" : "vertical"}
              onPointerDown={(event) => handlePointerDown(event, i - 1)}
            />
          )}
          <div
            className="min-w-0 min-h-0 overflow-hidden"
            style={{
              flexBasis: 0,
              flexGrow: (sizes[i] ?? 1) / total,
              flexShrink: 1,
            }}
          >
            <LayoutRenderer node={child} project={project} path={[...path, i]} />
          </div>
        </div>
      ))}
    </div>
  );
}
