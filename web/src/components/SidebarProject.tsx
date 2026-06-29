import { useCallback, useState } from "react";
import type { ApiProject, ApiServiceInfo } from "../api/types";
import { useApp } from "../state/store";
import { postAction } from "../api/client";
import { collectTerminalIds } from "../utils/layout";

export function SidebarProject({
  project,
  selected,
  isMobile,
  worktrees = [],
  depth = 0,
}: {
  project: ApiProject;
  selected: boolean;
  isMobile: boolean;
  worktrees?: ApiProject[];
  depth?: number;
}) {
  const { state, dispatch } = useApp();
  const [expanded, setExpanded] = useState(selected);

  const terminalIds = collectTerminalIds(project.layout);
  const services = project.services ?? [];

  const selectProject = useCallback(() => {
    dispatch({ type: "select_project", projectId: project.id });
    setExpanded((prev) => !prev);
  }, [dispatch, project.id]);

  const selectTerminal = useCallback(
    (projectId: string, terminalId: string) => {
      dispatch({ type: "select_project", projectId });
      dispatch({ type: "select_terminal", terminalId });
      if (isMobile) {
        dispatch({ type: "set_sidebar_open", open: false });
      } else {
        postAction({ action: "focus_terminal", project_id: projectId, terminal_id: terminalId }).catch(() => {});
      }
    },
    [dispatch, isMobile],
  );

  const selectWorktree = useCallback(
    (worktree: ApiProject) => {
      const firstTerminalId = collectTerminalIds(worktree.layout)[0] ?? null;
      dispatch({ type: "select_project", projectId: worktree.id });
      dispatch({ type: "select_terminal", terminalId: firstTerminalId });
      if (isMobile) {
        dispatch({ type: "set_sidebar_open", open: false });
      } else if (firstTerminalId) {
        postAction({ action: "focus_terminal", project_id: worktree.id, terminal_id: firstTerminalId }).catch(() => {});
      }
    },
    [dispatch, isMobile],
  );

  const createTerminal = useCallback(
    (e: React.MouseEvent) => {
      e.stopPropagation();
      postAction({ action: "create_terminal", project_id: project.id }).catch(() => {});
    },
    [project.id],
  );

  const togglePinned = useCallback(
    (e: React.MouseEvent) => {
      e.stopPropagation();
      postAction({ action: "toggle_project_pinned", project_id: project.id }).catch(() => {});
    },
    [project.id],
  );

  const toggleVisible = useCallback(
    (e: React.MouseEvent) => {
      e.stopPropagation();
      postAction({
        action: "set_project_show_in_overview",
        project_id: project.id,
        show: !project.show_in_overview,
      }).catch(() => {});
    },
    [project.id, project.show_in_overview],
  );

  const runServiceAction = useCallback(
    (service: ApiServiceInfo, action: "start_service" | "stop_service" | "restart_service") => {
      postAction({
        action,
        project_id: project.id,
        service_name: service.name,
      }).catch(() => {});
    },
    [project.id],
  );

  const reloadServices = useCallback(() => {
    postAction({ action: "reload_services", project_id: project.id }).catch(() => {});
  }, [project.id]);

  const isExpanded = expanded || selected;
  const hasChildren = terminalIds.length > 0 || services.length > 0 || worktrees.length > 0;
  const muted = !project.show_in_overview;

  return (
    <div className={muted ? "opacity-70" : undefined}>
      <div className="flex items-center gap-0.5" style={{ paddingLeft: depth * 10 }}>
        <button
          onClick={selectProject}
          className={`flex min-w-0 flex-1 items-center gap-1 rounded-[3px] px-2 py-1.5 text-left text-[12px] transition-colors
            ${selected ? "bg-[var(--ok-selection)] text-white" : "text-[var(--ok-text-secondary)] hover:bg-[var(--ok-hover)] hover:text-[var(--ok-text)]"}`}
        >
          <span
            className="w-3 flex-shrink-0 text-[10px] text-[var(--ok-text-muted)] transition-transform duration-150"
            style={{ transform: isExpanded ? "rotate(90deg)" : "rotate(0deg)" }}
          >
            {hasChildren ? "▶" : ""}
          </span>
          <span className="truncate">{project.name}</span>
          {project.pinned && <span className="text-[10px] text-[var(--ok-text-muted)]">pin</span>}
          {worktrees.length > 0 && <span className="text-[10px] text-[var(--ok-text-muted)]">wt {worktrees.length}</span>}
        </button>
        <button
          onClick={createTerminal}
          className="icon-button h-[22px] w-[22px] flex-shrink-0"
          title="New terminal"
          aria-label="New terminal"
        >
          +
        </button>
        <button
          onClick={togglePinned}
          className={`icon-button h-[22px] w-[22px] flex-shrink-0 ${
            project.pinned ? "text-[var(--ok-text)]" : ""
          }`}
          title={project.pinned ? "Unpin project" : "Pin project"}
          aria-label={project.pinned ? "Unpin project" : "Pin project"}
        >
          P
        </button>
        <button
          onClick={toggleVisible}
          className="icon-button h-[22px] w-[22px] flex-shrink-0"
          title={project.show_in_overview ? "Hide from overview" : "Show in overview"}
          aria-label={project.show_in_overview ? "Hide from overview" : "Show in overview"}
        >
          {project.show_in_overview ? "H" : "S"}
        </button>
      </div>

      {isExpanded && (
        <div className="ml-4 mt-0.5 space-y-0.5">
          {terminalIds.map((terminalId) => {
            const name = project.terminal_names[terminalId] ?? "Terminal";
            const isSelected = terminalId === state.selectedTerminalId && selected;
            return (
              <button
                key={terminalId}
                onClick={() => selectTerminal(project.id, terminalId)}
                className={`w-full truncate rounded-[3px] px-2 py-1 text-left text-[11px] transition-colors
                  ${isSelected ? "bg-[var(--ok-header)] text-[var(--ok-text)]" : "text-[var(--ok-text-muted)] hover:bg-[var(--ok-hover)] hover:text-[var(--ok-text-secondary)]"}`}
              >
                {name}
              </button>
            );
          })}

          {services.length > 0 && (
            <div className="pt-1">
              <div className="flex items-center px-2 pb-0.5 text-[10px] text-[var(--ok-text-muted)]">
                <span>services</span>
                <button
                  onClick={reloadServices}
                  className="ml-auto hover:text-[var(--ok-text-secondary)]"
                  title="Reload services"
                >
                  reload
                </button>
              </div>
              <div className="space-y-0.5">
                {services.map((service) => (
                  <ServiceRow
                    key={`${service.kind ?? "service"}:${service.name}`}
                    service={service}
                    onOpenTerminal={(terminalId) => selectTerminal(project.id, terminalId)}
                    onStart={() => runServiceAction(service, "start_service")}
                    onStop={() => runServiceAction(service, "stop_service")}
                    onRestart={() => runServiceAction(service, "restart_service")}
                  />
                ))}
              </div>
            </div>
          )}

          {worktrees.length > 0 && (
            <div className="pt-1">
              <div className="px-2 pb-0.5 text-[10px] text-[var(--ok-text-muted)]">worktrees</div>
              <div className="space-y-0.5">
                {worktrees.map((worktree) => (
                  <button
                    key={worktree.id}
                    onClick={() => selectWorktree(worktree)}
                    className={`w-full truncate rounded-[3px] px-2 py-1 text-left text-[11px] transition-colors
                      ${worktree.id === state.selectedProjectId ? "bg-[var(--ok-header)] text-[var(--ok-text)]" : "text-[var(--ok-text-muted)] hover:bg-[var(--ok-hover)] hover:text-[var(--ok-text-secondary)]"}`}
                  >
                    <span className="truncate">{worktree.name}</span>
                    {worktree.git_status?.branch && (
                      <span className="ml-1 text-[var(--ok-text-muted)]">{worktree.git_status.branch}</span>
                    )}
                  </button>
                ))}
              </div>
            </div>
          )}
        </div>
      )}
    </div>
  );
}

