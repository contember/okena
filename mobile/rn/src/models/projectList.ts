/**
 * projectList.ts — the project list the drawer and the switcher render.
 *
 * Mirrors the desktop sidebar's manual view (`ProjectSortMode::Manual`, the
 * default; `crates/okena-views-sidebar/src/sidebar/render.rs`):
 *   - walk `projectOrder`; an entry is either a folder (its projects in
 *     `folder.projectIds` order) or a top-level project,
 *   - a worktree whose parent exists is not listed on its own; it is nested
 *     under the parent in the parent's `worktreeIds` order and inherits the
 *     parent's color,
 *   - a worktree whose parent is gone (orphan) is listed like a normal project.
 * Unlike the desktop, projects missing from `projectOrder` are appended at the
 * end so they stay reachable on mobile.
 *
 * The activity sort mode is per desktop window and is not sent over the wire,
 * so mobile always uses the manual order.
 */

import type { FolderInfo, ProjectInfo } from '../native/okena';

/** A listed project plus its nested worktrees. */
export interface ProjectNode {
  project: ProjectInfo;
  worktrees: ProjectInfo[];
}

export type ProjectListItem =
  | { kind: 'folder'; folder: FolderInfo; projects: ProjectNode[] }
  | { kind: 'project'; node: ProjectNode };

/** One row of the flattened list (for the compact switcher). */
export type ProjectListRow =
  | { kind: 'folder'; folder: FolderInfo }
  | {
      kind: 'project';
      project: ProjectInfo;
      /** Nesting depth: 0 top level, +1 inside a folder, +1 for a worktree. */
      depth: number;
    };

/**
 * Name the desktop shows for a project: worktrees show their git branch
 * (`SidebarProjectInfo::from_project`).
 */
export function projectDisplayName(project: ProjectInfo): string {
  if (project.parentProjectId && project.gitBranch) return project.gitBranch;
  return project.name;
}

export function buildProjectList(
  projects: ProjectInfo[],
  folders: FolderInfo[],
  projectOrder: string[],
): ProjectListItem[] {
  const projectMap = new Map(projects.map((p) => [p.id, p]));
  const folderMap = new Map(folders.map((f) => [f.id, f]));

  const isNestedWorktree = (p: ProjectInfo): boolean =>
    p.parentProjectId !== undefined && projectMap.has(p.parentProjectId);

  const listed = new Set<string>();

  const toNode = (project: ProjectInfo): ProjectNode => {
    listed.add(project.id);
    const worktrees: ProjectInfo[] = [];
    for (const wtId of project.worktreeIds) {
      const wt = projectMap.get(wtId);
      if (wt && !listed.has(wt.id)) {
        listed.add(wt.id);
        worktrees.push({ ...wt, folderColor: project.folderColor });
      }
    }
    return { project, worktrees };
  };

  const items: ProjectListItem[] = [];
  for (const entryId of projectOrder) {
    const folder = folderMap.get(entryId);
    if (folder) {
      const nodes: ProjectNode[] = [];
      for (const pid of folder.projectIds) {
        const p = projectMap.get(pid);
        if (p && !isNestedWorktree(p) && !listed.has(p.id)) nodes.push(toNode(p));
      }
      items.push({ kind: 'folder', folder, projects: nodes });
      continue;
    }
    const project = projectMap.get(entryId);
    if (project && !isNestedWorktree(project) && !listed.has(project.id)) {
      items.push({ kind: 'project', node: toNode(project) });
    }
  }

  // Safety net: anything the order did not reach.
  for (const p of projects) {
    if (!listed.has(p.id) && !isNestedWorktree(p)) {
      items.push({ kind: 'project', node: toNode(p) });
    }
  }
  // Worktrees whose parent was listed but does not reference them.
  for (const p of projects) {
    if (!listed.has(p.id)) items.push({ kind: 'project', node: toNode(p) });
  }

  return items;
}

function matches(project: ProjectInfo, needle: string): boolean {
  return (
    project.name.toLowerCase().includes(needle) ||
    projectDisplayName(project).toLowerCase().includes(needle) ||
    project.path.toLowerCase().includes(needle)
  );
}

/**
 * Case-insensitive substring filter on project name (or the displayed branch
 * for worktrees) and path. A parent stays when it or one of its worktrees
 * matches; only matching worktrees are kept. A folder whose name matches keeps
 * all of its projects. Empty or whitespace query returns the list unchanged.
 */
export function filterProjectList(items: ProjectListItem[], query: string): ProjectListItem[] {
  const needle = query.trim().toLowerCase();
  if (needle.length === 0) return items;

  const filterNode = (node: ProjectNode): ProjectNode | null => {
    const worktrees = node.worktrees.filter((wt) => matches(wt, needle));
    if (worktrees.length > 0 || matches(node.project, needle)) {
      return { project: node.project, worktrees };
    }
    return null;
  };

  const out: ProjectListItem[] = [];
  for (const item of items) {
    if (item.kind === 'folder') {
      if (item.folder.name.toLowerCase().includes(needle)) {
        out.push(item);
        continue;
      }
      const projects = item.projects
        .map(filterNode)
        .filter((n): n is ProjectNode => n !== null);
      if (projects.length > 0) out.push({ ...item, projects });
    } else {
      const node = filterNode(item.node);
      if (node) out.push({ kind: 'project', node });
    }
  }
  return out;
}

/** Flatten the list into display rows (folder headers + projects), in order. */
export function flattenProjectList(items: ProjectListItem[]): ProjectListRow[] {
  const rows: ProjectListRow[] = [];
  const pushNode = (node: ProjectNode, depth: number) => {
    rows.push({ kind: 'project', project: node.project, depth });
    for (const wt of node.worktrees) rows.push({ kind: 'project', project: wt, depth: depth + 1 });
  };
  for (const item of items) {
    if (item.kind === 'folder') {
      rows.push({ kind: 'folder', folder: item.folder });
      for (const node of item.projects) pushNode(node, 1);
    } else {
      pushNode(item.node, 0);
    }
  }
  return rows;
}
