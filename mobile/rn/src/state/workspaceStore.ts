/**
 * workspaceStore.ts — project / folder / layout state (zustand).
 *
 * Ports `mobile/lib/src/providers/workspace_provider.dart`. While connected it
 * polls the cached remote state (~1s) for:
 *   - the project list (`getProjects`),
 *   - folders (`getFolders`),
 *   - project order (`getProjectOrder`),
 *   - the server's focused project (`getFocusedProjectId`),
 *   - the fullscreen terminal (`getFullscreenTerminal`),
 *   - the selected project's layout JSON (`getProjectLayoutJson`),
 *   - per-terminal activity (`getTerminalActivity`), folded into per-project
 *     {@link ProjectSignals} for the activity-ordered project list.
 *
 * The terminal on screen counts as seen: each poll (and each selection)
 * clears its unseen bell / notification and acknowledges a `done` agent. Its
 * output does not make its project "running".
 *
 * It auto-selects the focused project when nothing is selected, and auto-selects
 * a terminal within the selected project (newly-added one, or the remembered /
 * first one if the current selection vanished) — mirroring the Dart
 * `_pollState` logic.
 *
 * The phone shows one terminal at a time, so the store also remembers the
 * selected terminal per project (`terminalByProject`) and can reopen a
 * remembered selection when polling starts (see `start`'s `restore`).
 *
 * Lifecycle: the Dart provider listened to the connection provider and
 * started/stopped polling on connect/disconnect. Here that wiring is explicit —
 * call {@link WorkspaceState.start} when the connection becomes connected and
 * {@link WorkspaceState.stop} when it isn't (App.tsx wires this; see the nav
 * layer). `start` needs the live `connId`.
 *
 * Dependencies (the native module) are injected via
 * {@link configureWorkspaceStore}, same philosophy as the connection store.
 */

import { AppState } from 'react-native';
import { create, type StoreApi, type UseBoundStore } from 'zustand';

import type {
  ConnId,
  FolderInfo,
  FullscreenInfo,
  OkenaNative,
  ProjectId,
  ProjectInfo,
  TerminalId,
} from '../native/okena';
import { getOkenaNative } from '../native/okena';
import {
  pickTerminal,
  resolveRestore,
  type SelectionMemory,
} from '../models/terminalSelection';
import {
  computeProjectSignals,
  signalsEqual,
  type ProjectSignals,
} from '../models/activityOrder';
import type { TerminalActivity } from '../native/okena';

/** Poll interval (ms) for the workspace state — Dart used 1000ms. */
export const WORKSPACE_POLL_MS = 1000;

/**
 * The slice of the native surface this store calls. Narrowed from
 * {@link OkenaNative} so a test stub only has to provide these.
 */
export type WorkspaceNative = Pick<
  OkenaNative,
  | 'getProjects'
  | 'getFocusedProjectId'
  | 'getFolders'
  | 'getProjectOrder'
  | 'getFullscreenTerminal'
  | 'getProjectLayoutJson'
  | 'secondsSinceActivity'
  | 'getTerminalActivity'
  | 'markTerminalSeen'
  | 'focusTerminal'
>;

/** Dependencies the store calls out to. Overridable for tests. */
export interface WorkspaceDeps {
  native: WorkspaceNative;
  /**
   * Whether the app is in the foreground. The terminal on screen only counts
   * as seen while it is.
   */
  isForeground: () => boolean;
}

/**
 * The workspace store's state + actions. Screen agents read fields with the
 * `useWorkspaceStore(selector)` hook and call the action methods.
 */
export interface WorkspaceState {
  // ── state ────────────────────────────────────────────────────────────────
  /** Projects from the cached remote state. */
  projects: ProjectInfo[];
  /** Folders from the cached remote state. */
  folders: FolderInfo[];
  /** Server-defined project ordering (list of project ids). */
  projectOrder: ProjectId[];
  /** The active fullscreen terminal, if any. */
  fullscreenTerminal: FullscreenInfo | null;
  /**
   * Authoritative layout JSON for the selected project, refreshed every poll.
   * The terminal id set does not change on a tab switch, a minimize or a split
   * resize, so this string is what consumers must key a re-render on.
   */
  projectLayoutJson: string | null;
  /** Currently-selected project id (auto-selected from focused, or by the user). */
  selectedProjectId: ProjectId | null;
  /** Currently-selected terminal id within the selected project. */
  selectedTerminalId: TerminalId | null;
  /** Seconds since last WS activity (also tracked here, per the Dart provider). */
  secondsSinceActivity: number;
  /** The terminal last selected in each project, for switching back to a project. */
  terminalByProject: Record<ProjectId, TerminalId>;
  /** Live activity signals per project (drawer / switcher order and dots). */
  projectSignals: Record<ProjectId, ProjectSignals>;
  /** The project shown before the current one (switcher: one tap back). */
  previousProjectId: ProjectId | null;
  /**
   * Set when a restore named a project the server no longer has. The screen
   * shows the project list and clears it via {@link clearRestoreProjectMissing}.
   */
  restoreProjectMissing: boolean;

