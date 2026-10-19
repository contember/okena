/// <reference types="jest" />
// tsconfig pins `types` to react/react-native, so pull jest globals in here.

/**
 * Workspace-store regression: structural layout changes reach the UI even when
 * the terminal id set is untouched (CORR-15).
 */

import type { AgentLifecycle } from '../native/claude';
import type { ProjectInfo, TerminalActivity } from '../native/okena';
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
  worktreeIds: [],
  terminalAgentStatus: {},
  pinned: false,
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
    getClaudeDialogCounts: () => ({}),
    getClaudeSessions: () => [],
    secondsSinceActivity: () => 0,
    getTerminalActivity: () => [],
    markTerminalSeen: () => {},
    focusTerminal: () => Promise.resolve(),
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

describe('workspace folder tracking', () => {
  beforeEach(() => {
    jest.useFakeTimers();
  });

  afterEach(() => {
    useWorkspaceStore.getState().stop();
    jest.useRealTimers();
  });

  it('publishes a reorder inside a folder (same folder ids)', () => {
    const before = [{ id: 'f', name: 'Work', projectIds: ['a', 'b'], folderColor: 'blue' }];
    const after = [{ id: 'f', name: 'Work', projectIds: ['b', 'a'], folderColor: 'blue' }];
    let tick = 0;
    configureWorkspaceStore({
      native: {
        ...stubNative([TABS_FIRST_ACTIVE]),
        getFolders: () => (tick++ === 0 ? before : after),
      },
    });

    useWorkspaceStore.getState().start('conn-1');
    expect(useWorkspaceStore.getState().folders[0]!.projectIds).toEqual(['a', 'b']);

    jest.advanceTimersByTime(WORKSPACE_POLL_MS);

    expect(useWorkspaceStore.getState().folders[0]!.projectIds).toEqual(['b', 'a']);
  });
});

describe('workspace terminal selection memory', () => {
  const P1: ProjectInfo = { ...PROJECT, id: 'p1', terminalIds: ['a', 'b', 'c'] };
  const P2: ProjectInfo = { ...PROJECT, id: 'p2', terminalIds: ['x', 'y'] };

  function native(projects: () => ProjectInfo[], focused = 'p2'): WorkspaceNative {
    return {
      ...stubNative([TABS_FIRST_ACTIVE]),
      getProjects: projects,
      getFocusedProjectId: () => focused,
    };
  }

  beforeEach(() => {
    jest.useFakeTimers();
  });

  afterEach(() => {
    useWorkspaceStore.getState().stop();
    jest.useRealTimers();
  });

  it('restores the remembered project and terminal over the focused project', () => {
    configureWorkspaceStore({ native: native(() => [P1, P2]) });
    useWorkspaceStore
      .getState()
      .start('conn-1', { projectId: 'p1', terminalByProject: { p1: 'c', p2: 'y' } });

    const s = useWorkspaceStore.getState();
    expect(s.selectedProjectId).toBe('p1');
    expect(s.selectedTerminalId).toBe('c');
    expect(s.restoreProjectMissing).toBe(false);
  });

  it('falls back to the first terminal when the remembered one is gone', () => {
    configureWorkspaceStore({ native: native(() => [P1, P2]) });
    useWorkspaceStore.getState().start('conn-1', { projectId: 'p1', terminalByProject: { p1: 'gone' } });

    expect(useWorkspaceStore.getState().selectedTerminalId).toBe('a');
  });

  it('flags a missing project and falls back to the focused one', () => {
    configureWorkspaceStore({ native: native(() => [P1, P2]) });
    useWorkspaceStore.getState().start('conn-1', { projectId: 'deleted', terminalByProject: {} });

    const s = useWorkspaceStore.getState();
    expect(s.restoreProjectMissing).toBe(true);
    expect(s.selectedProjectId).toBe('p2');
    s.clearRestoreProjectMissing();
    expect(useWorkspaceStore.getState().restoreProjectMissing).toBe(false);
  });

  it('waits for the project list before restoring', () => {
    let projects: ProjectInfo[] = [];
    configureWorkspaceStore({ native: native(() => projects) });
    useWorkspaceStore.getState().start('conn-1', { projectId: 'p1', terminalByProject: { p1: 'b' } });
    expect(useWorkspaceStore.getState().selectedProjectId).toBeNull();

    projects = [P1, P2];
    jest.advanceTimersByTime(WORKSPACE_POLL_MS);

    expect(useWorkspaceStore.getState().selectedProjectId).toBe('p1');
    expect(useWorkspaceStore.getState().selectedTerminalId).toBe('b');
  });

  it('reopens the last terminal of a project when switching back to it', () => {
    configureWorkspaceStore({ native: native(() => [P1, P2], 'p1') });
    const store = useWorkspaceStore.getState();
    store.start('conn-1');
    expect(useWorkspaceStore.getState().selectedTerminalId).toBe('a');

    useWorkspaceStore.getState().selectTerminal('c');
    useWorkspaceStore.getState().selectProject('p2');
    expect(useWorkspaceStore.getState().selectedTerminalId).toBe('x');
    useWorkspaceStore.getState().selectTerminal('y');

    useWorkspaceStore.getState().selectProject('p1');
    expect(useWorkspaceStore.getState().selectedTerminalId).toBe('c');
    jest.advanceTimersByTime(WORKSPACE_POLL_MS);
    expect(useWorkspaceStore.getState().selectedTerminalId).toBe('c');
    expect(useWorkspaceStore.getState().terminalByProject).toEqual({ p1: 'c', p2: 'y' });
  });

  it('selects the remembered terminal when the selection vanishes', () => {
    let p1 = P1;
    configureWorkspaceStore({ native: native(() => [p1, P2], 'p1') });
    useWorkspaceStore.getState().start('conn-1', { projectId: null, terminalByProject: { p1: 'b' } });
    expect(useWorkspaceStore.getState().selectedTerminalId).toBe('b');

    useWorkspaceStore.getState().selectTerminal('c');
    // 'c' closed on the desktop: fall back to the first terminal.
    p1 = { ...P1, terminalIds: ['a', 'b'] };
    jest.advanceTimersByTime(WORKSPACE_POLL_MS);
    expect(useWorkspaceStore.getState().selectedTerminalId).toBe('a');
  });
});

