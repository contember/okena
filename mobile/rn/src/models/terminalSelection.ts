/**
 * terminalSelection.ts — which single terminal the phone shows.
 *
 * The mobile client renders one terminal at a time instead of the desktop's
 * split layout. These pure helpers pick that terminal, label it for the
 * switcher, and locate it in the layout tree so layout actions (split) still
 * target the terminal the user is looking at. Nothing here changes the
 * server-side layout.
 */

import type { ProjectId, ProjectInfo, TerminalId } from '../native/okena';
import type { LayoutNode } from './layoutNode';

/**
 * The first candidate that is still one of `terminalIds`, else the first
 * terminal, else `null`. Candidates are tried in order and may be missing.
 */
export function pickTerminal(
  terminalIds: readonly TerminalId[],
  candidates: readonly (TerminalId | null | undefined)[] = [],
): TerminalId | null {
  for (const candidate of candidates) {
    if (candidate && terminalIds.includes(candidate)) return candidate;
  }
  return terminalIds[0] ?? null;
}

/** Display name for a terminal: its custom name, else `Terminal N` (1-based, layout order). */
export function terminalLabel(project: ProjectInfo, terminalId: TerminalId): string {
  const name = project.terminalNames[terminalId];
  if (name) return name;
  const index = project.terminalIds.indexOf(terminalId);
  return `Terminal ${index >= 0 ? index + 1 : project.terminalIds.length + 1}`;
}

/**
 * Child-index path from the layout root to the leaf hosting `terminalId`, or
 * `null` when the terminal is not in the tree. This is the path the layout
 * actions (`splitTerminal`, …) expect.
 */
export function findTerminalPath(node: LayoutNode, terminalId: TerminalId): number[] | null {
  if (node.type === 'terminal') return node.terminalId === terminalId ? [] : null;
  for (let i = 0; i < node.children.length; i++) {
    const sub = findTerminalPath(node.children[i]!, terminalId);
    if (sub) return [i, ...sub];
  }
  return null;
}

/** A remembered selection to reopen (see `state/sessionStore.ts`). */
export interface SelectionMemory {
  projectId: ProjectId | null;
  terminalByProject: Record<ProjectId, TerminalId>;
}

/** Outcome of {@link resolveRestore}. */
export type RestoreOutcome =
  | { kind: 'restored'; projectId: ProjectId; terminalId: TerminalId | null }
  | { kind: 'projectMissing' };

/**
 * Reopen a remembered selection against the live project list: the remembered
 * project with its remembered terminal, falling back to the project's first
 * terminal. `projectMissing` when the project is gone (the UI then shows the
 * project list); `null` when nothing was remembered.
 */
export function resolveRestore(
  projects: readonly ProjectInfo[],
  memory: SelectionMemory,
): RestoreOutcome | null {
  if (memory.projectId === null) return null;
  const project = projects.find((p) => p.id === memory.projectId);
  if (!project) return { kind: 'projectMissing' };
  return {
    kind: 'restored',
    projectId: project.id,
    terminalId: pickTerminal(project.terminalIds, [memory.terminalByProject[project.id]]),
  };
}