  // ── actions ────────────────────────────────────────────────────────────
  /**
   * Start polling for the given connection. Idempotent: re-calling with a new
   * `connId` restarts against it. Does an immediate first poll (as Dart did).
   * Call when the connection becomes `connected`.
   *
   * `restore` reopens a remembered selection: its project + terminal are
   * selected once the project list arrives, instead of the server's focused
   * project. Ignored when already polling `connId`.
   */
  start(connId: ConnId, restore?: SelectionMemory): void;
  /**
   * Stop polling and clear all workspace state. Call when the connection is no
   * longer connected (mirrors the Dart `_onConnectionChanged` else-branch).
   */
  stop(): void;
  /** Select a project and its remembered (or first) terminal. Mirrors `selectProject`. */
  selectProject(projectId: ProjectId): void;
  /** Select a terminal within the current project. Mirrors `selectTerminal`. */
  selectTerminal(terminalId: TerminalId): void;
  /** Acknowledge {@link restoreProjectMissing}. */
  clearRestoreProjectMissing(): void;
  /**
   * The currently-selected project object, or the first project as a fallback,
   * or `null`. Mirrors the Dart `selectedProject` getter. (Provided as a
   * selector helper since zustand state holds only the id.)
   */
  getSelectedProject(): ProjectInfo | null;
}

let injectedDeps: Partial<WorkspaceDeps> = {};
let resolvedDeps: WorkspaceDeps | null = null;

function deps(): WorkspaceDeps {
  if (!resolvedDeps) {
    resolvedDeps = {
      native: injectedDeps.native ?? getOkenaNative(),
      isForeground: injectedDeps.isForeground ?? (() => AppState.currentState === 'active'),
    };
  }
  return resolvedDeps;
}

/**
 * Override the store's native module. Call before the first action runs (e.g.
 * at the top of a test, or once at app start).
 */
export function configureWorkspaceStore(overrides: Partial<WorkspaceDeps>): void {
  injectedDeps = { ...injectedDeps, ...overrides };
  resolvedDeps = null;
}

// ── polling: the store owns a single timer + the live connId ────────────────

let pollTimer: ReturnType<typeof setInterval> | null = null;
let activeConnId: ConnId | null = null;
/** Previous terminal-id set for the selected project (newly-added detection). */
let previousTerminalIds: Set<TerminalId> | null = null;
/** Selection to reopen once the first non-empty project list arrives. */
let pendingRestore: SelectionMemory | null = null;
/** Latest per-terminal activity, reused when a selection re-derives signals. */
let lastActivity: TerminalActivity[] = [];
/**
 * Terminals whose current `done` agent status was seen on the phone. An entry
 * is dropped as soon as the terminal leaves `done`, so the next transition to
 * `done` is unseen again: the key is effectively terminal + transition.
 */
const doneSeen = new Set<TerminalId>();

function stopPolling(): void {
  if (pollTimer !== null) {
    clearInterval(pollTimer);
    pollTimer = null;
  }
  activeConnId = null;
  previousTerminalIds = null;
  pendingRestore = null;
  lastActivity = [];
  doneSeen.clear();
}

/** `map` with `projectId → terminalId` set; the same object when unchanged. */
function rememberTerminal(
  map: Record<ProjectId, TerminalId>,
  projectId: ProjectId,
  terminalId: TerminalId | null,
): Record<ProjectId, TerminalId> {
  if (terminalId === null || map[projectId] === terminalId) return map;
  return { ...map, [projectId]: terminalId };
}

