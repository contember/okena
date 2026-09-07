import { useCallback, useEffect, useMemo, useState } from "react";
import { postAction } from "../api/client";
import type { ApiProject, ApiServiceInfo, SpecialKey } from "../api/types";
import { useApp } from "../state/store";
import { useMobileVisualViewport } from "../hooks/useMobileVisualViewport";
import { collectTerminalIds } from "../utils/layout";
import { buildSidebarItems } from "../utils/sidebar";
import { FilesPanel } from "./FilesPanel";
import { GitPanel } from "./GitPanel";
import { TerminalPane } from "./TerminalPane";

type MobilePage =
  | "current"
  | "projects"
  | "project"
  | "terminals"
  | "more"
  | "files"
  | "git"
  | "services";

type PrimaryPage = "current" | "projects" | "terminals" | "more";
type IconName =
  | "terminal"
  | "projects"
  | "more"
  | "search"
  | "back"
  | "chevron"
  | "chevronDown"
  | "plus"
  | "keyboard"
  | "close"
  | "files"
  | "git"
  | "services";

type TerminalEntry = {
  project: ApiProject;
  terminalId: string;
  name: string;
};

const PRIMARY_PAGES: readonly PrimaryPage[] = [
  "current",
  "projects",
  "terminals",
  "more",
];

const MOBILE_RECENT_TERMINALS_KEY = "okena.mobile.recent-terminals";

const SPECIAL_KEYS: ReadonlyArray<{ label: string; key: SpecialKey }> = [
  { label: "esc", key: "Escape" },
  { label: "tab", key: "Tab" },
  { label: "^C", key: "CtrlC" },
  { label: "^D", key: "CtrlD" },
  { label: "↑", key: "ArrowUp" },
  { label: "↓", key: "ArrowDown" },
  { label: "←", key: "ArrowLeft" },
  { label: "→", key: "ArrowRight" },
];

