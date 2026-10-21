/**
 * activityOrder.ts — the activity-sorted project list (drawer + switcher).
 *
 * Port of the desktop sidebar's `ProjectSortMode::Activity`
 * (`crates/okena-views-sidebar/src/activity_order.rs`, entries built in
 * `sidebar/render.rs`). Every project is a peer: folders are ignored and
 * worktrees are listed flat. Tiers, top to bottom:
 *
 *   1. Pinned — `pinned` projects in their stable manual order, never
 *      reordered by activity (a fixed anchor for muscle memory).
 *   2. Needs attention — something the user should look at (see
 *      {@link projectSignals}).
 *   3. Running — work in progress.
 *   4. Recent — everything else.
 *
 * Tiers 2–4 sort by `lastActivityAt` desc (`undefined` last); ties break by
 * the manual rank, so the order is stable and deterministic.
 *
 * Mobile signals differ from the desktop's because the phone sees other
 * inputs: the desktop reads live terminal objects (bell, OSC notification,
 * "driven and not at a prompt"), mobile reads its terminal mirrors plus the
 * daemon's agent status.
 */

import type {
  ProjectId,
  ProjectInfo,
  TerminalActivity,
  TerminalId,
} from '../native/okena';

export type ActivityTier = 'pinned' | 'attention' | 'running' | 'recent';

/** Section header labels, as on the desktop. */
export const ACTIVITY_TIER_LABELS: Record<ActivityTier, string> = {
  pinned: 'PINNED',
  attention: 'NEEDS ATTENTION',
  running: 'RUNNING',
  recent: 'RECENT',
};

/** One project's inputs to the ordering (desktop `ActivityEntry`). */
export interface ActivityEntry {
  id: ProjectId;
  pinned: boolean;
  /** Stable manual position: pinned-tier order and tiebreaker. Lower first. */
  manualIndex: number;
  /** Unix millis of the last meaningful activity; `undefined` = never. */
  lastActivityAt?: number;
  hasAttention: boolean;
  isRunning: boolean;
}

export interface OrderedEntry {
  tier: ActivityTier;
  entry: ActivityEntry;
}

function tierOf(e: ActivityEntry): ActivityTier {
  if (e.pinned) return 'pinned';
  if (e.hasAttention) return 'attention';
  if (e.isRunning) return 'running';
  return 'recent';
}

/** Order `entries` into the four tiers (desktop `order_by_activity`). */
export function orderByActivity(entries: readonly ActivityEntry[]): OrderedEntry[] {
  // Manual order is the baseline; the tier sorts are stable, so equal keys
  // keep it.
  const byManual = [...entries].sort((a, b) => a.manualIndex - b.manualIndex);
  const groups: Record<ActivityTier, ActivityEntry[]> = {
    pinned: [],
    attention: [],
    running: [],
    recent: [],
  };
  for (const e of byManual) groups[tierOf(e)].push(e);

  const recency = (a: ActivityEntry, b: ActivityEntry) =>
    (b.lastActivityAt ?? -1) - (a.lastActivityAt ?? -1);
  groups.attention.sort(recency);
  groups.running.sort(recency);
  groups.recent.sort(recency);

  const out: OrderedEntry[] = [];
  for (const tier of ['pinned', 'attention', 'running', 'recent'] as const) {
    for (const entry of groups[tier]) out.push({ tier, entry });
  }
  return out;
}

// ── mobile signals ───────────────────────────────────────────────────────────

/**
 * A terminal that printed within this window counts as running. The desktop
 * asks the process table ("driven and not at a prompt"), which the phone
 * cannot; recent output is the cheap stand-in. Five seconds keeps a build
 * that prints every few seconds in the tier without a finished command
 * lingering there, and is well above the 1 s state poll.
 */
export const RUNNING_OUTPUT_WINDOW_MS = 5000;

/** Per-project live state behind the tier and the row's status dot. */
export interface ProjectSignals {
  attention: boolean;
  running: boolean;
}

/** Why a terminal needs the user, most urgent first. */
export type AttentionKind = 'dialog' | 'blocked' | 'notification' | 'bell' | 'done';

const ATTENTION_PRIORITY: Record<AttentionKind, number> = {
  dialog: 0,
  blocked: 1,
  notification: 2,
  bell: 3,
  done: 4,
};