/** Shallow id+name+git+services equality for the project list (mirrors `_projectListEquals`). */
function projectListEquals(a: ProjectInfo[], b: ProjectInfo[]): boolean {
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) {
    const pa = a[i]!;
    const pb = b[i]!;
    if (pa.id !== pb.id || pa.name !== pb.name || pa.path !== pb.path) return false;
    if (pa.showInOverview !== pb.showInOverview) return false;
    if (pa.parentProjectId !== pb.parentProjectId) return false;
    if (!arrayEquals(pa.worktreeIds, pb.worktreeIds)) return false;
    if (!arrayEquals(pa.terminalIds, pb.terminalIds)) return false;
    if (pa.gitBranch !== pb.gitBranch) return false;
    if (pa.gitLinesAdded !== pb.gitLinesAdded) return false;
    if (pa.gitLinesRemoved !== pb.gitLinesRemoved) return false;
    if (pa.services.length !== pb.services.length) return false;
    for (let j = 0; j < pa.services.length; j++) {
      if (
        pa.services[j]!.name !== pb.services[j]!.name ||
        pa.services[j]!.status !== pb.services[j]!.status
      ) {
        return false;
      }
    }
    if (pa.folderColor !== pb.folderColor) return false;
    if (pa.pinned !== pb.pinned || pa.lastActivityAt !== pb.lastActivityAt) return false;
    if (!agentStatusEquals(pa.terminalAgentStatus, pb.terminalAgentStatus)) return false;
  }
  return true;
}

function agentStatusEquals(
  a: ProjectInfo['terminalAgentStatus'],
  b: ProjectInfo['terminalAgentStatus'],
): boolean {
  const ka = Object.keys(a);
  if (ka.length !== Object.keys(b).length) return false;
  for (const k of ka) {
    const sa = a[k]!;
    const sb = b[k];
    if (!sb || sa.lifecycle !== sb.lifecycle || sa.custom !== sb.custom) return false;
  }
  return true;
}

/**
 * Mark the terminal on screen seen (bell / notification cleared in the
 * mirror, a `done` agent acknowledged), forget acknowledgements of terminals
 * that left `done`, and derive per-project signals from `lastActivity`.
 */
function deriveSignals(
  projects: ProjectInfo[],
  shownTerminalId: TerminalId | null,
  connId: ConnId,
): Record<ProjectId, ProjectSignals> {
  const native = deps().native;
  const lifecycle = new Map<TerminalId, string>();
  for (const p of projects) {
    for (const [tid, status] of Object.entries(p.terminalAgentStatus)) {
      lifecycle.set(tid, status.lifecycle);
    }
  }
  for (const tid of [...doneSeen]) if (lifecycle.get(tid) !== 'done') doneSeen.delete(tid);

  if (shownTerminalId !== null && deps().isForeground()) {
    if (lifecycle.get(shownTerminalId) === 'done') doneSeen.add(shownTerminalId);
    const i = lastActivity.findIndex((a) => a.terminalId === shownTerminalId);
    const shown = i >= 0 ? lastActivity[i]! : undefined;
    if (shown && (shown.bellUnseen || shown.notificationUnseen)) {
      native.markTerminalSeen(connId, shownTerminalId);
      lastActivity = [...lastActivity];
      lastActivity[i] = { ...shown, bellUnseen: false, notificationUnseen: false };
    }
  }
  // Output of the terminal on screen is not "running": the user watches it,
  // and most of it is their own typing echo or the redraw after the phone
  // resized the PTY on open.
  const activity =
    shownTerminalId === null
      ? lastActivity
      : lastActivity.map((a) =>
          a.terminalId === shownTerminalId ? { ...a, lastOutputMsAgo: undefined } : a,
        );
  return computeProjectSignals(projects, activity, doneSeen);
}

/** Folder lists equal by id, name, color and member order (drives drawer ordering). */
function folderListEquals(a: FolderInfo[], b: FolderInfo[]): boolean {
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) {
    const fa = a[i]!;
    const fb = b[i]!;
    if (fa.id !== fb.id || fa.name !== fb.name || fa.folderColor !== fb.folderColor) return false;
    if (!arrayEquals(fa.projectIds, fb.projectIds)) return false;
  }
  return true;
}

function arrayEquals<T>(a: readonly T[], b: readonly T[]): boolean {
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) if (a[i] !== b[i]) return false;
  return true;
}

/** Resolve the selected project from a project list + selected id (Dart `selectedProject`). */
function resolveSelectedProject(
  projects: ProjectInfo[],
  selectedProjectId: ProjectId | null,
): ProjectInfo | null {
  if (selectedProjectId === null) return projects[0] ?? null;
  return projects.find((p) => p.id === selectedProjectId) ?? projects[0] ?? null;
}