export function MobileWorkspace() {
  useMobileVisualViewport();
  const { state, dispatch } = useApp();
  const [page, setPage] = useState<MobilePage>("current");
  const [detailBackPage, setDetailBackPage] = useState<PrimaryPage>("projects");
  const [toolBackPage, setToolBackPage] = useState<"project" | "more">("more");
  const [contextOpen, setContextOpen] = useState(false);
  const [keysOpen, setKeysOpen] = useState(false);
  const [projectQuery, setProjectQuery] = useState("");
  const [terminalQuery, setTerminalQuery] = useState("");
  const [notice, setNotice] = useState<string | null>(null);

  const projects = state.workspace?.projects ?? [];
  const selectedProject = projects.find(
    (project) => project.id === state.selectedProjectId,
  );
  const selectedTerminalId = state.selectedTerminalId;
  const selectedTerminalName =
    selectedProject && selectedTerminalId
      ? (selectedProject.terminal_names[selectedTerminalId] ?? "Terminal")
      : null;

  const { entries: allTerminals, markUsed: markTerminalUsed } =
    useRecentTerminals(projects, selectedTerminalId);

  useEffect(() => {
    if (!state.workspace || selectedProject) return;
    const projectId =
      state.workspace.focused_project_id ?? state.workspace.projects[0]?.id;
    if (projectId) dispatch({ type: "select_project", projectId });
  }, [dispatch, selectedProject, state.workspace]);

  useEffect(() => {
    if (!selectedProject) {
      dispatch({ type: "select_terminal", terminalId: null });
      return;
    }
    const terminalIds = collectTerminalIds(selectedProject.layout);
    if (terminalIds.length === 0) {
      dispatch({ type: "select_terminal", terminalId: null });
      return;
    }
    if (!selectedTerminalId || !terminalIds.includes(selectedTerminalId)) {
      dispatch({ type: "select_terminal", terminalId: terminalIds[0] });
    }
  }, [dispatch, selectedProject, selectedTerminalId]);

  useEffect(() => {
    if (!selectedProject) return;
    postAction({ action: "git_status", project_id: selectedProject.id }).catch(
      () => undefined,
    );
  }, [selectedProject?.id]);

  useEffect(() => {
    if (!notice) return;
    const timeout = window.setTimeout(() => setNotice(null), 3000);
    return () => window.clearTimeout(timeout);
  }, [notice]);

  const openTerminal = useCallback(
    (project: ApiProject, terminalId: string) => {
      dispatch({ type: "select_project", projectId: project.id });
      dispatch({ type: "select_terminal", terminalId });
      markTerminalUsed(terminalId);
      setContextOpen(false);
      setKeysOpen(false);
      setPage("current");
      postAction({
        action: "record_project_activity",
        project_id: project.id,
      }).catch(() => undefined);
    },
    [dispatch, markTerminalUsed],
  );

  const createTerminal = useCallback(
    async (project: ApiProject) => {
      try {
        const result = await postAction({
          action: "create_terminal",
          project_id: project.id,
        });
        if (
          typeof result === "object" &&
          result !== null &&
          "terminal_id" in result &&
          typeof result.terminal_id === "string"
        ) {
          dispatch({ type: "select_project", projectId: project.id });
          dispatch({ type: "select_terminal", terminalId: result.terminal_id });
          setPage("current");
        }
      } catch (error) {
        setNotice(actionError(error, "Could not create terminal"));
      }
    },
    [dispatch],
  );

  const openProject = useCallback(
    (project: ApiProject, backPage: PrimaryPage) => {
      dispatch({ type: "select_project", projectId: project.id });
      setDetailBackPage(backPage);
      setContextOpen(false);
      setPage("project");
      postAction({
        action: "record_project_activity",
        project_id: project.id,
      }).catch(() => undefined);
    },
    [dispatch],
  );

  const openTool = useCallback(
    (nextPage: "files" | "git" | "services", backPage: "project" | "more") => {
      setToolBackPage(backPage);
      setPage(nextPage);
    },
    [],
  );

  const navigatePrimary = useCallback((nextPage: PrimaryPage) => {
    setContextOpen(false);
    setKeysOpen(false);
    setPage(nextPage);
  }, []);

  let content: React.ReactNode;
  switch (page) {
    case "projects":
      content = (
        <ProjectsPage
          query={projectQuery}
          onQueryChange={setProjectQuery}
          selectedProject={selectedProject}
          onOpenProject={(project) => openProject(project, "projects")}
          onOpenTerminal={openTerminal}
        />
      );
      break;
    case "project":
      content = selectedProject ? (
        <ProjectPage
          project={selectedProject}
          selectedTerminalId={selectedTerminalId}
          onBack={() => setPage(detailBackPage)}
          onOpenTerminal={openTerminal}
          onCreateTerminal={createTerminal}
          onOpenTool={(nextPage) => openTool(nextPage, "project")}
          onShowAllTerminals={() => setPage("terminals")}
        />
      ) : (
        <MobileEmpty
          title="No project selected"
          detail="Choose a project first."
        />
      );
      break;
    case "terminals":
      content = (
        <TerminalsPage
          entries={allTerminals}
          query={terminalQuery}
          selectedTerminalId={selectedTerminalId}
          selectedProject={selectedProject}
          onQueryChange={setTerminalQuery}
          onOpenTerminal={openTerminal}
          onCreateTerminal={createTerminal}
        />
      );
      break;
    case "more":
      content = (
        <MorePage
          project={selectedProject}
          wsStatus={state.wsStatus}
          onOpenProject={() => {
            if (selectedProject) openProject(selectedProject, "more");
          }}
          onOpenTool={(nextPage) => openTool(nextPage, "more")}
        />
      );
      break;
    case "files":
      content = (
        <ToolPage
          title="Files"
          project={selectedProject}
          onBack={() => setPage(toolBackPage)}
        >
          {selectedProject && (
            <FilesPanel project={selectedProject} fullWidth />
          )}
        </ToolPage>
      );
      break;
    case "git":
      content = (
        <ToolPage
          title="Git"
          project={selectedProject}
          onBack={() => setPage(toolBackPage)}
        >
          {selectedProject && (
            <div className="mobile-git-page">
              <GitPanel project={selectedProject} />
              <GitFacts project={selectedProject} />
            </div>
          )}
        </ToolPage>
      );
      break;
    case "services":
      content = (
        <ServicesPage
          project={selectedProject}
          onBack={() => setPage(toolBackPage)}
          onOpenTerminal={openTerminal}
          onError={setNotice}
        />
      );
      break;
    default:
      content = (
        <CurrentPage
          project={selectedProject}
          terminalId={selectedTerminalId}
          terminalName={selectedTerminalName}
          keysOpen={keysOpen}
          onOpenContext={() => setContextOpen(true)}
          onToggleKeys={() => setKeysOpen((open) => !open)}
          onCreateTerminal={createTerminal}
          onSendKey={(key) => {
            if (!selectedTerminalId) return;
            postAction({
              action: "send_special_key",
              terminal_id: selectedTerminalId,
              key,
            }).catch((error) =>
              setNotice(actionError(error, "Could not send key")),
            );
          }}
        />
      );
  }

  return (
    <div className="mobile-workspace">
      <div className="mobile-page-slot">{content}</div>
      <MobileDock
        activePage={activePrimaryPage(page, toolBackPage)}
        onNavigate={navigatePrimary}
      />
      {contextOpen && selectedProject && (
        <ContextSheet
          project={selectedProject}
          selectedTerminalId={selectedTerminalId}
          terminals={allTerminals.filter(
            (entry) => entry.project.id === selectedProject.id,
          )}
          onClose={() => setContextOpen(false)}
          onOpenTerminal={openTerminal}
          onOpenProject={() => openProject(selectedProject, "current")}
          onAllProjects={() => navigatePrimary("projects")}
          onCreateTerminal={() => createTerminal(selectedProject)}
        />
      )}
      {notice && <div className="mobile-toast">{notice}</div>}
    </div>
  );
}

