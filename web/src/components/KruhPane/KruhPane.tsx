import type { KruhAction, KruhViewState } from "../../api/types";
import { useApp } from "../../state/store";

interface KruhPaneProps {
  appId: string | null;
  appKind: string;
  onAction: (action: KruhAction) => void;
}

export function KruhPane({ appId, appKind: _appKind, onAction }: KruhPaneProps) {
  const { state } = useApp();
  const viewState: KruhViewState | undefined = appId ? state.appStates[appId] : undefined;

  if (!viewState) {
    return (
      <div className="h-full flex items-center justify-center bg-zinc-950 text-zinc-500 text-sm">
        Connecting to remote app...
      </div>
    );
  }

  const screen = viewState.screen;

  switch (screen.screen) {
    case "Scanning":
      return <ScanningScreen />;
    case "PlanPicker":
      return <PlanPickerScreen plans={screen.plans} selectedIndex={screen.selected_index} onAction={onAction} />;
    case "TaskBrowser":
      return <TaskBrowserScreen planName={screen.plan_name} issues={screen.issues} onAction={onAction} />;
    case "Editing":
      return <EditingScreen filePath={screen.file_path} content={screen.content} isNew={screen.is_new} onAction={onAction} />;
    case "Settings":
      return <SettingsScreen model={screen.model} maxIterations={screen.max_iterations} autoStart={screen.auto_start} onAction={onAction} />;
    case "LoopOverview":
      return <LoopOverviewScreen loops={screen.loops} focusedIndex={screen.focused_index} onAction={onAction} />;
  }
}

// ── Screen components ────────────────────────────────────────────────────────

function ScanningScreen() {
  return (
    <div className="h-full flex items-center justify-center bg-zinc-950 text-zinc-500 text-sm">
      Scanning for plans...
    </div>
  );
}

function PlanPickerScreen({
  plans,
  selectedIndex,
  onAction,
}: {
  plans: KruhViewState extends { screen: { screen: "PlanPicker" } } ? never : any;
  selectedIndex: number;
  onAction: (action: KruhAction) => void;
}) {
  return (
    <div className="h-full overflow-y-auto bg-zinc-950 text-zinc-100">
      {plans.map((plan: any, i: number) => (
        <button
          key={plan.name}
          onClick={() => onAction({ action: "SelectPlan", index: i })}
          className={`w-full text-left px-4 py-3 border-b border-zinc-800 hover:bg-zinc-900 transition-colors ${
            i === selectedIndex ? "bg-zinc-900" : ""
          }`}
        >
          <div className="text-sm font-medium text-zinc-100">{plan.name}</div>
          <div className="text-xs text-zinc-500 mt-0.5">
            {plan.completed_count}/{plan.issue_count} done
          </div>
        </button>
      ))}
    </div>
  );
}

function TaskBrowserScreen({
  planName,
  issues,
  onAction: _onAction,
}: {
  planName: string;
  issues: any[];
  onAction: (action: KruhAction) => void;
}) {
  return (
    <div className="h-full flex flex-col bg-zinc-950 text-zinc-100">
      <div className="px-4 py-2 border-b border-zinc-800 text-sm font-medium text-zinc-300">
        {planName}
      </div>
      <div className="flex-1 overflow-y-auto">
        {issues.map((issue: any) => (
          <div key={issue.number} className="flex items-start gap-3 px-4 py-2 border-b border-zinc-900">
            <span
              className={`text-xs mt-0.5 shrink-0 ${
                issue.status === "completed"
                  ? "text-green-500"
                  : issue.status === "in_progress"
                  ? "text-yellow-400"
                  : "text-zinc-500"
              }`}
            >
              {issue.status}
            </span>
            <span className="text-sm text-zinc-200">{issue.title}</span>
          </div>
        ))}
      </div>
    </div>
  );
}