/** One poll tick — refresh remote state + run auto-selection. Mirrors `_pollState`. */
function pollState(
  get: StoreApi<WorkspaceState>['getState'],
  set: StoreApi<WorkspaceState>['setState'],
): void {
  const connId = activeConnId;
  if (!connId) return;
  const native = deps().native;

  const newProjects = native.getProjects(connId);
  const focusedId = native.getFocusedProjectId(connId);
  const newFolders = native.getFolders(connId);
  const newProjectOrder = native.getProjectOrder(connId);
  const newFullscreen = native.getFullscreenTerminal(connId) ?? null;

  const prev = get();
  const patch: Partial<WorkspaceState> = {};
  let changed = false;

  if (!projectListEquals(newProjects, prev.projects)) {
    patch.projects = newProjects;
    changed = true;
  }
  if (!folderListEquals(newFolders, prev.folders)) {
    patch.folders = newFolders;
    changed = true;
  }
  if (!arrayEquals(newProjectOrder, prev.projectOrder)) {
    patch.projectOrder = newProjectOrder;
    changed = true;
  }
  if (newFullscreen?.terminalId !== prev.fullscreenTerminal?.terminalId) {
    patch.fullscreenTerminal = newFullscreen;
    changed = true;
  }

  // Work against the freshest values for the auto-select logic below.
  const projects = patch.projects ?? prev.projects;
  let selectedProjectId = prev.selectedProjectId;
  let selectedTerminalId = prev.selectedTerminalId;

  // Reopen a remembered selection once the project list is known. Until then,
  // hold off the focused-project fallback so it can't win the race.
  if (pendingRestore && projects.length > 0) {
    const outcome = resolveRestore(projects, pendingRestore);
    pendingRestore = null;
    if (outcome?.kind === 'restored') {
      selectedProjectId = outcome.projectId;
      selectedTerminalId = outcome.terminalId;
      patch.selectedProjectId = selectedProjectId;
      patch.selectedTerminalId = selectedTerminalId;
      previousTerminalIds = null;
      changed = true;
    } else if (outcome?.kind === 'projectMissing') {
      patch.restoreProjectMissing = true;
      changed = true;
    }
  }

  // Auto-select the focused project if nothing is selected.
  if (selectedProjectId === null && focusedId && !pendingRestore) {
    selectedProjectId = focusedId;
    patch.selectedProjectId = focusedId;
    changed = true;
  }

  // Auto-select a terminal: pick a newly-added one, or the first if the current
  // selection is gone (matches the Dart logic + `_previousTerminalIds`).
  const project = resolveSelectedProject(projects, selectedProjectId);
  if (project && project.terminalIds.length > 0) {
    if (selectedTerminalId === null || !project.terminalIds.includes(selectedTerminalId)) {
      selectedTerminalId = pickTerminal(project.terminalIds, [prev.terminalByProject[project.id]]);
      patch.selectedTerminalId = selectedTerminalId;
      changed = true;
    } else if (previousTerminalIds !== null) {
      const newIds = project.terminalIds.filter((id) => !previousTerminalIds!.has(id));
      if (newIds.length > 0) {
        selectedTerminalId = newIds[newIds.length - 1]!;
        patch.selectedTerminalId = selectedTerminalId;
        changed = true;
      }
    }
    previousTerminalIds = new Set(project.terminalIds);
    const remembered = rememberTerminal(prev.terminalByProject, project.id, selectedTerminalId);
    if (remembered !== prev.terminalByProject) {
      patch.terminalByProject = remembered;
      changed = true;
    }
  } else {
    previousTerminalIds = null;
    if (selectedTerminalId !== null) {
      patch.selectedTerminalId = null;
      changed = true;
    }
  }

  const layoutJson =
    (project ? native.getProjectLayoutJson(connId, project.id) : undefined) ?? null;
  if (layoutJson !== prev.projectLayoutJson) {
    patch.projectLayoutJson = layoutJson;
    changed = true;
  }

  // Activity signals. The shown terminal is the validated selection above.
  lastActivity = native.getTerminalActivity(connId);
  const signals = deriveSignals(projects, project ? selectedTerminalId : null, connId);
  if (!signalsEqual(signals, prev.projectSignals)) {
    patch.projectSignals = signals;
    changed = true;
  }

  // Connection health (drives the staleness indicator's 3s / 10s thresholds).
  const newActivity = native.secondsSinceActivity(connId);
  const oldActivity = prev.secondsSinceActivity;
  if (
    (oldActivity < 3) !== (newActivity < 3) ||
    (oldActivity < 10) !== (newActivity < 10)
  ) {
    changed = true;
  }
  patch.secondsSinceActivity = newActivity;

  if (changed) set(patch);
  else if (patch.secondsSinceActivity !== undefined) {
    // Always keep the raw activity number current, even when no UI-visible
    // threshold crossed (cheap, avoids a stale value).
    set({ secondsSinceActivity: newActivity });
  }
}

