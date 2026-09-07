import { useEffect, useRef, useState } from "react";
import { useApp } from "../state/store";
import { postAction } from "../api/client";
import type { ApiProject } from "../api/types";
import { useIsMobile } from "../hooks/useIsMobile";
import { collectTerminalIds } from "../utils/layout";
import { Sidebar } from "./Sidebar";
import { TerminalArea } from "./TerminalArea";
import { MobileWorkspace } from "./MobileWorkspace";
import { StatusBar } from "./StatusBar";
import { GitPanel } from "./GitPanel";
import { FileViewerModal } from "./FileViewerModal";

export function WorkspaceLayout() {
  const isMobile = useIsMobile();
  return isMobile ? <MobileWorkspace /> : <DesktopLayout />;
}

function DesktopLayout() {
  const { state } = useApp();
  const [fileViewerProjectId, setFileViewerProjectId] = useState<string | null>(
    null,
  );
  const projects = resolveOverviewProjects(
    state.workspace?.projects ?? [],
    state.selectedProjectId,
  );
  const fileViewerProject = state.workspace?.projects.find(
    (project) => project.id === fileViewerProjectId,
  );
  useRequestGitPollForVisibleProjects(projects, state.selectedProjectId);

  return (
    <div className="app-shell flex h-full overflow-hidden flex-col">
      <div className="flex flex-1 min-h-0">
        <aside className="app-sidebar w-64 flex-shrink-0 border-r">
          <Sidebar />
        </aside>
        <main className="project-overview flex min-w-0 flex-1 flex-col">
          <div className="project-strip">
            {state.workspace ? (
              projects.length > 0 ? (
                projects.map((project) => (
                  <ProjectColumn
                    key={project.id}
                    project={project}
                    selected={project.id === state.selectedProjectId}
                    onOpenFiles={() => setFileViewerProjectId(project.id)}
                  />
                ))
              ) : (
                <OverviewEmptyState label="No projects in this workspace" />
              )
            ) : (
              <OverviewEmptyState label="Loading workspace..." />
            )}
          </div>
        </main>
      </div>
      <StatusBar />
      {fileViewerProject && (
        <FileViewerModal
          project={fileViewerProject}
          onClose={() => setFileViewerProjectId(null)}
        />
      )}
    </div>
  );
}

function useRequestGitPollForVisibleProjects(
  projects: readonly ApiProject[],
  selectedProjectId: string | null,
): void {
  const previousVisibleProjectIds = useRef<Set<string>>(new Set());

  useEffect(() => {
    const nextVisibleProjectIds = new Set(
      projects.map((project) => project.id),
    );
    for (const project of projects) {
      if (previousVisibleProjectIds.current.has(project.id)) {
        continue;
      }
      requestGitPoll(project.id);
    }
    previousVisibleProjectIds.current = nextVisibleProjectIds;
  }, [projects]);

  useEffect(() => {
    if (selectedProjectId) {
      requestGitPoll(selectedProjectId);
    }
  }, [selectedProjectId]);
}

function requestGitPoll(projectId: string): void {
  postAction({ action: "git_status", project_id: projectId }).catch(() => {});
}

