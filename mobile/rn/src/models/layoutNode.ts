/**
 * layoutNode.ts — the project layout tree.
 *
 * Ported from `mobile/lib/src/models/layout_node.dart` (the sealed
 * `LayoutNode` hierarchy). This mirrors the server's `ApiLayoutNode`, which is
 * delivered as a JSON string by `OkenaNative.getProjectLayoutJson()`.
 *
 * {@link parseLayout} matches the Dart parser exactly: unknown node types and
 * malformed JSON yield `null`; missing fields fall back to the same defaults
 * the Dart code used.
 *
 * The layout renderer (a later screen agent) walks this tree:
 *   - `TerminalNode` → a {@link import('../components/TerminalView').TerminalView}
 *   - `SplitNode`    → a flex row/column
 *   - `TabsNode`     → a tab bar + the active child
 */

import type { SplitDirection } from '../native/okena';

/** Discriminator for a {@link LayoutNode}. */
export type LayoutNodeType = 'terminal' | 'split' | 'tabs';

/**
 * A leaf node hosting a single terminal. `terminalId` can be `undefined` for an
 * empty/placeholder pane (matches the nullable `terminal_id` Dart-side).
 */
export interface TerminalNode {
  readonly type: 'terminal';
  readonly terminalId?: string;
  readonly minimized: boolean;
  readonly detached: boolean;
}

/**
 * A split container. `sizes` are the fractional weights of each child along the
 * split `direction` (parallel arrays with `children`).
 */
export interface SplitNode {
  readonly type: 'split';
  readonly direction: SplitDirection;
  readonly sizes: number[];
  readonly children: LayoutNode[];
}

/** A tab group; `activeTab` indexes into `children`. */
export interface TabsNode {
  readonly type: 'tabs';
  readonly activeTab: number;
  readonly children: LayoutNode[];
}

/** The discriminated union — narrow on `.type`. */
export type LayoutNode = TerminalNode | SplitNode | TabsNode;

/**
 * Parse one raw JSON object into a {@link LayoutNode}, or `null` if its `type`
 * is unknown. Mirrors the private `_parse` in `layout_node.dart`, including its
 * `whereType<LayoutNode>()` behavior: children that fail to parse are dropped.
 */
function parseNode(map: Record<string, unknown>): LayoutNode | null {
  const type = map.type;
  switch (type) {
    case 'terminal':
      return {
        type: 'terminal',
        terminalId:
          typeof map.terminal_id === 'string'
            ? (map.terminal_id as string)
            : undefined,
        minimized: map.minimized === true,
        detached: map.detached === true,
      };
    case 'split': {
      const children = parseChildren(map.children);
      const rawSizes = map.sizes;
      const sizes = Array.isArray(rawSizes)
        ? rawSizes.filter((s): s is number => typeof s === 'number')
        : [];
      return {
        type: 'split',
        direction: map.direction === 'vertical' ? 'vertical' : 'horizontal',
        sizes,
        children,
      };
    }
    case 'tabs': {
      const children = parseChildren(map.children);
      const activeTab =
        typeof map.active_tab === 'number' ? (map.active_tab as number) : 0;
      return { type: 'tabs', activeTab, children };
    }
    default:
      return null;
  }
}

/** Parse a raw `children` array, dropping any that fail to parse (Dart `whereType`). */
function parseChildren(raw: unknown): LayoutNode[] {
  if (!Array.isArray(raw)) return [];
  const out: LayoutNode[] = [];
  for (const child of raw) {
    if (child && typeof child === 'object') {
      const node = parseNode(child as Record<string, unknown>);
      if (node) out.push(node);
    }
  }
  return out;
}

/**
 * Parse the layout JSON string returned by `getProjectLayoutJson()` into a
 * {@link LayoutNode} tree. Returns `null` on invalid JSON, a non-object root, or
 * an unknown root node type — matching the Dart `LayoutNode.fromJson`, which
 * caught all errors and returned `null`.
 */
export function parseLayout(json: string): LayoutNode | null {
  try {
    const parsed: unknown = JSON.parse(json);
    if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
      return null;
    }
    return parseNode(parsed as Record<string, unknown>);
  } catch {
    return null;
  }
}
