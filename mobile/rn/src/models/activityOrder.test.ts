/// <reference types="jest" />
// tsconfig pins `types` to react/react-native, so pull jest globals in here.

import type { ProjectInfo, TerminalActivity } from '../native/okena';
import {
  RUNNING_OUTPUT_WINDOW_MS,
  applySnapshot,
  buildActivityRows,
  computeProjectSignals,
  orderByActivity,
  projectSignals,
  snapshotRows,
  switcherRows,
  type ActivityEntry,
  type OrderedEntry,
} from './activityOrder';

function entry(id: string, manualIndex: number, extra: Partial<ActivityEntry> = {}): ActivityEntry {
  return {
    id,
    pinned: false,
    manualIndex,
    lastActivityAt: undefined,
    hasAttention: false,
    isRunning: false,
    ...extra,
  };
}

const ids = (out: OrderedEntry[]) => out.map((o) => o.entry.id);

// Mirrors the tests in crates/okena-views-sidebar/src/activity_order.rs.
describe('orderByActivity', () => {
  it('keeps pinned first in manual order regardless of activity', () => {
    const a = entry('a', 0, { pinned: true, lastActivityAt: 10 });
    const b = entry('b', 1, { pinned: true, lastActivityAt: 999 });
    const out = orderByActivity([b, a]);
    expect(ids(out)).toEqual(['a', 'b']);
    expect(out.every((o) => o.tier === 'pinned')).toBe(true);
  });

  it('ranks attention over running over recent', () => {
    const out = orderByActivity([
      entry('rest', 0),
      entry('run', 1, { isRunning: true }),
      entry('att', 2, { hasAttention: true }),
    ]);
    expect(ids(out)).toEqual(['att', 'run', 'rest']);
    expect(out.map((o) => o.tier)).toEqual(['attention', 'running', 'recent']);
  });

  it('puts a pinned project above a non-pinned attention project', () => {
    const out = orderByActivity([
      entry('a', 0, { hasAttention: true }),
      entry('p', 5, { pinned: true }),
    ]);
    expect(ids(out)).toEqual(['p', 'a']);
  });

  it('sorts most recent activity first within a tier, never-active last', () => {
    const out = orderByActivity([
      entry('older', 0, { lastActivityAt: 100 }),
      entry('never', 2),
      entry('newer', 1, { lastActivityAt: 200 }),
    ]);
    expect(ids(out)).toEqual(['newer', 'older', 'never']);
  });

  it('breaks ties by manual index', () => {
    const out = orderByActivity([
      entry('x', 3, { lastActivityAt: 50 }),
      entry('y', 1, { lastActivityAt: 50 }),
    ]);
    expect(ids(out)).toEqual(['y', 'x']);
  });

  it('interleaves all four tiers', () => {
    const out = orderByActivity([
      entry('rest', 3, { lastActivityAt: 5 }),
      entry('run', 2, { isRunning: true }),
      entry('att1', 0, { hasAttention: true, lastActivityAt: 10 }),
      entry('att2', 1, { hasAttention: true, lastActivityAt: 20 }),
      entry('pin', 9, { pinned: true }),
    ]);
    expect(ids(out)).toEqual(['pin', 'att2', 'att1', 'run', 'rest']);
  });

  it('yields nothing for no input', () => {
    expect(orderByActivity([])).toEqual([]);
  });
});