function EditingScreen({
  filePath,
  content,
  isNew: _isNew,
  onAction,
}: {
  filePath: string;
  content: string;
  isNew: boolean;
  onAction: (action: KruhAction) => void;
}) {
  return (
    <div className="h-full flex flex-col bg-zinc-950">
      <div className="flex items-center gap-2 px-4 py-2 border-b border-zinc-800 shrink-0">
        <span className="text-xs text-zinc-500 font-mono">{filePath}</span>
        <div className="flex-1" />
        <button
          onClick={() => onAction({ action: "CloseEditor" })}
          className="text-xs text-zinc-500 hover:text-zinc-300 px-2 py-1 rounded hover:bg-zinc-800"
        >
          Close
        </button>
      </div>
      <div className="flex-1 overflow-y-auto p-4">
        <pre className="text-xs font-mono text-zinc-200 whitespace-pre-wrap break-all">{content}</pre>
      </div>
    </div>
  );
}

function SettingsScreen({
  model,
  maxIterations,
  autoStart,
  onAction,
}: {
  model: string;
  maxIterations: number;
  autoStart: boolean;
  onAction: (action: KruhAction) => void;
}) {
  return (
    <div className="h-full overflow-y-auto bg-zinc-950 text-zinc-100 p-4 space-y-4">
      <div className="space-y-1">
        <label className="text-xs text-zinc-500">Model</label>
        <div className="text-sm text-zinc-200 bg-zinc-900 px-3 py-2 rounded">{model}</div>
      </div>
      <div className="space-y-1">
        <label className="text-xs text-zinc-500">Max iterations</label>
        <div className="text-sm text-zinc-200 bg-zinc-900 px-3 py-2 rounded">{maxIterations}</div>
      </div>
      <div className="space-y-1">
        <label className="text-xs text-zinc-500">Auto start</label>
        <div className="text-sm text-zinc-200 bg-zinc-900 px-3 py-2 rounded">
          {autoStart ? "Enabled" : "Disabled"}
        </div>
      </div>
      <button
        onClick={() => onAction({ action: "CloseSettings" })}
        className="text-xs text-zinc-500 hover:text-zinc-300 px-3 py-1.5 rounded border border-zinc-700 hover:border-zinc-500"
      >
        Close
      </button>
    </div>
  );
}

function LoopOverviewScreen({
  loops,
  focusedIndex,
  onAction,
}: {
  loops: any[];
  focusedIndex: number;
  onAction: (action: KruhAction) => void;
}) {
  return (
    <div className="h-full overflow-y-auto bg-zinc-950 text-zinc-100">
      {loops.map((loop: any, i: number) => (
        <button
          key={loop.loop_id}
          onClick={() => onAction({ action: "FocusLoop", index: i })}
          className={`w-full text-left px-4 py-3 border-b border-zinc-800 hover:bg-zinc-900 transition-colors ${
            i === focusedIndex ? "bg-zinc-900" : ""
          }`}
        >
          <div className="flex items-center justify-between">
            <span className="text-sm font-medium text-zinc-100">{loop.plan_name}</span>
            <span className="text-xs text-zinc-500">
              {loop.progress.completed}/{loop.progress.total}
            </span>
          </div>
          <div className="text-xs text-zinc-500 mt-0.5">
            {loop.state} — {loop.phase}
          </div>
          {loop.current_issue && (
            <div className="text-xs text-zinc-400 mt-0.5 truncate">{loop.current_issue}</div>
          )}
          {loop.output_lines.length > 0 && (
            <div className="mt-1.5 bg-zinc-900 rounded p-1.5 max-h-16 overflow-hidden">
              {loop.output_lines.slice(-3).map((line: any, j: number) => (
                <div
                  key={j}
                  className={`text-xs font-mono truncate ${line.is_error ? "text-red-400" : "text-zinc-400"}`}
                >
                  {line.text}
                </div>
              ))}
            </div>
          )}
        </button>
      ))}
    </div>
  );
}
