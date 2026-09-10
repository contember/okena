/// <reference types="jest" />
// tsconfig pins `types` to react/react-native, so pull jest globals in here.

/**
 * Workspace-store regression: structural layout changes reach the UI even when
 * the terminal id set is untouched (CORR-15).
 */

import type { ProjectInfo } from '../native/okena';
import {
  configureWorkspaceStore,
  useWorkspaceStore,
  WORKSPACE_POLL_MS,
  type WorkspaceNative,
} from './workspaceStore';

const PROJECT: ProjectInfo = {
  id: 'p1',
  name: 'okena',
  path: '/tmp/okena',
  showInOverview: true,
  terminalIds: ['t1', 't2'],
  terminalNames: { t1: 'one', t2: 'two' },
  gitLinesAdded: 0,
  gitLinesRemoved: 0,
  services: [],
  folderColor: 'blue',
};

/** Two tab layouts over the same terminals — only `activeTab` differs. */
const TABS_FIRST_ACTIVE = JSON.stringify({
  type: 'tabs',
  activeTab: 0,
  children: [
    { type: 'terminal', terminalId: 't1' },
    { type: 'terminal', terminalId: 't2' },
  ],
});
const TABS_SECOND_ACTIVE = JSON.stringify({
  type: 'tabs',
  activeTab: 1,
  children: [
    { type: 'terminal', terminalId: 't1' },
    { type: 'terminal', terminalId: 't2' },
  ],
});

function stubNative(layouts: string[]): WorkspaceNative {
  let tick = 0;
  return {
    getProjects: () => [PROJECT],
    getFocusedProjectId: () => PROJECT.id,
    getFolders: () => [],
    getProjectOrder: () => [PROJECT.id],
    getFullscreenTerminal: () => undefined,
    getProjectLayoutJson: () => layouts[Math.min(tick++, layouts.length - 1)]!,
    secondsSinceActivity: () => 0,
  };
}

describe('workspace layout tracking', () => {
  beforeEach(() => {
    jest.useFakeTimers();
  });

  afterEach(() => {
    useWorkspaceStore.getState().stop();
    jest.useRealTimers();
  });

  it('publishes a tab switch that leaves the terminal ids identical', () => {
    configureWorkspaceStore({
      native: stubNative([TABS_FIRST_ACTIVE, TABS_SECOND_ACTIVE]),
    });

    useWorkspaceStore.getState().start('conn-1');
    expect(useWorkspaceStore.getState().projectLayoutJson).toBe(TABS_FIRST_ACTIVE);
    const projectsBefore = useWorkspaceStore.getState().projects;

    jest.advanceTimersByTime(WORKSPACE_POLL_MS);

    expect(useWorkspaceStore.getState().projectLayoutJson).toBe(TABS_SECOND_ACTIVE);
    // The trigger for the original bug: the project list is byte-identical.
    expect(useWorkspaceStore.getState().projects).toEqual(projectsBefore);
  });

  it('clears the layout when polling stops', () => {
    configureWorkspaceStore({ native: stubNative([TABS_FIRST_ACTIVE]) });

    useWorkspaceStore.getState().start('conn-1');
    expect(useWorkspaceStore.getState().projectLayoutJson).not.toBeNull();

    useWorkspaceStore.getState().stop();
    expect(useWorkspaceStore.getState().projectLayoutJson).toBeNull();
  });
});