function useRecentTerminals(
  projects: ApiProject[],
  selectedTerminalId: string | null,
): { entries: TerminalEntry[]; markUsed: (terminalId: string) => void } {
  const [recentIds, setRecentIds] = useState(readRecentTerminalIds);
  const markUsed = useCallback((terminalId: string) => {
    setRecentIds((current) => {
      const next = [
        terminalId,
        ...current.filter((id) => id !== terminalId),
      ].slice(0, 200);
      writeRecentTerminalIds(next);
      return next;
    });
  }, []);

  const entries = useMemo(() => {
    const source = projects.flatMap((project) =>
      collectTerminalIds(project.layout).map((terminalId) => ({
        project,
        terminalId,
        name: project.terminal_names[terminalId] ?? "Terminal",
      })),
    );
    const rank = new Map(
      recentIds.map((terminalId, index) => [terminalId, index]),
    );
    return source.sort((a, b) => {
      if (a.terminalId === selectedTerminalId) return -1;
      if (b.terminalId === selectedTerminalId) return 1;
      const aRank = rank.get(a.terminalId);
      const bRank = rank.get(b.terminalId);
      if (aRank !== undefined && bRank !== undefined) return aRank - bRank;
      if (aRank !== undefined) return -1;
      if (bRank !== undefined) return 1;
      const activityDifference =
        (b.project.last_activity_at ?? 0) - (a.project.last_activity_at ?? 0);
      if (activityDifference !== 0) return activityDifference;
      return 0;
    });
  }, [projects, recentIds, selectedTerminalId]);

  return { entries, markUsed };
}

function readRecentTerminalIds(): string[] {
  try {
    const value = window.localStorage.getItem(MOBILE_RECENT_TERMINALS_KEY);
    return value ? value.split("\n").filter(Boolean) : [];
  } catch {
    return [];
  }
}

function writeRecentTerminalIds(terminalIds: string[]): void {
  try {
    window.localStorage.setItem(
      MOBILE_RECENT_TERMINALS_KEY,
      terminalIds.join("\n"),
    );
  } catch {
    // Recency is a convenience only; private browsing may reject persistence.
  }
}

function CurrentPage({
  project,
  terminalId,
  terminalName,
  keysOpen,
  onOpenContext,
  onToggleKeys,
  onCreateTerminal,
  onSendKey,
}: {
  project: ApiProject | undefined;
  terminalId: string | null;
  terminalName: string | null;
  keysOpen: boolean;
  onOpenContext: () => void;
  onToggleKeys: () => void;
  onCreateTerminal: (project: ApiProject) => void;
  onSendKey: (key: SpecialKey) => void;
}) {
  const [externalInputEpoch, setExternalInputEpoch] = useState(0);
  const sendKey = (key: SpecialKey) => {
    setExternalInputEpoch((epoch) => epoch + 1);
    onSendKey(key);
  };

  return (
    <section className="mobile-page mobile-current">
      <header className="mobile-terminal-toolbar">
        <button
          className="mobile-context-trigger"
          onClick={onOpenContext}
          disabled={!project}
        >
          <span className="mobile-context-project">
            {project?.name ?? "No project"}
          </span>
          <span className="mobile-context-terminal">
            / {terminalName ?? "No terminal"}
          </span>
          <MobileIcon name="chevronDown" />
        </button>
        <button
          className="mobile-icon-button"
          onClick={onToggleKeys}
          disabled={!terminalId}
          aria-label="Terminal keys"
        >
          <MobileIcon name="keyboard" />
        </button>
      </header>
      <main className="mobile-terminal-content">
        {project && terminalId ? (
          <TerminalPane
            terminalId={terminalId}
            name={terminalName ?? undefined}
            projectId={project.id}
            path={[]}
            hideSplitActions
            mobile
            externalInputEpoch={externalInputEpoch}
          />
        ) : project ? (
          <MobileEmpty
            title="No terminals open"
            detail="Create one in this project."
            actionLabel="New terminal"
            onAction={() => onCreateTerminal(project)}
          />
        ) : (
          <MobileEmpty
            title="No projects"
            detail="Add a project from the desktop app."
          />
        )}
      </main>
      {keysOpen && (
        <div className="mobile-key-rail">
          {SPECIAL_KEYS.map(({ label, key }) => (
            <button key={label} onClick={() => sendKey(key)}>
              {label}
            </button>
          ))}
        </div>
      )}
    </section>
  );
}

