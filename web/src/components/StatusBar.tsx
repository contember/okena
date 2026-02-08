import { useApp } from "../state/store";

const STATUS_COLORS: Record<string, string> = {
  connected: "bg-green-500",
  connecting: "bg-yellow-500",
  disconnected: "bg-red-500",
};

export function StatusBar() {
  const { state } = useApp();

  return (
    <div className="flex items-center gap-2 px-3 py-1 bg-zinc-900 border-t border-zinc-800 text-xs text-zinc-500">
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
