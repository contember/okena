import type { ApiLayoutNode } from "../api/types";

/** Recursively collect all non-null terminal IDs from a layout tree. */
export function collectTerminalIds(node: ApiLayoutNode | null): string[] {
  if (!node) return [];
  switch (node.type) {
    case "terminal":
      return node.terminal_id ? [node.terminal_id] : [];
    case "split":
    case "tabs":
      return node.children.flatMap(collectTerminalIds);
  }
}