function ProjectsPage({
  query,
  selectedProject,
  onQueryChange,
  onOpenProject,
  onOpenTerminal,
}: {
  query: string;
  selectedProject: ApiProject | undefined;
  onQueryChange: (query: string) => void;
  onOpenProject: (project: ApiProject) => void;
  onOpenTerminal: (project: ApiProject, terminalId: string) => void;
}) {
  const { state } = useApp();
  const items = useMemo(
    () => buildSidebarItems(state.workspace),
    [state.workspace],
  );
  const normalizedQuery = query.trim().toLowerCase();
  const matches = (project: ApiProject) =>
    `${project.name} ${project.path} ${project.git_status?.branch ?? ""}`
      .toLowerCase()
      .includes(normalizedQuery);

  const groups = items
    .map((item) => {
      if (item.type === "folder") {
        const projects = item.projects
          .flatMap((node) => [node.project, ...node.worktrees])
          .filter(matches);
        return projects.length > 0
          ? { key: item.folder.id, label: item.folder.name, projects }
          : null;
      }
      const projects = [item.project, ...item.worktrees].filter(matches);
      return projects.length > 0
        ? { key: item.project.id, label: null, projects }
        : null;
    })
    .filter(
      (
        group,
      ): group is {
        key: string;
        label: string | null;
        projects: ApiProject[];
      } => Boolean(group),
    );

  const continueTerminal = selectedProject
    ? collectTerminalIds(selectedProject.layout)[0]
    : undefined;

  return (
    <section className="mobile-page">
      <MobileHeader title="Projects" />
      <MobileSearch
        value={query}
        onChange={onQueryChange}
        placeholder="Search projects or branches"
      />
      <div className="mobile-list-scroll">
        {!normalizedQuery && selectedProject && continueTerminal && (
          <section>
            <MobileGroupHeader label="Continue" />
            <button
              className="mobile-project-row mobile-continue-row"
              onClick={() => onOpenTerminal(selectedProject, continueTerminal)}
            >
              <span className="mobile-terminal-mark">›_</span>
              <ProjectRowText project={selectedProject} />
              <MobileIcon name="chevron" />
            </button>
          </section>
        )}
        {groups.length > 0 ? (
          groups.map((group) => (
            <section key={group.key}>
              {group.label && (
                <MobileGroupHeader
                  label={group.label}
                  count={group.projects.length}
                />
              )}
              {group.projects.map((project) => (
                <button
                  className="mobile-project-row"
                  key={project.id}
                  onClick={() => onOpenProject(project)}
                >
                  <ProjectRowText project={project} />
                  <MobileIcon name="chevron" />
                </button>
              ))}
            </section>
          ))
        ) : (
          <MobileEmpty
            title="No projects found"
            detail="Try a project or branch name."
          />
        )}
      </div>
    </section>
  );
}

function ProjectPage({
  project,
  selectedTerminalId,
  onBack,
  onOpenTerminal,
  onCreateTerminal,
  onOpenTool,
  onShowAllTerminals,
}: {
  project: ApiProject;
  selectedTerminalId: string | null;
  onBack: () => void;
  onOpenTerminal: (project: ApiProject, terminalId: string) => void;
  onCreateTerminal: (project: ApiProject) => void;
  onOpenTool: (page: "files" | "git" | "services") => void;
  onShowAllTerminals: () => void;
}) {
  const terminalIds = collectTerminalIds(project.layout);
  const git = project.git_status;

  return (
    <section className="mobile-page">
      <MobileHeader
        title={project.name}
        meta={project.git_status?.branch ?? undefined}
        onBack={onBack}
      />
      <div className="mobile-detail-scroll">
        <section className="mobile-project-identity">
          <h2 className={projectToneClass(project)}>{project.name}</h2>
          <code>{project.path}</code>
          <div className="mobile-identity-meta">
            {git?.branch && <span>{git.branch}</span>}
            <span>{terminalIds.length} terminals</span>
          </div>
          <div className="mobile-detail-actions">
            {terminalIds[0] ? (
              <button
                className="mobile-primary-button"
                onClick={() => onOpenTerminal(project, terminalIds[0])}
              >
                Open terminal
              </button>
            ) : (
              <button
                className="mobile-primary-button"
                onClick={() => onCreateTerminal(project)}
              >
                New terminal
              </button>
            )}
            {terminalIds.length > 0 && (
              <button
                className="mobile-quiet-button"
                onClick={() => onCreateTerminal(project)}
              >
                New terminal
              </button>
            )}
          </div>
        </section>

        <MobileFlatGroup
          label="Terminals"
          actionLabel="Show all"
          onAction={onShowAllTerminals}
        >
          {terminalIds.map((terminalId) => (
            <MobileNavigationRow
              key={terminalId}
              label={project.terminal_names[terminalId] ?? "Terminal"}
              value={terminalId === selectedTerminalId ? "Current" : "Open"}
              onClick={() => onOpenTerminal(project, terminalId)}
            />
          ))}
          {terminalIds.length === 0 && (
            <p className="mobile-group-empty">No terminals open</p>
          )}
        </MobileFlatGroup>

        <MobileFlatGroup label="Repository">
          <MobileFactRow
            label="Branch"
            value={git?.branch ?? "Unavailable"}
            mono
          />
          <MobileFactRow
            label="Working tree"
            value={
              git
                ? `${git.lines_added} added · ${git.lines_removed} removed`
                : "Unavailable"
            }
          />
          <MobileNavigationRow
            label="Git"
            value="Status and changes"
            onClick={() => onOpenTool("git")}
          />
        </MobileFlatGroup>

        <MobileFlatGroup label="Workspace">
          <MobileNavigationRow
            label="Files"
            value={project.path}
            onClick={() => onOpenTool("files")}
          />
          <MobileNavigationRow
            label="Services"
            value={`${project.services?.length ?? 0} configured`}
            onClick={() => onOpenTool("services")}
          />
        </MobileFlatGroup>
      </div>
    </section>
  );
}