describe('workspace activity signals', () => {
  const A: ProjectInfo = { ...PROJECT, id: 'pa', terminalIds: ['a1'] };
  const B: ProjectInfo = { ...PROJECT, id: 'pb', terminalIds: ['b1'] };

  function activityNative(opts: {
    projects?: () => ProjectInfo[];
    activity: () => TerminalActivity[];
    seen?: string[];
    focused?: { project: string; terminal: string }[];
  }): WorkspaceNative {
    return {
      ...stubNative([TABS_FIRST_ACTIVE]),
      getProjects: opts.projects ?? (() => [A, B]),
      getFocusedProjectId: () => 'pa',
      getTerminalActivity: opts.activity,
      markTerminalSeen: (_c, tid) => {
        opts.seen?.push(tid);
      },
      focusTerminal: (_c, project, terminal) => {
        opts.focused?.push({ project, terminal });
        return Promise.resolve();
      },
    };
  }

  const bell = (terminalId: string): TerminalActivity => ({
    terminalId,
    bellUnseen: true,
    notificationUnseen: false,
  });

  beforeEach(() => {
    jest.useFakeTimers();
  });

  afterEach(() => {
    useWorkspaceStore.getState().stop();
    configureWorkspaceStore({ isForeground: () => true });
    jest.useRealTimers();
  });

  it('flags a bell in a background project and marks the shown terminal seen', () => {
    const seen: string[] = [];
    configureWorkspaceStore({
      native: activityNative({ activity: () => [bell('a1'), bell('b1')], seen }),
      isForeground: () => true,
    });
    useWorkspaceStore.getState().start('conn-1');

    const s = useWorkspaceStore.getState();
    expect(s.selectedTerminalId).toBe('a1');
    expect(seen).toEqual(['a1']);
    expect(s.projectSignals.pa).toEqual({ attention: false, running: false });
    expect(s.projectSignals.pb).toEqual({ attention: true, running: false });
  });

  it('ignores output of the terminal on screen for running', () => {
    configureWorkspaceStore({
      native: activityNative({
        activity: () => [
          { terminalId: 'a1', bellUnseen: false, notificationUnseen: false, lastOutputMsAgo: 10 },
          { terminalId: 'b1', bellUnseen: false, notificationUnseen: false, lastOutputMsAgo: 10 },
        ],
      }),
      isForeground: () => true,
    });
    useWorkspaceStore.getState().start('conn-1');
    const s = useWorkspaceStore.getState();
    expect(s.projectSignals.pa!.running).toBe(false);
    expect(s.projectSignals.pb!.running).toBe(true);
  });

  it('does not mark anything seen while the app is in the background', () => {
    const seen: string[] = [];
    configureWorkspaceStore({
      native: activityNative({ activity: () => [bell('a1')], seen }),
      isForeground: () => false,
    });
    useWorkspaceStore.getState().start('conn-1');
    expect(seen).toEqual([]);
    expect(useWorkspaceStore.getState().projectSignals.pa!.attention).toBe(true);
  });

  it('clears attention as soon as the project is opened', () => {
    const seen: string[] = [];
    configureWorkspaceStore({
      native: activityNative({ activity: () => [bell('b1')], seen }),
      isForeground: () => true,
    });
    useWorkspaceStore.getState().start('conn-1');
    expect(useWorkspaceStore.getState().projectSignals.pb!.attention).toBe(true);

    useWorkspaceStore.getState().selectProject('pb');
    expect(seen).toEqual(['b1']);
    expect(useWorkspaceStore.getState().projectSignals.pb!.attention).toBe(false);
  });

  it('acknowledges a done agent once seen, until the next transition to done', () => {
    let lifecycle: AgentLifecycle = 'done';
    const withAgent = (): ProjectInfo[] => [
      A,
      { ...B, terminalAgentStatus: { b1: { lifecycle } } },
    ];
    configureWorkspaceStore({
      native: activityNative({ projects: withAgent, activity: () => [] }),
      isForeground: () => true,
    });
    const store = useWorkspaceStore.getState();
    store.start('conn-1');
    expect(useWorkspaceStore.getState().projectSignals.pb!.attention).toBe(true);

    store.selectProject('pb');
    store.selectProject('pa');
    jest.advanceTimersByTime(WORKSPACE_POLL_MS);
    expect(useWorkspaceStore.getState().projectSignals.pb!.attention).toBe(false);

    lifecycle = 'working';
    jest.advanceTimersByTime(WORKSPACE_POLL_MS);
    expect(useWorkspaceStore.getState().projectSignals.pb).toEqual({
      attention: false,
      running: true,
    });

    lifecycle = 'done';
    jest.advanceTimersByTime(WORKSPACE_POLL_MS);
    expect(useWorkspaceStore.getState().projectSignals.pb!.attention).toBe(true);
  });

  it('remembers the previous project and reports the switch to the daemon', () => {
    const focused: { project: string; terminal: string }[] = [];
    configureWorkspaceStore({
      native: activityNative({ activity: () => [], focused }),
      isForeground: () => true,
    });
    const store = useWorkspaceStore.getState();
    store.start('conn-1');
    store.selectProject('pb');
    expect(useWorkspaceStore.getState().previousProjectId).toBe('pa');
    store.selectProject('pb'); // re-selecting the same project changes nothing
    expect(useWorkspaceStore.getState().previousProjectId).toBe('pa');
    store.selectProject('pa');
    expect(useWorkspaceStore.getState().previousProjectId).toBe('pb');
    expect(focused).toEqual([
      { project: 'pb', terminal: 'b1' },
      { project: 'pa', terminal: 'a1' },
    ]);
  });

  it('publishes a pin or new activity stamp even when nothing else changed', () => {
    let pinned = false;
    configureWorkspaceStore({
      native: activityNative({ projects: () => [{ ...A, pinned }, B], activity: () => [] }),
      isForeground: () => true,
    });
    useWorkspaceStore.getState().start('conn-1');
    pinned = true;
    jest.advanceTimersByTime(WORKSPACE_POLL_MS);
    expect(useWorkspaceStore.getState().projects[0]!.pinned).toBe(true);
  });
});