/** A terminal that needs the user, as listed on the dashboard. */
export interface AttentionItem {
  projectId: ProjectId;
  terminalId: TerminalId;
  kind: AttentionKind;
  /** Open Claude dialogs in the terminal. */
  dialogs: number;
}

/**
 * The most urgent reason `terminalId` needs the user, or `null`: an open
 * Claude dialog, an agent that is `blocked`, an unseen notification or bell,
 * or an agent that is `done` and not yet seen on the phone (`doneSeen`).
 */
export function terminalAttention(
  project: ProjectInfo,
  terminalId: TerminalId,
  activity: TerminalActivity | undefined,
  doneSeen: ReadonlySet<TerminalId>,
  claudeDialogs: Readonly<Record<TerminalId, number>>,
): AttentionKind | null {
  const lifecycle = project.terminalAgentStatus[terminalId]?.lifecycle;
  if ((claudeDialogs[terminalId] ?? 0) > 0) return 'dialog';
  if (lifecycle === 'blocked') return 'blocked';
  if (activity?.notificationUnseen) return 'notification';
  if (activity?.bellUnseen) return 'bell';
  if (lifecycle === 'done' && !doneSeen.has(terminalId)) return 'done';
  return null;
}

/**
 * Attention: any of the project's terminals needs the user (see
 * {@link terminalAttention}). Running: an agent that is `working`, or any
 * terminal with output in the last {@link RUNNING_OUTPUT_WINDOW_MS}.
 */
export function projectSignals(
  project: ProjectInfo,
  activity: ReadonlyMap<TerminalId, TerminalActivity>,
  doneSeen: ReadonlySet<TerminalId>,
  claudeDialogs: Readonly<Record<TerminalId, number>>,
): ProjectSignals {
  let attention = false;
  let running = false;
  for (const tid of project.terminalIds) {
    const a = activity.get(tid);
    if (terminalAttention(project, tid, a, doneSeen, claudeDialogs) !== null) attention = true;
    if (a?.lastOutputMsAgo !== undefined && a.lastOutputMsAgo < RUNNING_OUTPUT_WINDOW_MS) {
      running = true;
    }
    if (project.terminalAgentStatus[tid]?.lifecycle === 'working') running = true;
  }
  return { attention, running };
}

/** Signals for every project, keyed by id. */
export function computeProjectSignals(
  projects: readonly ProjectInfo[],
  activity: readonly TerminalActivity[],
  doneSeen: ReadonlySet<TerminalId>,
  claudeDialogs: Readonly<Record<TerminalId, number>>,
): Record<ProjectId, ProjectSignals> {
  const byTerminal = new Map(activity.map((a) => [a.terminalId, a]));
  const out: Record<ProjectId, ProjectSignals> = {};
  for (const p of projects) out[p.id] = projectSignals(p, byTerminal, doneSeen, claudeDialogs);
  return out;
}

/**
 * Every terminal that needs the user, most urgent kind first; within a kind
 * in project and terminal order.
 */
export function computeAttentionItems(
  projects: readonly ProjectInfo[],
  activity: readonly TerminalActivity[],
  doneSeen: ReadonlySet<TerminalId>,
  claudeDialogs: Readonly<Record<TerminalId, number>>,
): AttentionItem[] {
  const byTerminal = new Map(activity.map((a) => [a.terminalId, a]));
  const items: AttentionItem[] = [];
  for (const p of projects) {
    for (const tid of p.terminalIds) {
      const kind = terminalAttention(p, tid, byTerminal.get(tid), doneSeen, claudeDialogs);
      if (kind === null) continue;
      items.push({ projectId: p.id, terminalId: tid, kind, dialogs: claudeDialogs[tid] ?? 0 });
    }
  }
  // Array.prototype.sort is stable, so project and terminal order survive.
  return items.sort((a, b) => ATTENTION_PRIORITY[a.kind] - ATTENTION_PRIORITY[b.kind]);
}