function TerminalsPage({
  entries,
  query,
  selectedTerminalId,
  selectedProject,
  onQueryChange,
  onOpenTerminal,
  onCreateTerminal,
}: {
  entries: TerminalEntry[];
  query: string;
  selectedTerminalId: string | null;
  selectedProject: ApiProject | undefined;
  onQueryChange: (query: string) => void;
  onOpenTerminal: (project: ApiProject, terminalId: string) => void;
  onCreateTerminal: (project: ApiProject) => void;
}) {
  const normalizedQuery = query.trim().toLowerCase();
  const filtered = entries.filter((entry) =>
    `${entry.name} ${entry.project.name} ${entry.project.git_status?.branch ?? ""}`
      .toLowerCase()
      .includes(normalizedQuery),
  );
  return (
    <section className="mobile-page">
      <MobileHeader
        title="Terminals"
        meta={`${entries.length} open`}
        action={
          selectedProject ? (
            <button
              className="mobile-icon-button"
              onClick={() => onCreateTerminal(selectedProject)}
              aria-label={`New terminal in ${selectedProject.name}`}
            >
              <MobileIcon name="plus" />
            </button>
          ) : undefined
        }
      />
      <MobileSearch
        value={query}
        onChange={onQueryChange}
        placeholder="Search terminals, projects, branches"
      />
      <div className="mobile-list-scroll">
        {filtered.length > 0 ? (
          filtered.map((entry) => (
            <button
              className="mobile-terminal-row"
              key={entry.terminalId}
              onClick={() => onOpenTerminal(entry.project, entry.terminalId)}
            >
              <span className="mobile-row-copy">
                <span className="mobile-row-title">{entry.name}</span>
                <span className="mobile-row-meta">
                  <span className={projectToneClass(entry.project)}>
                    {entry.project.name}
                  </span>
                  {entry.project.git_status?.branch && (
                    <span>{entry.project.git_status.branch}</span>
                  )}
                </span>
              </span>
              <span className="mobile-row-side">
                {entry.terminalId === selectedTerminalId ? "Current" : ""}
              </span>
              <MobileIcon name="chevron" />
            </button>
          ))
        ) : (
          <MobileEmpty
            title="No terminals found"
            detail="Try a terminal, project, or branch name."
          />
        )}
      </div>
    </section>
  );
}

function MorePage({
  project,
  wsStatus,
  onOpenProject,
  onOpenTool,
}: {
  project: ApiProject | undefined;
  wsStatus: string;
  onOpenProject: () => void;
  onOpenTool: (page: "files" | "git" | "services") => void;
}) {
  return (
    <section className="mobile-page">
      <MobileHeader title="More" />
      <div className="mobile-detail-scroll">
        <section className="mobile-profile-row">
          <span className="mobile-profile-mark">O</span>
          <span>
            <strong>Okena remote</strong>
            <small>{window.location.host}</small>
          </span>
        </section>
        <MobileFlatGroup label="Workspace">
          <MobileNavigationRow
            label="Files"
            value={project?.name ?? "Select a project"}
            disabled={!project}
            icon="files"
            onClick={() => onOpenTool("files")}
          />
          <MobileNavigationRow
            label="Git"
            value={project?.git_status?.branch ?? "Select a project"}
            disabled={!project}
            icon="git"
            onClick={() => onOpenTool("git")}
          />
          <MobileNavigationRow
            label="Services"
            value={
              project
                ? `${project.services?.length ?? 0} configured`
                : "Select a project"
            }
            disabled={!project}
            icon="services"
            onClick={() => onOpenTool("services")}
          />
        </MobileFlatGroup>
        <MobileFlatGroup label="Connection">
          <MobileFactRow
            label="Status"
            value={capitalize(wsStatus)}
            tone={wsStatus === "connected" ? "success" : "warning"}
          />
          <MobileFactRow label="Transport" value="WebSocket" />
          <MobileFactRow label="Host" value={window.location.host} mono />
        </MobileFlatGroup>
        <MobileFlatGroup label="Current context">
          <MobileNavigationRow
            label="Project"
            value={project?.name ?? "None"}
            disabled={!project}
            onClick={onOpenProject}
          />
          <MobileFactRow label="Path" value={project?.path ?? "—"} mono />
        </MobileFlatGroup>
      </div>
    </section>
  );
}

