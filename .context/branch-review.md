# Branch Review: feat/hook-terminals vs origin/main

**Commits:** 3 | **Files changed:** 26 | **Insertions:** +1,884 | **Deletions:** -205

## Features

1. PTY-Backed Hook Execution — hooks now run in visible terminal panes instead of headless background processes
2. Hook Terminal Data Model & Lifecycle — track, register, update, and remove hook terminals per project
3. Hook Terminal UI in Sidebar — collapsible "Hooks" group in the sidebar with status icons and dismiss buttons
4. Hook Log Overlay & Status Bar Indicator — execution history modal and running-hook count in the status bar
5. Pending Worktree Close with Hook-Gated Deletion — `before_worktree_remove` hooks run visibly and gate the actual removal

---

## 1. PTY-Backed Hook Execution

### What it does

Hooks (on_project_open, on_worktree_create, pre_merge, etc.) previously ran as invisible background processes — users had no way to see their output or interact with them. Now every hook spawns a real PTY terminal that appears as a visible pane within the project. Users can watch hook scripts execute in real time, scroll through output, and diagnose failures directly instead of guessing why something went wrong from a toast notification.

### How it's built

A new `HookRunner` struct (`src/workspace/hooks.rs:16`) is stored as a GPUI Global, bundling a `TerminalBackend` and `TerminalsRegistry`. It's initialized during app startup alongside the service manager (`src/app/mod.rs:151`). Each hook function (`fire_on_project_open`, `fire_pre_merge`, etc.) checks for a `HookRunner` in the GPUI context — when present, it creates a PTY terminal via `create_hook_terminal` instead of spawning a headless `sh -c`. The headless path is preserved as a fallback for tests and contexts where no runner is available.

A companion `HookMonitor` (`src/workspace/hook_monitor.rs:47`) tracks execution history, queues toast notifications on failure, and provides an exit-waiter channel mechanism for synchronous hooks (`pre_merge`, `before_worktree_remove`) that need to block until the PTY process exits. The PTY event loop in `src/app/mod.rs:478` notifies exit waiters for all terminals on every batch, unblocking any sync hooks waiting in `smol::unblock`.

### How to test it

1. Configure an `on_project_open` hook in settings (e.g., `"on_project_open": "echo 'Hello from hook'; sleep 2"`)
2. Add a new project
3. A terminal pane should appear split into the project showing the hook output in real time
4. After 2 seconds, the hook should complete and the sidebar should update from a running icon to a checkmark

### What changed

Hooks that previously ran silently in the background now spawn visible PTY terminals. The `run_hook` function (`src/workspace/hooks.rs:184`) attempts the PTY path first when a `HookRunner` is available, falling back to headless execution otherwise. The synchronous variant `run_hook_sync` (`src/workspace/hooks.rs:291`) uses the monitor's exit-waiter channel to block until the PTY process exits, enabling pre-merge and before-worktree-remove hooks to gate operations on hook success.

The `HookMonitor` (`src/workspace/hook_monitor.rs:51`) maintains a capped history (50 entries), tracks running count, queues toast notifications for failures, and manages exit-waiter channels for synchronous hooks. Exit notifications flow from the PTY event loop through `notify_exit` and `finish_by_terminal_id`.

---

## 2. Hook Terminal Data Model & Lifecycle

### What it does

Hook terminals are now first-class citizens in the project data model. Each project tracks its active hook terminals with status (running/succeeded/failed), and hook terminals are automatically added to the project's layout tree so they render as visible terminal panes. When a hook finishes or is dismissed, the terminal and its layout node are cleaned up.

### How it's built

`ProjectData` gained a transient `hook_terminals: HashMap<String, HookTerminalEntry>` field (`src/workspace/state.rs:152`) that maps terminal IDs to hook metadata. `HookTerminalEntry` and `HookTerminalStatus` (`src/workspace/state.rs:80-95`) carry the hook type, label, project association, and current status. `PendingWorktreeClose` (`src/workspace/state.rs:98`) tracks deferred worktree removals waiting on a hook.

`Workspace` exposes `register_hook_terminal` (`src/workspace/state.rs:368`), `update_hook_terminal_status` (`src/workspace/state.rs:404`), `remove_hook_terminal` (`src/workspace/state.rs:419`), and `is_hook_terminal` (`src/workspace/state.rs:441`). Registration automatically splits the hook terminal into the project layout as a 70/30 horizontal split.

### How to test it

1. Trigger a hook (e.g., open a project with `on_project_open` configured)
2. Verify the hook terminal appears as a pane split into the project layout
3. After the hook completes, click the dismiss (X) button in the sidebar
4. Verify the terminal pane is removed and the layout collapses back

### What changed

Hook terminal results are now registered into `ProjectData.hook_terminals` immediately after hook functions fire. This happens in `add_project` (`src/workspace/actions/project.rs:63`), `create_worktree_project` (`src/workspace/actions/project.rs:319`), and throughout the PTY exit handler in `src/app/mod.rs:507`. The layout integration in `register_hook_terminal` creates a terminal node and splits it into the existing layout, while `remove_hook_terminal` cleans up both the map entry and the layout tree node. Hook terminal IDs are filtered out of the regular terminal list in the sidebar (`src/views/panels/sidebar/mod.rs:1147`) to avoid duplicate display.

---

## 3. Hook Terminal UI in Sidebar

### What it does

