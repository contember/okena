import { useApp } from "../state/store";

const STATUS_COLORS: Record<string, string> = {
  connected: "bg-[var(--ok-green)]",
  connecting: "bg-[var(--ok-yellow)]",
  disconnected: "bg-[var(--ok-red)]",
};

export function StatusBar() {
  const { state } = useApp();

  return (
    <div className="status-bar panel-rule flex flex-shrink-0 items-center gap-2 border-t bg-[var(--ok-header)] text-[0.6875rem] text-[var(--ok-text-secondary)]">
      <span
        className={`inline-block h-2 w-2 rounded-full ${STATUS_COLORS[state.wsStatus]}`}
      />
      <span className="capitalize">{state.wsStatus}</span>
    </div>
  );
}