function ToolPage({
  title,
  project,
  onBack,
  children,
}: {
  title: string;
  project: ApiProject | undefined;
  onBack: () => void;
  children: React.ReactNode;
}) {
  return (
    <section className="mobile-page">
      <MobileHeader title={title} meta={project?.name} onBack={onBack} />
      <div className="mobile-tool-content">
        {project ? (
          children
        ) : (
          <MobileEmpty
            title="No project selected"
            detail="Choose a project first."
          />
        )}
      </div>
    </section>
  );
}

function ServicesPage({
  project,
  onBack,
  onOpenTerminal,
  onError,
}: {
  project: ApiProject | undefined;
  onBack: () => void;
  onOpenTerminal: (project: ApiProject, terminalId: string) => void;
  onError: (message: string) => void;
}) {
  const runAction = async (
    service: ApiServiceInfo,
    action: "start_service" | "stop_service" | "restart_service",
  ) => {
    if (!project) return;
    try {
      await postAction({
        action,
        project_id: project.id,
        service_name: service.name,
      });
    } catch (error) {
      onError(actionError(error, `Could not update ${service.name}`));
    }
  };

  return (
    <section className="mobile-page">
      <MobileHeader title="Services" meta={project?.name} onBack={onBack} />
      <div className="mobile-list-scroll">
        {project?.services && project.services.length > 0 ? (
          project.services.map((service) => (
            <div className="mobile-service-row" key={service.name}>
              <span className="mobile-row-copy">
                <span className="mobile-row-title">{service.name}</span>
                <span className="mobile-row-meta">
                  <span
                    className={
                      service.status === "running" ? "mobile-success" : ""
                    }
                  >
                    {service.status}
                  </span>
                  {service.status === "running" &&
                    service.ports &&
                    service.ports.length > 0 && (
                      <span className="mobile-service-ports">
                        {service.ports.map((port) => (
                          <a
                            href={servicePortUrl(port)}
                            key={port}
                            target="_blank"
                            rel="noreferrer"
                          >
                            Open :{port}
                          </a>
                        ))}
                      </span>
                    )}
                </span>
              </span>
              {service.status === "running" &&
                service.ports?.[0] !== undefined && (
                  <a
                    className="mobile-service-action"
                    href={servicePortUrl(service.ports[0])}
                    target="_blank"
                    rel="noreferrer"
                  >
                    Open
                  </a>
                )}
              {service.terminal_id && (
                <button
                  className="mobile-service-action"
                  onClick={() => {
                    if (service.terminal_id) {
                      onOpenTerminal(project, service.terminal_id);
                    }
                  }}
                >
                  Terminal
                </button>
              )}
              <button
                className="mobile-service-action"
                onClick={() =>
                  runAction(
                    service,
                    service.status === "running"
                      ? "restart_service"
                      : "start_service",
                  )
                }
              >
                {service.status === "running" ? "Restart" : "Start"}
              </button>
            </div>
          ))
        ) : (
          <MobileEmpty
            title="No services configured"
            detail="Configure services on the desktop first."
          />
        )}
      </div>
    </section>
  );
}

function ContextSheet({
  project,
  terminals,
  selectedTerminalId,
  onClose,
  onOpenTerminal,
  onOpenProject,
  onAllProjects,
  onCreateTerminal,
}: {
  project: ApiProject;
  terminals: TerminalEntry[];
  selectedTerminalId: string | null;
  onClose: () => void;
  onOpenTerminal: (project: ApiProject, terminalId: string) => void;
  onOpenProject: () => void;
  onAllProjects: () => void;
  onCreateTerminal: () => void;
}) {
  return (
    <>
      <button
        className="mobile-sheet-scrim"
        onClick={onClose}
        aria-label="Close terminal switcher"
      />
      <section className="mobile-context-sheet" aria-label="Switch terminal">
        <header className="mobile-sheet-header">
          <span className="mobile-row-copy">
            <strong>{project.name}</strong>
            <small>{project.git_status?.branch ?? project.path}</small>
          </span>
          <button
            className="mobile-icon-button"
            onClick={onClose}
            aria-label="Close"
          >
            <MobileIcon name="close" />
          </button>
        </header>
        <div className="mobile-sheet-body">
          <MobileGroupHeader label="Terminals" />
          {terminals.map((entry) => (
            <button
              className="mobile-sheet-terminal"
              key={entry.terminalId}
              onClick={() => onOpenTerminal(project, entry.terminalId)}
            >
              <span>{entry.name}</span>
              <small>
                {entry.terminalId === selectedTerminalId ? "Current" : "Open"}
              </small>
            </button>
          ))}
          {terminals.length === 0 && (
            <p className="mobile-group-empty">No terminals open</p>
          )}
        </div>
        <footer className="mobile-sheet-footer">
          <button className="mobile-ghost-button" onClick={onOpenProject}>
            Project details
          </button>
          <button className="mobile-ghost-button" onClick={onCreateTerminal}>
            New terminal
          </button>
          <button className="mobile-quiet-button" onClick={onAllProjects}>
            All projects
          </button>
        </footer>
      </section>
    </>
  );
}