export function attentionItemsEqual(
  a: readonly AttentionItem[],
  b: readonly AttentionItem[],
): boolean {
  if (a.length !== b.length) return false;
  return a.every((item, i) => {
    const other = b[i]!;
    return (
      item.terminalId === other.terminalId &&
      item.projectId === other.projectId &&
      item.kind === other.kind &&
      item.dialogs === other.dialogs
    );
  });
}

/** Shallow equality for {@link computeProjectSignals} results. */
export function signalsEqual(
  a: Record<ProjectId, ProjectSignals>,
  b: Record<ProjectId, ProjectSignals>,
): boolean {
  const ka = Object.keys(a);
  if (ka.length !== Object.keys(b).length) return false;
  for (const k of ka) {
    const sa = a[k]!;
    const sb = b[k];
    if (!sb || sa.attention !== sb.attention || sa.running !== sb.running) return false;
  }
  return true;
}

// ── list assembly ────────────────────────────────────────────────────────────

/** One row of the activity list. */
export interface ActivityRow {
  tier: ActivityTier;
  project: ProjectInfo;
}

/**
 * Order the projects by activity. `manualOrder` is the project ids in the
 * manual list's display order (folders expanded, worktrees after their
 * parent); ids missing from it rank after all listed ones. Unlike the desktop,
 * whose manual rank skips worktrees (they all tie last), a worktree here
 * ranks right after its parent.
 */
export function buildActivityRows(
  projects: readonly ProjectInfo[],
  manualOrder: readonly ProjectId[],
  signals: Readonly<Record<ProjectId, ProjectSignals>>,
): ActivityRow[] {
  const rank = new Map<ProjectId, number>();
  manualOrder.forEach((id, i) => {
    if (!rank.has(id)) rank.set(id, i);
  });
  const byId = new Map(projects.map((p) => [p.id, p]));
  const entries: ActivityEntry[] = projects.map((p) => ({
    id: p.id,
    pinned: p.pinned,
    manualIndex: rank.get(p.id) ?? Number.MAX_SAFE_INTEGER,
    lastActivityAt: p.lastActivityAt,
    hasAttention: signals[p.id]?.attention ?? false,
    isRunning: signals[p.id]?.running ?? false,
  }));
  return orderByActivity(entries).map(({ tier, entry }) => ({
    tier,
    project: byId.get(entry.id)!,
  }));
}

/** Frozen order: tier + id per row, captured when a list opens. */
export type ActivitySnapshot = ReadonlyArray<{ tier: ActivityTier; id: ProjectId }>;

export function snapshotRows(rows: readonly ActivityRow[]): ActivitySnapshot {
  return rows.map((r) => ({ tier: r.tier, id: r.project.id }));
}

/**
 * Re-apply a frozen order to fresh rows: positions and tiers come from the
 * snapshot, project data from `rows`. Projects that disappeared are dropped;
 * new ones are appended at the end of the list so they stay reachable.
 */
export function applySnapshot(
  snapshot: ActivitySnapshot,
  rows: readonly ActivityRow[],
): ActivityRow[] {
  const byId = new Map(rows.map((r) => [r.project.id, r]));
  const out: ActivityRow[] = [];
  const used = new Set<ProjectId>();
  for (const { tier, id } of snapshot) {
    const row = byId.get(id);
    if (row && !used.has(id)) {
      out.push({ tier, project: row.project });
      used.add(id);
    }
  }
  const lastTier = out.length > 0 ? out[out.length - 1]!.tier : 'recent';
  for (const r of rows) {
    if (!used.has(r.project.id)) out.push({ tier: lastTier, project: r.project });
  }
  return out;
}

/**
 * Switcher order: the current project first, the previously viewed one right
 * under it (one tap to toggle between two projects), then the rest in
 * activity order. Missing ids are skipped.
 */
export function switcherRows(
  rows: readonly ActivityRow[],
  currentId: ProjectId | null,
  previousId: ProjectId | null,
): { top: ActivityRow[]; rest: ActivityRow[] } {
  const top: ActivityRow[] = [];
  for (const id of [currentId, previousId]) {
    if (id === null || top.some((r) => r.project.id === id)) continue;
    const row = rows.find((r) => r.project.id === id);
    if (row) top.push(row);
  }
  const topIds = new Set(top.map((r) => r.project.id));
  return { top, rest: rows.filter((r) => !topIds.has(r.project.id)) };
}
