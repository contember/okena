import { useApp } from "../state/store";
import { LIFECYCLE_COLOR } from "./claudeStyles";

/** Header affordance for a terminal running an agent; opens the Claude chat panel. */
export function ClaudeBadge({ terminalId, projectId }: { terminalId: string; projectId: string }) {
  const { state, dispatch } = useApp();
  const project = state.workspace?.projects.find((candidate) => candidate.id === projectId);
  const lifecycle = project?.terminal_agent_status?.[terminalId]?.lifecycle;
  const dialogCount = state.workspace?.claude_dialogs?.[terminalId] ?? 0;

  if (!lifecycle && dialogCount === 0) return null;

  const description = `${lifecycle ?? "no status"}, ${dialogCount} open dialog${dialogCount === 1 ? "" : "s"}`;

  return (
    <button
      className="flex h-[22px] items-center justify-center gap-1 rounded-[var(--ok-radius)] px-1.5 text-[11px] text-[var(--ok-text-secondary)] hover:bg-[var(--ok-hover)] hover:text-[var(--ok-text)] pointer-coarse:min-h-8 pointer-coarse:min-w-8"
      title={`Claude: ${description}`}
      aria-label={`Open Claude panel (${description})`}
      onMouseDown={(event) => event.stopPropagation()}
      onClick={() => dispatch({ type: "open_claude_panel", terminalId })}
    >
      <span
        className="h-1.5 w-1.5 rounded-full"
        style={{ backgroundColor: LIFECYCLE_COLOR[lifecycle ?? "idle"] }}
      />
      Claude
      {dialogCount > 0 && (
        <span className="rounded-full bg-[var(--ok-red)] px-1.5 text-[10px] font-bold leading-4 text-white">
          {dialogCount}
        </span>
      )}
    </button>
  );
}