function MobileDock({
  activePage,
  onNavigate,
}: {
  activePage: PrimaryPage;
  onNavigate: (page: PrimaryPage) => void;
}) {
  const entries: Array<{ page: PrimaryPage; label: string; icon: IconName }> = [
    { page: "current", label: "Current", icon: "terminal" },
    { page: "projects", label: "Projects", icon: "projects" },
    { page: "terminals", label: "Terminals", icon: "terminal" },
    { page: "more", label: "More", icon: "more" },
  ];
  return (
    <nav className="mobile-dock" aria-label="Main navigation">
      {entries.map((entry) => (
        <button
          className={entry.page === activePage ? "active" : undefined}
          key={entry.page}
          onClick={() => onNavigate(entry.page)}
          aria-current={entry.page === activePage ? "page" : undefined}
        >
          <MobileIcon name={entry.icon} />
          <span>{entry.label}</span>
        </button>
      ))}
    </nav>
  );
}

function MobileHeader({
  title,
  meta,
  onBack,
  action,
}: {
  title: string;
  meta?: string;
  onBack?: () => void;
  action?: React.ReactNode;
}) {
  return (
    <header className="mobile-page-header">
      {onBack && (
        <button
          className="mobile-icon-button"
          onClick={onBack}
          aria-label="Back"
        >
          <MobileIcon name="back" />
        </button>
      )}
      <h1>{title}</h1>
      {meta && <span className="mobile-header-meta">{meta}</span>}
      {action}
    </header>
  );
}

function MobileSearch({
  value,
  onChange,
  placeholder,
}: {
  value: string;
  onChange: (value: string) => void;
  placeholder: string;
}) {
  return (
    <div className="mobile-search-bar">
      <label>
        <MobileIcon name="search" />
        <input
          value={value}
          onChange={(event) => onChange(event.currentTarget.value)}
          placeholder={placeholder}
          autoCapitalize="off"
          autoComplete="off"
          spellCheck={false}
        />
      </label>
    </div>
  );
}

function MobileGroupHeader({
  label,
  count,
}: {
  label: string;
  count?: number;
}) {
  return (
    <div className="mobile-group-header">
      <span>{label}</span>
      {count !== undefined && <span>{count}</span>}
    </div>
  );
}

function ProjectRowText({ project }: { project: ApiProject }) {
  const count = collectTerminalIds(project.layout).length;
  return (
    <span className="mobile-row-copy">
      <span className={`mobile-row-title ${projectToneClass(project)}`}>
        {project.name}
      </span>
      <span className="mobile-row-meta">
        <span>{project.git_status?.branch ?? compactPath(project.path)}</span>
        <span>{count} terminals</span>
      </span>
    </span>
  );
}

function MobileFlatGroup({
  label,
  actionLabel,
  onAction,
  children,
}: {
  label: string;
  actionLabel?: string;
  onAction?: () => void;
  children: React.ReactNode;
}) {
  return (
    <section className="mobile-flat-group">
      <div className="mobile-flat-group-header">
        <h3>{label}</h3>
        {actionLabel && onAction && (
          <button onClick={onAction}>{actionLabel}</button>
        )}
      </div>
      {children}
    </section>
  );
}

function MobileNavigationRow({
  label,
  value,
  onClick,
  disabled,
  icon,
}: {
  label: string;
  value: string;
  onClick: () => void;
  disabled?: boolean;
  icon?: IconName;
}) {
  return (
    <button className="mobile-nav-row" onClick={onClick} disabled={disabled}>
      {icon && <MobileIcon name={icon} />}
      <span>{label}</span>
      <small>{value}</small>
      <MobileIcon name="chevron" />
    </button>
  );
}

function MobileFactRow({
  label,
  value,
  mono,
  tone,
}: {
  label: string;
  value: string;
  mono?: boolean;
  tone?: "success" | "warning";
}) {
  const className = [mono ? "mono" : "", tone ? `mobile-${tone}` : ""]
    .filter(Boolean)
    .join(" ");
  return (
    <div className="mobile-fact-row">
      <span>{label}</span>
      <strong className={className}>{value}</strong>
    </div>
  );
}

function MobileEmpty({
  title,
  detail,
  actionLabel,
  onAction,
}: {
  title: string;
  detail: string;
  actionLabel?: string;
  onAction?: () => void;
}) {
  return (
    <div className="mobile-empty">
      <div>
        <strong>{title}</strong>
        <span>{detail}</span>
        {actionLabel && onAction && (
          <button className="mobile-quiet-button" onClick={onAction}>
            {actionLabel}
          </button>
        )}
      </div>
    </div>
  );
}