describe('workspace Claude dialog tracking', () => {
  beforeEach(() => {
    jest.useFakeTimers();
  });

  afterEach(() => {
    useWorkspaceStore.getState().stop();
    jest.useRealTimers();
  });

  it('publishes dialog-count changes', () => {
    let counts: Record<string, number> = {};
    configureWorkspaceStore({
      native: { ...stubNative([TABS_FIRST_ACTIVE]), getClaudeDialogCounts: () => counts },
    });

    useWorkspaceStore.getState().start('conn-1');
    expect(useWorkspaceStore.getState().claudeDialogs).toEqual({});

    counts = { t1: 1 };
    jest.advanceTimersByTime(WORKSPACE_POLL_MS);
    expect(useWorkspaceStore.getState().claudeDialogs).toEqual({ t1: 1 });
  });
});

describe('workspace Claude sessions', () => {
  beforeEach(() => {
    jest.useFakeTimers();
  });

  afterEach(() => {
    useWorkspaceStore.getState().stop();
    jest.useRealTimers();
  });

  it('publishes the terminals with a connected claude', () => {
    let sessions: string[] = [];
    configureWorkspaceStore({
      native: { ...stubNative([TABS_FIRST_ACTIVE]), getClaudeSessions: () => sessions },
    });

    useWorkspaceStore.getState().start('conn-1');
    expect(useWorkspaceStore.getState().claudeSessions).toEqual([]);

    sessions = ['t1'];
    jest.advanceTimersByTime(WORKSPACE_POLL_MS);
    expect(useWorkspaceStore.getState().claudeSessions).toEqual(['t1']);
  });
});

describe('workspace terminal views', () => {
  afterEach(() => {
    useWorkspaceStore.getState().stop();
  });

  it('remembers the chat view per terminal until switched back or stopped', () => {
    configureWorkspaceStore({ native: stubNative([TABS_FIRST_ACTIVE]) });
    const store = useWorkspaceStore.getState();
    store.start('conn-1');

    store.setTerminalView('t1', 'chat');
    store.setTerminalView('t2', 'chat');
    store.setTerminalView('t2', 'terminal');
    expect(useWorkspaceStore.getState().terminalViews).toEqual({ t1: 'chat' });

    store.stop();
    expect(useWorkspaceStore.getState().terminalViews).toEqual({});
  });
});