/** `projectSignals` patch after showing `terminalId` (empty when unchanged). */
function signalsPatch(
  get: StoreApi<WorkspaceState>['getState'],
  projects: ProjectInfo[],
  terminalId: TerminalId | null,
  connId: ConnId,
): Partial<WorkspaceState> {
  const signals = deriveSignals(projects, terminalId, connId);
  return signalsEqual(signals, get().projectSignals) ? {} : { projectSignals: signals };
}

/**
 * The workspace store hook + bound store. Module-level (construction is
 * side-effect-free w.r.t. the native module — see the connection store for the
 * same reasoning). Use it like any zustand hook, plus `.getState()` for
 * imperative access:
 *
 * ```ts
 * const projects = useWorkspaceStore((s) => s.projects);
 * const selectProject = useWorkspaceStore((s) => s.selectProject);
 * ```
 */
export const useWorkspaceStore: UseBoundStore<StoreApi<WorkspaceState>> =
  create<WorkspaceState>((set, get) => ({
    projects: [],
    folders: [],
    projectOrder: [],
    fullscreenTerminal: null,
    projectLayoutJson: null,
    selectedProjectId: null,
    selectedTerminalId: null,
    secondsSinceActivity: 0,
    terminalByProject: {},
    projectSignals: {},
    previousProjectId: null,
    restoreProjectMissing: false,

    start(connId, restore) {
      if (pollTimer !== null && activeConnId === connId) return; // already polling this conn
      stopPolling();
      activeConnId = connId;
      if (restore) {
        set({ terminalByProject: { ...restore.terminalByProject } });
        if (restore.projectId !== null) pendingRestore = restore;
      }
      pollTimer = setInterval(() => pollState(get, set), WORKSPACE_POLL_MS);
      pollState(get, set); // immediate first poll
    },

    stop() {
      stopPolling();
      set({
        projects: [],
        folders: [],
        projectOrder: [],
        fullscreenTerminal: null,
        projectLayoutJson: null,
        selectedProjectId: null,
        selectedTerminalId: null,
        terminalByProject: {},
        projectSignals: {},
        previousProjectId: null,
        restoreProjectMissing: false,
      });
    },

    selectProject(projectId) {
      previousTerminalIds = null;
      const { projects, terminalByProject, selectedProjectId } = get();
      const project = projects.find((p) => p.id === projectId);
      // Reopen the terminal last used in this project (else its first); the
      // next poll re-validates it against the live terminal list.
      const terminalId = project
        ? pickTerminal(project.terminalIds, [terminalByProject[projectId]])
        : null;
      const connId = activeConnId;
      set({
        selectedProjectId: projectId,
        selectedTerminalId: terminalId,
        previousProjectId:
          selectedProjectId !== null && selectedProjectId !== projectId
            ? selectedProjectId
            : get().previousProjectId,
        // Swap the layout with the project so the old tree isn't shown until
        // the next poll.
        projectLayoutJson: connId
          ? (deps().native.getProjectLayoutJson(connId, projectId) ?? null)
          : null,
        ...(connId ? signalsPatch(get, projects, terminalId, connId) : {}),
      });
      // Opening a project is activity: the daemon stamps `lastActivityAt`
      // (`RecordProjectActivity`), which the activity order sorts by.
      if (connId && terminalId !== null && selectedProjectId !== projectId) {
        deps()
          .native.focusTerminal(connId, projectId, terminalId)
          .catch(() => {
            // Ordering only; not worth surfacing.
          });
      }
    },

    selectTerminal(terminalId) {
      const project = get().getSelectedProject();
      const connId = activeConnId;
      set({
        selectedTerminalId: terminalId,
        terminalByProject: project
          ? rememberTerminal(get().terminalByProject, project.id, terminalId)
          : get().terminalByProject,
        ...(connId ? signalsPatch(get, get().projects, terminalId, connId) : {}),
      });
    },

    clearRestoreProjectMissing() {
      if (get().restoreProjectMissing) set({ restoreProjectMissing: false });
    },

    getSelectedProject() {
      const { projects, selectedProjectId } = get();
      return resolveSelectedProject(projects, selectedProjectId);
    },
  }));

/** Imperative store handle for non-React consumers (e.g. App.tsx lifecycle wiring). */
export function workspaceStore(): StoreApi<WorkspaceState> {
  return useWorkspaceStore;
}