function GitFacts({ project }: { project: ApiProject }) {
  const git = project.git_status;
  return (
    <div className="mobile-detail-scroll mobile-git-facts">
      <MobileFlatGroup label="Repository">
        <MobileFactRow
          label="Branch"
          value={git?.branch ?? "Unavailable"}
          mono
        />
        <MobileFactRow
          label="Added"
          value={String(git?.lines_added ?? 0)}
          tone="success"
        />
        <MobileFactRow
          label="Removed"
          value={String(git?.lines_removed ?? 0)}
          tone="warning"
        />
        <MobileFactRow label="Ahead" value={String(git?.ahead ?? 0)} />
        <MobileFactRow label="Behind" value={String(git?.behind ?? 0)} />
      </MobileFlatGroup>
    </div>
  );
}

function MobileIcon({ name }: { name: IconName }) {
  switch (name) {
    case "projects":
      return (
        <svg viewBox="0 0 24 24">
          <path d="M3 7.5h7l2-2h9v13H3z" />
        </svg>
      );
    case "terminal":
      return (
        <svg viewBox="0 0 24 24">
          <rect x="3" y="4" width="18" height="16" rx="2" />
          <path d="m7 9 3 3-3 3m5 0h5" />
        </svg>
      );
    case "more":
      return (
        <svg viewBox="0 0 24 24">
          <circle cx="5" cy="12" r="1" />
          <circle cx="12" cy="12" r="1" />
          <circle cx="19" cy="12" r="1" />
        </svg>
      );
    case "search":
      return (
        <svg viewBox="0 0 24 24">
          <circle cx="10.5" cy="10.5" r="6.5" />
          <path d="m16 16 5 5" />
        </svg>
      );
    case "back":
      return (
        <svg viewBox="0 0 24 24">
          <path d="m15 18-6-6 6-6" />
        </svg>
      );
    case "chevron":
      return (
        <svg viewBox="0 0 24 24">
          <path d="m9 18 6-6-6-6" />
        </svg>
      );
    case "chevronDown":
      return (
        <svg viewBox="0 0 24 24">
          <path d="m6 9 6 6 6-6" />
        </svg>
      );
    case "plus":
      return (
        <svg viewBox="0 0 24 24">
          <path d="M12 5v14M5 12h14" />
        </svg>
      );
    case "keyboard":
      return (
        <svg viewBox="0 0 24 24">
          <rect x="2.5" y="5" width="19" height="14" rx="2" />
          <path d="M6 9h.01M10 9h.01M14 9h.01M18 9h.01M6 13h.01M10 13h.01M14 13h4M7 16h10" />
        </svg>
      );
    case "close":
      return (
        <svg viewBox="0 0 24 24">
          <path d="m6 6 12 12M18 6 6 18" />
        </svg>
      );
    case "files":
      return (
        <svg viewBox="0 0 24 24">
          <path d="M5 3h9l5 5v13H5z" />
          <path d="M14 3v5h5" />
        </svg>
      );
    case "git":
      return (
        <svg viewBox="0 0 24 24">
          <circle cx="6" cy="5" r="2" />
          <circle cx="18" cy="7" r="2" />
          <circle cx="6" cy="19" r="2" />
          <path d="M6 7v10M8 9c5 0 3-2 8-2" />
        </svg>
      );
    case "services":
      return (
        <svg viewBox="0 0 24 24">
          <rect x="4" y="4" width="16" height="6" rx="2" />
          <rect x="4" y="14" width="16" height="6" rx="2" />
          <path d="M8 7h.01M8 17h.01" />
        </svg>
      );
  }
}

function activePrimaryPage(
  page: MobilePage,
  toolBackPage: "project" | "more",
): PrimaryPage {
  if (PRIMARY_PAGES.includes(page as PrimaryPage)) return page as PrimaryPage;
  if (page === "project" || toolBackPage === "project") return "projects";
  return "more";
}

function servicePortUrl(port: number): string {
  const url = new URL(window.location.href);
  url.port = String(port);
  url.pathname = "/";
  url.search = "";
  url.hash = "";
  return url.toString();
}

function projectToneClass(project: ApiProject): string {
  const color = project.folder_color;
  const supported = [
    "red",
    "orange",
    "yellow",
    "lime",
    "green",
    "teal",
    "cyan",
    "blue",
    "indigo",
    "purple",
    "pink",
  ];
  return color && supported.includes(color)
    ? `mobile-project-${color}`
    : "mobile-project-default";
}

function compactPath(path: string): string {
  const parts = path.split("/").filter(Boolean);
  return parts[parts.length - 1] ?? path;
}

function capitalize(value: string): string {
  return value.charAt(0).toUpperCase() + value.slice(1);
}

function actionError(error: unknown, fallback: string): string {
  return error instanceof Error ? error.message : fallback;
}
