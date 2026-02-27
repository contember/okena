# Issue 12: Web client support for app panes

**Priority:** medium
**Files:** `web/src/api/types.ts`, `web/src/components/KruhPane/` (new directory), `web/src/components/TerminalArea.tsx`, `web/src/state/store.ts`, `web/src/api/client.ts`

## Description

Add web client rendering for app panes. The web client should display KruhPane state, handle real-time updates via WebSocket, and dispatch actions back to the server.

## Implementation

### 1. `web/src/api/types.ts`

Add TypeScript types mirroring the Rust types:

```typescript
// Add to ApiLayoutNode union
| { type: "app"; app_id: string | null; app_kind: string; app_state?: KruhViewState }

// New types
export interface KruhViewState {
  app_id: string | null;
  screen: KruhScreen;
}

export type KruhScreen =
  | { screen: "Scanning" }
  | { screen: "PlanPicker"; plans: PlanViewInfo[]; selected_index: number }
  | { screen: "TaskBrowser"; plan_name: string; issues: IssueViewInfo[] }
  | { screen: "Editing"; file_path: string; content: string; is_new: boolean }
  | { screen: "Settings"; model: string; max_iterations: number; auto_start: boolean }
  | { screen: "LoopOverview"; loops: LoopViewInfo[]; focused_index: number };

export interface PlanViewInfo {
  name: string;
  path: string;
  issue_count: number;
  completed_count: number;
}

export interface IssueViewInfo {
  number: string;
  title: string;
  status: string;
  priority: string | null;
}

export interface LoopViewInfo {
  loop_id: number;
  plan_name: string;
  phase: string;
  state: string;
  current_issue: string | null;
  progress: { completed: number; total: number };
  output_lines: { text: string; is_error: boolean }[];
}

export type KruhAction =
  | { action: "StartScan" }
  | { action: "SelectPlan"; index: number }
  | { action: "OpenPlan"; name: string }
  | { action: "BackToPlans" }
  | { action: "StartLoop"; plan_name: string }
  | { action: "StartAllLoops" }
  | { action: "PauseLoop"; loop_id: number }
  | { action: "ResumeLoop"; loop_id: number }
  | { action: "StopLoop"; loop_id: number }
  | { action: "CloseLoops" }
  | { action: "FocusLoop"; index: number }
  | { action: "OpenEditor"; file_path: string }
  | { action: "SaveEditor"; content: string }
  | { action: "CloseEditor" }
  | { action: "OpenSettings" }
  | { action: "UpdateSettings"; model: string; max_iterations: number; auto_start: boolean }
  | { action: "CloseSettings" }
  | { action: "BrowseTasks"; plan_name: string };

// WS message types
export interface WsAppStateChanged {
  type: "AppStateChanged";
  app_id: string;
  app_kind: string;
  state: KruhViewState;
}
```

### 2. `web/src/components/KruhPane/` (new directory)

Create React components that render each KruhScreen variant:

- `KruhPane.tsx` — main component, switches on `screen.screen`
- `ScanningScreen.tsx` — loading spinner
- `PlanPickerScreen.tsx` — list of plans with select buttons
- `TaskBrowserScreen.tsx` — issue list with status indicators
- `EditingScreen.tsx` — textarea with save/cancel buttons
- `SettingsScreen.tsx` — form fields for model, max_iterations, auto_start
- `LoopOverviewScreen.tsx` — loop cards with output log, progress bar, controls

Each interactive element calls `onAction(action: KruhAction)` prop which sends via WebSocket.

### 3. `web/src/components/TerminalArea.tsx`

Add `"app"` case to the layout renderer:

```tsx
case "app":
  return <AppPane
    appId={node.app_id}
    appKind={node.app_kind}
    initialState={node.app_state}
    onAction={(action) => sendWsMessage({ type: "AppAction", app_id: node.app_id, action })}
  />;
```

### 4. `web/src/state/store.ts`

Add app state tracking:

```typescript
// In AppState
appStates: Map<string, KruhViewState>;

// Handle WsAppStateChanged
case "AppStateChanged":
  appStates.set(msg.app_id, msg.state);
  break;
```

### 5. `web/src/api/client.ts`

Add methods for app subscription:

```typescript
subscribeApps(appIds: string[]) {
  this.sendWs({ type: "SubscribeApps", app_ids: appIds });
}

unsubscribeApps(appIds: string[]) {
  this.sendWs({ type: "UnsubscribeApps", app_ids: appIds });
}

sendAppAction(appId: string, action: KruhAction) {
  this.sendWs({ type: "AppAction", app_id: appId, action });
}
```

## Acceptance Criteria

- Web client renders app panes in the layout
- All 6 KruhScreen variants have a visual representation
- State updates stream via WebSocket and UI updates in real-time
- Clicking buttons sends actions to the server
- App subscription/unsubscription works on state changes
- `npm run build` (or equivalent) succeeds in `web/`
