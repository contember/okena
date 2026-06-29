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
          className={`min-w-0 flex-1 text-left px-2 py-1.5 rounded text-sm transition-colors flex items-center gap-1
            ${selected ? "bg-zinc-700 text-zinc-100" : "text-zinc-400 hover:bg-zinc-800 hover:text-zinc-200"}`}
        >
          <span
            className="w-3 text-[10px] transition-transform duration-150 flex-shrink-0"
            style={{ transform: isExpanded ? "rotate(90deg)" : "rotate(0deg)" }}
          >
            {hasChildren ? "▶" : ""}
          </span>
          <span className="truncate">{project.name}</span>
          {project.pinned && <span className="text-[10px] text-zinc-500">pin</span>}
          {worktrees.length > 0 && <span className="text-[10px] text-zinc-500">wt {worktrees.length}</span>}
        </button>
        <button
          onClick={createTerminal}
          className="flex-shrink-0 px-1 py-1 text-zinc-500 hover:text-zinc-200 hover:bg-zinc-800 rounded text-xs leading-none"
          title="New terminal"
        >
          +
        </button>
        <button
          onClick={togglePinned}
          className={`flex-shrink-0 px-1 py-1 hover:bg-zinc-800 rounded text-[10px] leading-none ${
            project.pinned ? "text-zinc-200" : "text-zinc-600 hover:text-zinc-300"
          }`}
          title={project.pinned ? "Unpin project" : "Pin project"}
        >
          P
        </button>
        <button
          onClick={toggleVisible}
          className="flex-shrink-0 px-1 py-1 text-zinc-600 hover:text-zinc-300 hover:bg-zinc-800 rounded text-[10px] leading-none"
          title={project.show_in_overview ? "Hide from overview" : "Show in overview"}
        >
          {project.show_in_overview ? "on" : "off"}
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
                className={`w-full text-left px-2 py-1 rounded text-xs truncate transition-colors
                  ${isSelected ? "bg-zinc-600 text-zinc-100" : "text-zinc-500 hover:bg-zinc-800 hover:text-zinc-300"}`}
              >
                {name}
              </button>
            );
          })}

          {services.length > 0 && (
            <div className="pt-1">
              <div className="flex items-center px-2 pb-0.5 text-[10px] text-zinc-600">
                <span>services</span>
                <button
                  onClick={reloadServices}
                  className="ml-auto text-zinc-600 hover:text-zinc-300"
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
              <div className="px-2 pb-0.5 text-[10px] text-zinc-600">worktrees</div>
              <div className="space-y-0.5">
                {worktrees.map((worktree) => (
                  <button
                    key={worktree.id}
                    onClick={() => selectWorktree(worktree)}
                    className={`w-full text-left px-2 py-1 rounded text-xs truncate transition-colors
                      ${worktree.id === state.selectedProjectId ? "bg-zinc-600 text-zinc-100" : "text-zinc-500 hover:bg-zinc-800 hover:text-zinc-300"}`}
                  >
                    <span className="truncate">{worktree.name}</span>
                    {worktree.git_status?.branch && (
                      <span className="ml-1 text-zinc-600">{worktree.git_status.branch}</span>
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
    <div className="flex items-center gap-1 px-2 py-1 rounded text-xs text-zinc-500 hover:bg-zinc-800/70">
      <button
        className="min-w-0 flex-1 text-left truncate hover:text-zinc-300"
        onClick={() => service.terminal_id && onOpenTerminal(service.terminal_id)}
        disabled={!service.terminal_id}
        title={service.terminal_id ? "Open service terminal" : undefined}
      >
        <span className="text-zinc-400">{service.name}</span>
        <span className="ml-1 text-zinc-600">{status}{ports}{crash}</span>
      </button>
      {canStart ? (
        <button className="text-zinc-600 hover:text-zinc-300" onClick={onStart} title="Start service">
          start
        </button>
      ) : (
        <button className="text-zinc-600 hover:text-zinc-300" onClick={onStop} disabled={!canStop} title="Stop service">
          stop
        </button>
      )}
      <button className="text-zinc-600 hover:text-zinc-300" onClick={onRestart} title="Restart service">
        restart
      </button>
    </div>
  );
}
