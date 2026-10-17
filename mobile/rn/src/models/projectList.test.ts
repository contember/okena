/// <reference types="jest" />
// tsconfig pins `types` to react/react-native, so pull jest globals in here.

/**
 * Project list ordering mirrors the desktop sidebar's manual view, and the
 * search filter matches name / path case-insensitively.
 */

import type { FolderInfo, ProjectInfo } from '../native/okena';
import {
  buildProjectList,
  filterProjectList,
  flattenProjectList,
  projectDisplayName,
  type ProjectListItem,
} from './projectList';

function project(id: string, extra: Partial<ProjectInfo> = {}): ProjectInfo {
  return {
    id,
    name: id,
    path: `/home/me/${id}`,
    showInOverview: true,
    terminalIds: [],
    terminalNames: {},
    gitLinesAdded: 0,
    gitLinesRemoved: 0,
    services: [],
    folderColor: 'default',
    worktreeIds: [],
    terminalAgentStatus: {},
    pinned: false,
    ...extra,
  };
}

function folder(id: string, projectIds: string[], name = id): FolderInfo {
  return { id, name, projectIds, folderColor: 'blue' };
}

/** Compact shape: `id`, `id[wt,wt]`, `folder{a,b[wt]}`. */
function shape(items: ProjectListItem[]): string[] {
  const node = (n: { project: ProjectInfo; worktrees: ProjectInfo[] }) =>
    n.worktrees.length > 0
      ? `${n.project.id}[${n.worktrees.map((w) => w.id).join(',')}]`
      : n.project.id;
  return items.map((item) =>
    item.kind === 'folder'
      ? `${item.folder.id}{${item.projects.map(node).join(',')}}`
      : node(item.node),
  );
}

describe('buildProjectList', () => {
  it('follows projectOrder, not the projects array order', () => {
    const projects = [project('a'), project('b'), project('c')];
    expect(shape(buildProjectList(projects, [], ['c', 'a', 'b']))).toEqual(['c', 'a', 'b']);
  });

  it('expands folders in their own member order', () => {
    const projects = [project('a'), project('b'), project('c')];
    const folders = [folder('f', ['c', 'a'])];
    expect(shape(buildProjectList(projects, folders, ['b', 'f']))).toEqual(['b', 'f{c,a}']);
  });

  it('nests worktrees under their parent in worktreeIds order', () => {
    // The daemon pushes worktree ids into project_order too; the desktop skips
    // them at the top level and lists them under the parent.
    const projects = [
      project('p', { worktreeIds: ['w2', 'w1'], folderColor: 'red' }),
      project('w1', { parentProjectId: 'p' }),
      project('w2', { parentProjectId: 'p' }),
      project('q'),
    ];
    const items = buildProjectList(projects, [], ['p', 'q', 'w1', 'w2']);
    expect(shape(items)).toEqual(['p[w2,w1]', 'q']);
    const first = items[0]!;
    if (first.kind !== 'project') throw new Error('expected project');
    // Worktrees inherit the parent's color, like the desktop.
    expect(first.node.worktrees.map((w) => w.folderColor)).toEqual(['red', 'red']);
  });

  it('skips nested worktrees inside folders and nests them under the parent', () => {
    const projects = [
      project('p', { worktreeIds: ['w'] }),
      project('w', { parentProjectId: 'p' }),
    ];
    const folders = [folder('f', ['p', 'w'])];
    expect(shape(buildProjectList(projects, folders, ['f']))).toEqual(['f{p[w]}']);
  });

  it('lists an orphan worktree (parent gone) as a normal project', () => {
    const projects = [project('w', { parentProjectId: 'gone' })];
    expect(shape(buildProjectList(projects, [], ['w']))).toEqual(['w']);
  });

  it('appends projects missing from projectOrder', () => {
    const projects = [project('a'), project('b')];
    expect(shape(buildProjectList(projects, [], ['b']))).toEqual(['b', 'a']);
    expect(shape(buildProjectList(projects, [], []))).toEqual(['a', 'b']);
  });
});

describe('filterProjectList', () => {
  const projects = [
    project('okena', { name: 'Okena', path: '/Users/me/Projects/okena' }),
    project('site', { name: 'Website', path: '/srv/www', worktreeIds: ['wt'] }),
    project('wt', { name: 'site-wt', parentProjectId: 'site', gitBranch: 'feat/Login' }),
    project('api', { name: 'API', path: '/srv/backend' }),
  ];
  const folders = [folder('f', ['api'], 'Work')];
  const items = buildProjectList(projects, folders, ['okena', 'site', 'f']);

  it('returns everything for an empty or blank query', () => {
    expect(filterProjectList(items, '')).toBe(items);
    expect(shape(filterProjectList(items, '   '))).toEqual(shape(items));
  });

  it('matches the name case-insensitively', () => {
    expect(shape(filterProjectList(items, 'OKE'))).toEqual(['okena']);
    expect(shape(filterProjectList(items, 'web'))).toEqual(['site']);
  });

  it('matches the path', () => {
    expect(shape(filterProjectList(items, 'backend'))).toEqual(['f{api}']);
  });

  it('keeps the parent of a matching worktree (matched by displayed branch)', () => {
    expect(shape(filterProjectList(items, 'login'))).toEqual(['site[wt]']);
  });

  it('keeps a whole folder whose name matches', () => {
    expect(shape(filterProjectList(items, 'work'))).toEqual(['f{api}']);
  });

  it('returns nothing when nothing matches', () => {
    expect(filterProjectList(items, 'zzz')).toEqual([]);
  });
});

describe('flattenProjectList / projectDisplayName', () => {
  it('flattens with folder headers and depths', () => {
    const projects = [
      project('p', { worktreeIds: ['w'] }),
      project('w', { parentProjectId: 'p' }),
      project('q'),
    ];
    const rows = flattenProjectList(buildProjectList(projects, [folder('f', ['p'])], ['f', 'q']));
    expect(
      rows.map((r) => (r.kind === 'folder' ? `#${r.folder.id}` : `${r.project.id}:${r.depth}`)),
    ).toEqual(['#f', 'p:1', 'w:2', 'q:0']);
  });

  it('shows the branch for worktrees, the name otherwise', () => {
    expect(projectDisplayName(project('a', { name: 'A', gitBranch: 'main' }))).toBe('A');
    expect(
      projectDisplayName(project('w', { name: 'W', gitBranch: 'feat', parentProjectId: 'a' })),
    ).toBe('feat');
    expect(projectDisplayName(project('w', { name: 'W', parentProjectId: 'a' }))).toBe('W');
  });
});
