import { useApp } from "../state/store";

const STATUS_COLORS: Record<string, string> = {
  connected: "bg-[var(--ok-green)]",
  connecting: "bg-[var(--ok-yellow)]",
  disconnected: "bg-[var(--ok-red)]",
};

export function StatusBar() {
  const { state } = useApp();

  return (
    <div className="panel-rule flex items-center gap-2 border-t bg-[var(--ok-header)] px-3 py-1 text-[11px] text-[var(--ok-text-secondary)]">
      <span
        className={`inline-block w-2 h-2 rounded-full ${STATUS_COLORS[state.wsStatus]}`}
      />
      <span className="capitalize">{state.wsStatus}</span>
      {state.workspace && (
        <span className="ml-auto">
          {state.workspace.projects.length} project{state.workspace.projects.length !== 1 ? "s" : ""}
        </span>
      )}
    </div>
  );
}
