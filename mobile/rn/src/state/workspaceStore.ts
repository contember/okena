/**
 * workspaceStore.ts — project / folder / layout state (zustand).
 *
 * Ports `mobile/lib/src/providers/workspace_provider.dart`. While connected it
 * polls the cached remote state (~1s) for:
 *   - the project list (`getProjects`),
 *   - folders (`getFolders`),
 *   - project order (`getProjectOrder`),
 *   - the server's focused project (`getFocusedProjectId`),
 *   - the fullscreen terminal (`getFullscreenTerminal`).
 *
 * It auto-selects the focused project when nothing is selected, and auto-selects
 * a terminal within the selected project (newly-added one, or the first if the
 * current selection vanished) — mirroring the Dart `_pollState` logic.
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

/** Poll interval (ms) for the workspace state — Dart used 1000ms. */
export const WORKSPACE_POLL_MS = 1000;

/** Dependencies the store calls out to. Overridable for tests. */
export interface WorkspaceDeps {
  native: OkenaNative;
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
  /** Currently-selected project id (auto-selected from focused, or by the user). */
  selectedProjectId: ProjectId | null;
  /** Currently-selected terminal id within the selected project. */
  selectedTerminalId: TerminalId | null;
  /** Seconds since last WS activity (also tracked here, per the Dart provider). */
  secondsSinceActivity: number;

  // ── actions ────────────────────────────────────────────────────────────
  /**
   * Start polling for the given connection. Idempotent: re-calling with a new
   * `connId` restarts against it. Does an immediate first poll (as Dart did).
   * Call when the connection becomes `connected`.
   */
  start(connId: ConnId): void;
  /**
   * Stop polling and clear all workspace state. Call when the connection is no
   * longer connected (mirrors the Dart `_onConnectionChanged` else-branch).
   */
  stop(): void;
  /** Select a project; clears the terminal selection so it re-auto-selects. Mirrors `selectProject`. */
  selectProject(projectId: ProjectId): void;
  /** Select a terminal within the current project. Mirrors `selectTerminal`. */
  selectTerminal(terminalId: TerminalId): void;
  /**
   * The currently-selected project object, or the first project as a fallback,
   * or `null`. Mirrors the Dart `selectedProject` getter. (Provided as a
   * selector helper since zustand state holds only the id.)
   */
  getSelectedProject(): ProjectInfo | null;
  /**
   * The layout JSON for the selected project, via the native module. `null` if
   * not connected or no project selected. Mirrors `getProjectLayoutJson`.
   * Parse it with {@link import('../models/layoutNode').parseLayout}.
   */
  getProjectLayoutJson(): string | undefined;
}

let injectedDeps: Partial<WorkspaceDeps> = {};
let resolvedDeps: WorkspaceDeps | null = null;

function deps(): WorkspaceDeps {
  if (!resolvedDeps) {
    resolvedDeps = { native: injectedDeps.native ?? getOkenaNative() };
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

function stopPolling(): void {
  if (pollTimer !== null) {
    clearInterval(pollTimer);
    pollTimer = null;
  }
  activeConnId = null;
  previousTerminalIds = null;
}

/** Shallow id+name+git+services equality for the project list (mirrors `_projectListEquals`). */
function projectListEquals(a: ProjectInfo[], b: ProjectInfo[]): boolean {
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) {
    const pa = a[i]!;
    const pb = b[i]!;
    if (pa.id !== pb.id || pa.name !== pb.name) return false;
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
  if (
    !arrayEquals(
      newFolders.map((f) => f.id),
      prev.folders.map((f) => f.id),
    )
  ) {
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

  // Auto-select the focused project if nothing is selected.
  if (selectedProjectId === null && focusedId) {
    selectedProjectId = focusedId;
    patch.selectedProjectId = focusedId;
    changed = true;
  }

  // Auto-select a terminal: pick a newly-added one, or the first if the current
  // selection is gone (matches the Dart logic + `_previousTerminalIds`).
  const project = resolveSelectedProject(projects, selectedProjectId);
  if (project && project.terminalIds.length > 0) {
    if (selectedTerminalId === null || !project.terminalIds.includes(selectedTerminalId)) {
      selectedTerminalId = project.terminalIds[0]!;
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
  } else {
    previousTerminalIds = null;
    if (selectedTerminalId !== null) {
      patch.selectedTerminalId = null;
      changed = true;
    }
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
    selectedProjectId: null,
    selectedTerminalId: null,
    secondsSinceActivity: 0,

    start(connId) {
      if (pollTimer !== null && activeConnId === connId) return; // already polling this conn
      stopPolling();
      activeConnId = connId;
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
        selectedProjectId: null,
        selectedTerminalId: null,
      });
    },

    selectProject(projectId) {
      previousTerminalIds = null;
      set({ selectedProjectId: projectId, selectedTerminalId: null });
    },

    selectTerminal(terminalId) {
      set({ selectedTerminalId: terminalId });
    },

    getSelectedProject() {
      const { projects, selectedProjectId } = get();
      return resolveSelectedProject(projects, selectedProjectId);
    },

    getProjectLayoutJson() {
      if (!activeConnId) return undefined;
      const project = get().getSelectedProject();
      if (!project) return undefined;
      return deps().native.getProjectLayoutJson(activeConnId, project.id);
    },
  }));

/** Imperative store handle for non-React consumers (e.g. App.tsx lifecycle wiring). */
export function workspaceStore(): StoreApi<WorkspaceState> {
  return useWorkspaceStore;
}