function project(id: string, extra: Partial<ProjectInfo> = {}): ProjectInfo {
  return {
    id,
    name: id,
    path: `/home/me/${id}`,
    showInOverview: true,
    terminalIds: [`${id}-t`],
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

function act(terminalId: string, extra: Partial<TerminalActivity> = {}): TerminalActivity {
  return { terminalId, bellUnseen: false, notificationUnseen: false, ...extra };
}

describe('projectSignals', () => {
  const none = new Set<string>();
  const map = (...a: TerminalActivity[]) => new Map(a.map((x) => [x.terminalId, x]));

  it('flags an unseen bell or notification as attention', () => {
    expect(projectSignals(project('p'), map(act('p-t', { bellUnseen: true })), none)).toEqual({
      attention: true,
      running: false,
    });
    expect(
      projectSignals(project('p'), map(act('p-t', { notificationUnseen: true })), none).attention,
    ).toBe(true);
  });

  it('counts recent output as running, older output not', () => {
    const recent = act('p-t', { lastOutputMsAgo: RUNNING_OUTPUT_WINDOW_MS - 1 });
    const stale = act('p-t', { lastOutputMsAgo: RUNNING_OUTPUT_WINDOW_MS });
    expect(projectSignals(project('p'), map(recent), none).running).toBe(true);
    expect(projectSignals(project('p'), map(stale), none).running).toBe(false);
    expect(projectSignals(project('p'), map(act('p-t')), none).running).toBe(false);
  });

  it('maps agent lifecycles: blocked and unseen done need attention, working runs', () => {
    const withAgent = (lifecycle: string) =>
      project('p', { terminalAgentStatus: { 'p-t': { lifecycle } } });
    expect(projectSignals(withAgent('blocked'), map(), none)).toEqual({
      attention: true,
      running: false,
    });
    expect(projectSignals(withAgent('working'), map(), none)).toEqual({
      attention: false,
      running: true,
    });
    expect(projectSignals(withAgent('done'), map(), none).attention).toBe(true);
    expect(projectSignals(withAgent('done'), map(), new Set(['p-t'])).attention).toBe(false);
    expect(projectSignals(withAgent('idle'), map(), none)).toEqual({
      attention: false,
      running: false,
    });
  });

  it('ignores activity of terminals outside the project', () => {
    expect(
      projectSignals(project('p'), map(act('other', { bellUnseen: true })), none).attention,
    ).toBe(false);
  });
});

describe('buildActivityRows', () => {
  it('orders by tier, recency, then manual rank, with worktrees flat', () => {
    const projects = [
      project('a', { lastActivityAt: 100 }),
      project('b', { lastActivityAt: 300 }),
      project('wt', { parentProjectId: 'a', lastActivityAt: 200 }),
      project('pin', { pinned: true }),
      project('bell'),
      project('busy'),
    ];
    const signals = computeProjectSignals(
      projects,
      [act('bell-t', { bellUnseen: true }), act('busy-t', { lastOutputMsAgo: 10 })],
      new Set(),
    );
    const rows = buildActivityRows(projects, ['pin', 'a', 'wt', 'b', 'bell', 'busy'], signals);
    expect(rows.map((r) => [r.tier, r.project.id])).toEqual([
      ['pinned', 'pin'],
      ['attention', 'bell'],
      ['running', 'busy'],
      ['recent', 'b'],
      ['recent', 'wt'],
      ['recent', 'a'],
    ]);
  });

  it('ranks projects missing from the manual order after listed ones', () => {
    const rows = buildActivityRows([project('x'), project('y')], ['y'], {});
    expect(rows.map((r) => r.project.id)).toEqual(['y', 'x']);
  });
});

describe('frozen order', () => {
  it('keeps positions and tiers, refreshes data, drops gone and appends new projects', () => {
    const before = buildActivityRows(
      [project('a', { lastActivityAt: 2 }), project('b', { lastActivityAt: 1 }), project('c')],
      [],
      {},
    );
    const snap = snapshotRows(before);
    // b becomes the most recent and gets a bell; c is removed; d is new.
    const after = buildActivityRows(
      [project('a', { lastActivityAt: 2 }), project('b', { lastActivityAt: 9 }), project('d')],
      [],
      { b: { attention: true, running: false } },
    );
    const frozen = applySnapshot(snap, after);
    expect(frozen.map((r) => [r.tier, r.project.id])).toEqual([
      ['recent', 'a'],
      ['recent', 'b'],
      ['recent', 'd'],
    ]);
    expect(frozen[1]!.project.lastActivityAt).toBe(9);
  });
});

describe('switcherRows', () => {
  const rows = buildActivityRows(
    [
      project('a', { lastActivityAt: 3 }),
      project('b', { lastActivityAt: 2 }),
      project('c', { lastActivityAt: 1 }),
    ],
    [],
    {},
  );

  it('lists current then previous on top, the rest in activity order', () => {
    const { top, rest } = switcherRows(rows, 'c', 'b');
    expect(top.map((r) => r.project.id)).toEqual(['c', 'b']);
    expect(rest.map((r) => r.project.id)).toEqual(['a']);
  });

  it('skips a missing or duplicate previous project', () => {
    expect(switcherRows(rows, 'a', 'gone').top.map((r) => r.project.id)).toEqual(['a']);
    expect(switcherRows(rows, 'a', 'a').top.map((r) => r.project.id)).toEqual(['a']);
    expect(switcherRows(rows, null, null).rest).toHaveLength(3);
  });
});