function ServiceRow({
  service,
  onOpenTerminal,
  onStart,
  onStop,
  onRestart,
}: {
  service: ApiServiceInfo;
  onOpenTerminal: (terminalId: string) => void;
  onStart: () => void;
  onStop: () => void;
  onRestart: () => void;
}) {
  const status = service.status.toLowerCase();
  const canStart = status === "stopped" || status === "crashed";
  const canStop = status === "running" || status === "starting" || status === "restarting";
  const ports = service.ports?.length ? `:${service.ports.join(",")}` : "";
  const crash = service.exit_code != null ? ` exit ${service.exit_code}` : "";

  return (
    <div className="flex items-center gap-1 rounded-[3px] px-2 py-1 text-[11px] text-[var(--ok-text-muted)] hover:bg-[var(--ok-hover)]">
      <button
        className="min-w-0 flex-1 truncate text-left hover:text-[var(--ok-text-secondary)]"
        onClick={() => service.terminal_id && onOpenTerminal(service.terminal_id)}
        disabled={!service.terminal_id}
        title={service.terminal_id ? "Open service terminal" : undefined}
      >
        <span className="text-[var(--ok-text-secondary)]">{service.name}</span>
        <span className="ml-1 text-[var(--ok-text-muted)]">{status}{ports}{crash}</span>
      </button>
      {canStart ? (
        <button className="hover:text-[var(--ok-green)]" onClick={onStart} title="Start service">
          start
        </button>
      ) : (
        <button className="hover:text-[var(--ok-red)] disabled:opacity-30" onClick={onStop} disabled={!canStop} title="Stop service">
          stop
        </button>
      )}
      <button className="hover:text-[var(--ok-text-secondary)]" onClick={onRestart} title="Restart service">
        restart
      </button>
    </div>
  );
}
