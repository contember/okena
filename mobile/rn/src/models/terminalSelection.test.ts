/// <reference types="jest" />
// tsconfig pins `types` to react/react-native, so pull jest globals in here.

import type { ProjectInfo } from '../native/okena';
import { parseLayout } from './layoutNode';
import {
  findTerminalPath,
  pickTerminal,
  resolveRestore,
  terminalLabel,
} from './terminalSelection';

function project(id: string, terminalIds: string[], names: Record<string, string> = {}): ProjectInfo {
  return {
    id,
    name: id,
    path: `/tmp/${id}`,
    showInOverview: true,
    terminalIds,
    terminalNames: names,
    gitLinesAdded: 0,
    gitLinesRemoved: 0,
    services: [],
    folderColor: 'default',
    worktreeIds: [],
    terminalAgentStatus: {},
    pinned: false,
  };
}

describe('pickTerminal', () => {
  it('takes the first candidate that still exists', () => {
    expect(pickTerminal(['a', 'b', 'c'], ['gone', null, 'c', 'b'])).toBe('c');
  });

  it('falls back to the first terminal', () => {
    expect(pickTerminal(['a', 'b'], ['gone', undefined])).toBe('a');
    expect(pickTerminal(['a', 'b'])).toBe('a');
  });

  it('returns null for a project without terminals', () => {
    expect(pickTerminal([], ['a'])).toBeNull();
  });
});

describe('terminalLabel', () => {
  const p = project('p', ['t1', 't2', 't3'], { t2: 'claude' });

  it('uses the custom name when set', () => {
    expect(terminalLabel(p, 't2')).toBe('claude');
  });

  it('numbers unnamed terminals by layout position', () => {
    expect(terminalLabel(p, 't1')).toBe('Terminal 1');
    expect(terminalLabel(p, 't3')).toBe('Terminal 3');
  });
});

describe('findTerminalPath', () => {
  // Wire format (snake_case), as served by getProjectLayoutJson.
  const layout = parseLayout(
    JSON.stringify({
      type: 'split',
      direction: 'horizontal',
      sizes: [50, 50],
      children: [
        { type: 'terminal', terminal_id: 't1' },
        {
          type: 'tabs',
          active_tab: 0,
          children: [
            { type: 'terminal', terminal_id: 't2' },
            { type: 'terminal', terminal_id: 't3' },
          ],
        },
      ],
    }),
  )!;

  it('returns the child-index path to the terminal', () => {
    expect(findTerminalPath(layout, 't1')).toEqual([0]);
    expect(findTerminalPath(layout, 't3')).toEqual([1, 1]);
  });

  it('returns [] for a single-terminal root and null when absent', () => {
    const single = parseLayout(JSON.stringify({ type: 'terminal', terminal_id: 'only' }))!;
    expect(findTerminalPath(single, 'only')).toEqual([]);
    expect(findTerminalPath(layout, 'missing')).toBeNull();
  });
});

describe('resolveRestore', () => {
  const projects = [project('p1', ['a', 'b']), project('p2', ['c'])];

  it('reopens the remembered project and terminal', () => {
    expect(
      resolveRestore(projects, { projectId: 'p1', terminalByProject: { p1: 'b' } }),
    ).toEqual({ kind: 'restored', projectId: 'p1', terminalId: 'b' });
  });

  it("falls back to the project's first terminal when the terminal is gone", () => {
    expect(
      resolveRestore(projects, { projectId: 'p1', terminalByProject: { p1: 'closed' } }),
    ).toEqual({ kind: 'restored', projectId: 'p1', terminalId: 'a' });
  });

  it('reports a project that no longer exists', () => {
    expect(
      resolveRestore(projects, { projectId: 'deleted', terminalByProject: {} }),
    ).toEqual({ kind: 'projectMissing' });
  });

  it('does nothing when no project was remembered', () => {
    expect(resolveRestore(projects, { projectId: null, terminalByProject: { p1: 'b' } })).toBeNull();
  });
});