function ProjectColumn({
  project,
  selected,
  onOpenFiles,
}: {
  project: ApiProject;
  selected: boolean;
  onOpenFiles: () => void;
}) {
  const { dispatch } = useApp();
  const accent = folderAccent(project.folder_color);
  const terminalCount = collectTerminalIds(project.layout).length;

  return (
    <section
      className="project-column flex min-h-0 flex-col"
      data-selected={selected}
      onMouseDown={() =>
        dispatch({ type: "select_project", projectId: project.id })
      }
    >
      <div className="h-px flex-shrink-0" style={{ backgroundColor: accent }} />
      <header className="project-header flex min-h-[44px] flex-shrink-0 items-center gap-3 border-b px-3 py-2">
        <div className="min-w-0 flex-1">
          <div className="flex min-w-0 items-center gap-2">
            <span
              className="h-2 w-2 flex-shrink-0 rounded-full"
              style={{ backgroundColor: accent }}
            />
            <h2 className="truncate text-[13px] font-bold leading-4 text-[var(--ok-text)]">
              {project.name}
            </h2>
            {project.pinned && (
              <span className="rounded-[3px] border border-[var(--ok-border)] px-1 text-[10px] text-[var(--ok-text-muted)]">
                pin
              </span>
            )}
          </div>
          <div className="mt-1 flex min-w-0 items-center gap-2 text-[10px] text-[var(--ok-text-muted)]">
            <span className="truncate">{compactPath(project.path)}</span>
            <span>
              {terminalCount} term{terminalCount === 1 ? "" : "s"}
            </span>
          </div>
        </div>
        <div className="flex flex-shrink-0 items-center gap-1">
          <button
            className="rounded-[3px] px-2 py-1 text-[11px] text-[var(--ok-text-muted)] hover:bg-[var(--ok-hover)] hover:text-[var(--ok-text)]"
            title="Open files"
            onMouseDown={(event) => event.stopPropagation()}
            onClick={onOpenFiles}
          >
            files
          </button>
          <button
            className="icon-button"
            title="New terminal"
            aria-label="New terminal"
            onMouseDown={(event) => event.stopPropagation()}
            onClick={() =>
              postAction({
                action: "create_terminal",
                project_id: project.id,
              }).catch(() => {})
            }
          >
            +
          </button>
          <button
            className="icon-button"
            title={
              project.show_in_overview
                ? "Hide from overview"
                : "Show in overview"
            }
            aria-label={
              project.show_in_overview
                ? "Hide from overview"
                : "Show in overview"
            }
            onMouseDown={(event) => event.stopPropagation()}
            onClick={() =>
              postAction({
                action: "set_project_show_in_overview",
                project_id: project.id,
                show: !project.show_in_overview,
              }).catch(() => {})
            }
          >
            {project.show_in_overview ? "H" : "S"}
          </button>
        </div>
      </header>
      <GitPanel project={project} />
      <div className="min-h-0 flex-1">
        {project.layout ? (
          <TerminalArea layout={project.layout} project={project} />
        ) : (
          <ProjectEmptyState project={project} />
        )}
      </div>
    </section>
  );
}

function ProjectEmptyState({ project }: { project: ApiProject }) {
  return (
    <div className="flex h-full items-center justify-center bg-[var(--ok-panel)] px-4 text-center">
      <div className="max-w-72">
        <div className="mb-2 text-[11px] text-[var(--ok-text-muted)]">
          empty project
        </div>
        <button
          className="border border-[var(--ok-border)] bg-[var(--ok-header)] px-3 py-2 text-[12px] text-[var(--ok-text)] hover:bg-[var(--ok-hover)]"
          onClick={() =>
            postAction({
              action: "create_terminal",
              project_id: project.id,
            }).catch(() => {})
          }
        >
          New Terminal
        </button>
      </div>
    </div>
  );
}

function OverviewEmptyState({ label }: { label: string }) {
  return (
    <div className="flex h-full flex-1 items-center justify-center text-[12px] text-[var(--ok-text-muted)]">
      {label}
    </div>
  );
}

function resolveOverviewProjects(
  projects: ApiProject[],
  selectedProjectId: string | null,
): ApiProject[] {
  const visible = projects.filter((project) => project.show_in_overview);
  if (!selectedProjectId) {
    return visible.length > 0 ? visible : projects.slice(0, 1);
  }

  const selected = projects.find((project) => project.id === selectedProjectId);
  if (!selected) {
    return visible.length > 0 ? visible : projects.slice(0, 1);
  }

  if (visible.some((project) => project.id === selectedProjectId)) {
    return visible;
  }

  return [selected, ...visible];
}

function compactPath(path: string): string {
  const parts = path.split("/").filter(Boolean);
  if (parts.length <= 3) return path;
  return `.../${parts.slice(-3).join("/")}`;
}

function folderAccent(color: string | undefined): string {
  switch (color) {
    case "red":
      return "#e06c75";
    case "orange":
      return "#d19a66";
    case "yellow":
      return "#e5c07b";
    case "lime":
      return "#a3d955";
    case "green":
      return "#98c379";
    case "teal":
      return "#2fbda0";
    case "cyan":
      return "#56d7e5";
    case "blue":
      return "#61afef";
    case "indigo":
      return "#818cf8";
    case "purple":
      return "#c678dd";
    case "pink":
      return "#e06c9f";
    default:
      return "#8a9199";
  }
}
