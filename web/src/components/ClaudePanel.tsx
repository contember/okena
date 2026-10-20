import { useEffect } from "react";
import { createPortal } from "react-dom";
import type { AgentLifecycle, ApiClaudeSession, ApiProject } from "../api/types";
import { useClaudeSession } from "../hooks/useClaudeSession";
import { useApp } from "../state/store";
import { collectTerminalIds } from "../utils/layout";
import { ClaudeComposer } from "./ClaudeComposer";
import { ClaudeDialogCard } from "./ClaudeDialogCard";
import { ClaudeTranscript } from "./ClaudeTranscript";
import { LIFECYCLE_COLOR } from "./claudeStyles";

const HOOKS_HINT =
  "Answering dialogs needs Claude remote enabled in Okena settings and claude restarted in this pane.";

/** The single Claude panel of the app; closes itself when its terminal leaves the workspace. */
export function ClaudePanelHost() {
  const { state, dispatch } = useApp();
  const terminalId = state.claudePanelTerminalId;
  const project = terminalId ? findTerminalProject(state.workspace?.projects ?? [], terminalId) : undefined;
  const terminalGone = terminalId !== null && state.workspace !== null && project === undefined;

  useEffect(() => {
    if (terminalGone) dispatch({ type: "close_claude_panel" });
  }, [terminalGone, dispatch]);

  if (!terminalId || !project) return null;
  return (
    <ClaudePanel
      key={terminalId}
      terminalId={terminalId}
      title={project.terminal_names[terminalId] ?? "Terminal"}
      lifecycle={project.terminal_agent_status?.[terminalId]?.lifecycle}
      onClose={() => dispatch({ type: "close_claude_panel" })}
    />
  );
}

function findTerminalProject(projects: readonly ApiProject[], terminalId: string): ApiProject | undefined {
  return projects.find(
    (project) =>
      collectTerminalIds(project.layout).includes(terminalId) ||
      project.terminal_agent_status?.[terminalId] !== undefined,
  );
}

/** Chat view of the Claude session in one terminal: full screen on phones, a right drawer otherwise. */
function ClaudePanel({
  terminalId,
  title,
  lifecycle,
  onClose,
}: {
  terminalId: string;
  title: string;
  lifecycle: AgentLifecycle | undefined;
  onClose: () => void;
}) {
  const { session, entries, transcriptMissing, error, dismissDialog } = useClaudeSession(terminalId);
  // Hook-reported busy state is fresher than the lifecycle OSC, so it wins when known.
  const activity: AgentLifecycle | null =
    session?.busy === undefined ? (lifecycle ?? null) : session.busy ? "working" : "idle";
  const busy = activity === "working";

  return createPortal(
    <div
      role="dialog"
      aria-label={`Claude · ${title}`}
      className="fixed inset-0 z-50 flex flex-col bg-[var(--ok-panel)] text-[var(--ok-text)] md:left-auto md:w-[480px] md:border-l md:border-[var(--ok-border)] md:shadow-2xl"
      onMouseDown={(event) => event.stopPropagation()}
      onKeyDown={(event) => {
        if (event.key === "Escape" && !event.nativeEvent.isComposing) {
          event.stopPropagation();
          onClose();
        }
      }}
    >
      <PanelHeader title={title} session={session} activity={activity} onClose={onClose} />
      {error && <div className="border-b border-[var(--ok-border)] px-3 py-1 text-[11px] text-[var(--ok-red)]">{error}</div>}
      {session && !session.hooks_attached && (
        <div className="border-b border-[var(--ok-border)] px-3 py-1.5 text-[11px] text-[var(--ok-text-secondary)]">
          {HOOKS_HINT}
        </div>
      )}
      <ClaudeTranscript
        entries={entries}
        placeholder={transcriptMissing ? "No transcript for this pane yet." : session ? null : "Loading..."}
      />
      {session && session.dialogs.length > 0 && (
        <div className="flex max-h-[60%] flex-shrink-0 flex-col gap-2 overflow-y-auto border-t border-[var(--ok-border)] p-2">
          {session.dialogs.map((dialog) => (
            <ClaudeDialogCard key={dialog.id} terminalId={terminalId} dialog={dialog} onDone={dismissDialog} />
          ))}
        </div>
      )}
      <ClaudeComposer terminalId={terminalId} busy={busy} />
    </div>,
    document.body,
  );
}

function PanelHeader({
  title,
  session,
  activity,
  onClose,
}: {
  title: string;
  session: ApiClaudeSession | null;
  activity: AgentLifecycle | null;
  onClose: () => void;
}) {
  const stateLabel = activity ?? "unknown";
  const dotColor = LIFECYCLE_COLOR[activity ?? "idle"];

  return (
    <header className="flex min-h-[44px] flex-shrink-0 items-center gap-2 border-b border-[var(--ok-border)] bg-[var(--ok-header)] px-3 py-2">
      <span className="h-2 w-2 flex-shrink-0 rounded-full" style={{ backgroundColor: dotColor }} />
      <div className="min-w-0 flex-1">
        <div className="truncate text-[12px] font-bold">Claude · {title}</div>
        <div className="flex flex-wrap items-center gap-1.5 text-[10px] text-[var(--ok-text-muted)]">
          <span>{stateLabel}</span>
          {session && (
            <>
              <Indicator label="channel" on={session.channel_attached} />
              <Indicator label="hooks" on={session.hooks_attached} />
              {session.queued_prompts > 0 && <span>{session.queued_prompts} queued</span>}
            </>
          )}
        </div>
      </div>
      <button className="icon-button h-8 w-8" title="Close" aria-label="Close Claude panel" onClick={onClose}>
        &#x2715;
      </button>
    </header>
  );
}

function Indicator({ label, on }: { label: string; on: boolean }) {
  return (
    <span
      className={`rounded-[3px] border px-1 ${
        on ? "border-[var(--ok-green)]/50 text-[var(--ok-green)]" : "border-[var(--ok-border)] text-[var(--ok-text-muted)]"
      }`}
      title={`${label} ${on ? "attached" : "not attached"}`}
    >
      {label}
    </span>
  );
}