Each project in the sidebar now has a collapsible "Hooks" group that shows active hook terminals. Each item displays a status icon (yellow terminal for running, green check for succeeded, red X for failed), the hook label (e.g., "on_project_open (feature/foo)"), and a dismiss button that appears on hover. Projects with active hooks are automatically expanded.

### How it's built

A new `hook_list.rs` module (`src/views/panels/sidebar/hook_list.rs:1`) implements `render_hooks_group_header` and `render_hook_item` on the `Sidebar` struct. The group header reuses the shared `sidebar_group_header` widget. Hook items show status-appropriate SVG icons, the hook label, and a dismiss button that calls `ws.remove_hook_terminal()` and removes the terminal from the registry.

The `SidebarHookInfo` struct (`src/views/panels/sidebar/mod.rs:1101`) carries terminal_id, label, and status for rendering. During sidebar data collection, hook terminals are mapped into this struct and projects with active hooks are auto-expanded.

### How to test it

1. Trigger a hook that takes a few seconds (e.g., `"on_project_open": "sleep 5"`)
2. In the sidebar, verify the project expands and shows a "Hooks" group with a running indicator
3. After the hook completes, verify the icon changes to a checkmark (or X on failure)
4. Hover over the hook item and click the X to dismiss it
5. Verify the hook terminal is removed from both the sidebar and the layout

### What changed

The sidebar now renders a "Hooks" group between "Services" and other content when a project has active hook terminals. The `GroupKind::Hooks` variant was added (`src/views/panels/sidebar/mod.rs:49`) with collapse/expand support. Hook terminal IDs are excluded from the regular terminal list to prevent double-listing. Auto-expansion ensures users see hook activity immediately.

---

## 4. Hook Log Overlay & Status Bar Indicator

### What it does

A new "Hook Log" modal shows the full execution history of hooks — each entry displays the hook type, project name, command, duration, and status with error details for failures. The status bar shows a "hooks: N" indicator when hooks are running, which is clickable to open the log. A new `ShowHookLog` keybinding action is available in the command palette.

### How it's built

The `HookLog` view (`src/views/overlays/hook_log.rs:11`) reads history from the global `HookMonitor` and refreshes every 500ms to pick up status transitions. Each row is rendered with `render_hook_row` showing a color-coded status icon, hook type, project, duration, command, and error detail. The overlay is managed by `OverlayManager` with standard toggle/close behavior.

The status bar (`src/views/panels/status_bar.rs:233`) conditionally renders a "hooks: N" indicator by reading `HookMonitor.running_count()` from the GPUI global. Clicking dispatches `ShowHookLog`. The `ShowHookLog` action is registered in keybindings (`src/keybindings/descriptions.rs:1`) and wired through `RootView` (`src/views/root/render.rs:448`).

### How to test it

1. Trigger one or more hooks
2. Observe "hooks: 1" (or N) appearing in the status bar
3. Click the indicator — the Hook Log overlay should open showing the running hook
4. Wait for hooks to complete — entries should update to show success/failure with duration
5. Press Escape to close the overlay

### What changed

A new overlay type was added to the overlay management system. The `HookMonitor` serves as the data source, providing history snapshots and running counts. The status bar gained a conditional hooks indicator that appears only when hooks are actively running. Toast notifications for hook failures were already part of the monitor; the log overlay provides the full history view.

---

## 5. Pending Worktree Close with Hook-Gated Deletion

### What it does

When closing a worktree that has a `before_worktree_remove` hook, the close dialog now shows the hook running in a visible terminal instead of blocking the UI. The actual git worktree removal is deferred until the hook exits. If the hook succeeds, the project is deleted optimistically from the UI and the git cleanup runs in the background. If the hook fails, the close is aborted and the project remains. During this process, the sidebar shows a "Closing..." indicator on the project.

### How it's built

The close worktree dialog (`src/views/overlays/close_worktree_dialog.rs:412`) detects whether a `before_worktree_remove` hook exists. When a `HookRunner` is available, it fires the hook asynchronously via `fire_before_worktree_remove_async` and registers a `PendingWorktreeClose` on the workspace. The PTY exit handler in `src/app/mod.rs:531` checks for pending closes on each hook terminal exit — on success, it deletes the project immediately and runs git cleanup in a background thread; on failure, it clears the closing state and shows a toast.

The `closing_projects` set on `Workspace` (`src/workspace/state.rs:284`) drives the "Closing..." visual state in the sidebar. A new `get_default_branch` helper (`src/git/repository.rs:1`) reads the repo's default branch for the close dialog.

### How to test it

1. Configure a `before_worktree_remove` hook (e.g., `"before_worktree_remove": "echo 'cleaning up'; sleep 3"`)
2. Create a worktree from a project
3. Open the close worktree dialog for that worktree
4. Click "Remove" — the hook terminal should appear and the project should show "Closing..."
5. After 3 seconds, the project should disappear from the sidebar
6. Test with a failing hook (`"before_worktree_remove": "exit 1"`) — the close should abort with a toast

### What changed

The close worktree dialog gained an async hook execution path that avoids blocking the UI thread. Previously, the `before_worktree_remove` hook ran synchronously via `smol::unblock`, freezing the dialog. Now when a `HookRunner` is present, the hook fires as a visible terminal and the dialog closes immediately, with the actual removal deferred to the PTY exit handler. The fallback synchronous path is preserved for when no runner is available. A new `PendingWorktreeClose` struct tracks the deferred operation, and `closing_projects` provides the UI state for the "Closing..." indicator.
