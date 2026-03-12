# Hook Terminals — Design Spec

## Problem

Hook commands (`on_worktree_create`, `pre_merge`, etc.) run as invisible background processes. Users cannot see their output, diagnose failures beyond a toast message, or monitor long-running hooks. The hook log overlay only shows status/duration, not terminal output.

## Solution

Run every hook through a PTY-backed terminal and display it as a tab in the existing service panel. Hooks become first-class visible processes alongside services.

## Design Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Panel location | Shared service panel | Reuses existing infrastructure, avoids panel proliferation |
| PTY for all hooks | Yes, always | Simplifies model — no two execution paths |
| Auto-dismiss | On success after ~2s | Failed hooks need attention; successful ones are noise |
| Tab label | `hook_type (context)` | e.g. `on_worktree_create (feature/auth)` |
| Sync hooks | PTY + await exit | `pre_merge`, `before_worktree_remove` still block caller |
| Persistence | `#[serde(skip)]` | Hook terminals are ephemeral — not saved to workspace.json |
| Panel auto-open | On hook failure | Ensures failed hooks are discoverable even if panel was closed |

## Architecture

### Data Model

**New field on `ProjectData`:**

```rust
/// terminal_id -> HookTerminalEntry
#[serde(skip)]
pub hook_terminals: HashMap<String, HookTerminalEntry>,
```

Uses `#[serde(skip)]` like `remote_services` — ephemeral runtime state, not persisted to `workspace.json`. On app start, this map is empty.

**New struct (in `state.rs`):**

```rust
#[derive(Clone, Debug)]
pub struct HookTerminalEntry {
    pub hook_type: String,        // "on_worktree_create", "pre_merge", etc.
    pub label: String,            // "on_worktree_create (feature/auth)"
    pub project_id: String,       // which project this hook belongs to
    pub status: HookTerminalStatus,
}

#[derive(Clone, Debug, PartialEq)]
pub enum HookTerminalStatus {
    Running,
    Succeeded,
    Failed { exit_code: i32 },
}
```

No `Serialize`/`Deserialize` needed since the field is `#[serde(skip)]`.

The map is keyed by `terminal_id` (not hook execution ID) because the PTY event loop already has `terminal_id` when it receives exit events — this avoids an extra lookup.

### Two-Step Terminal Creation

Creating a hook terminal requires two steps, mirroring `ServiceManager::start_okena_service()`:

```
1. backend.create_terminal(project_path, ShellType::Custom { ... })
   → Returns terminal_id (PTY is now running)

2. Terminal::new(terminal_id, TerminalSize::default(), backend.transport(), cwd)
   → Insert Arc<Terminal> into TerminalsRegistry
   → This is the UI-level alacritty wrapper that TerminalPane looks up
```

Both steps are thread-safe: `backend` is `Arc<dyn TerminalBackend>` (Send + Sync), and `TerminalsRegistry` is `Arc<Mutex<HashMap>>`.

### HookRunner

Bundle hook execution dependencies into a clonable struct to avoid bloating every `fire_*` function with 4+ extra parameters:

```rust
#[derive(Clone)]
pub struct HookRunner {
    backend: Arc<dyn TerminalBackend>,
    terminals: TerminalsRegistry,
    monitor: Option<HookMonitor>,
}
```

Initialized once in `app/mod.rs` alongside `ServiceManager` setup, stored as a GPUI Global. The `fire_*` functions that take `cx: &App` retrieve it via `cx.try_global::<HookRunner>()`. The dialog clones it before spawning async work.

Methods on `HookRunner`:
- `run_hook_pty(command, env_vars, project_path, hook_type, label) -> String` — creates PTY terminal, registers in terminals registry, returns terminal_id
- `run_hook_pty_sync(command, env_vars, project_path, hook_type, label) -> (String, std::sync::mpsc::Receiver<Option<u32>>)` — same as above, also registers an exit waiter, returns terminal_id + receiver

### Environment Variables

Hook env vars (`OKENA_PROJECT_ID`, `OKENA_BRANCH`, etc.) are passed to the PTY command using the existing `LayoutNode::new_terminal_with_command` pattern: shell-escape and inline as a prefix:

```
OKENA_PROJECT_ID='abc' OKENA_BRANCH='feature/auth' npm install
```

This is the same approach already used by `terminal:` hook actions and is battle-tested in the codebase. On Windows, the `#[cfg(windows)]` path uses `cmd /C` with `set VAR=value &&` syntax, matching existing behavior.

### Execution Flow

#### Async hooks (`run_hook`)

Current flow:
```
run_hook(command, env_vars, ...) → std::thread::spawn → Command::new("sh -c") → stdout:null, stderr:piped
```

New flow:
```
fire_on_*(cx: &App):
  1. Get HookRunner from cx global
  2. runner.run_hook_pty(command, env_vars, project_path, hook_type, label)
     → Creates PTY + Terminal, inserts into TerminalsRegistry
     → Returns terminal_id
  3. Register terminal_id in workspace.hook_terminals (status: Running)
  4. HookMonitor.record_start(..., terminal_id)
  5. On PTY exit (handled by app event loop):
     a. If exit_code == 0: mark Succeeded, schedule removal after 2s
     b. If exit_code != 0: mark Failed, auto-open service panel
```

#### Sync hooks (`run_hook_sync`)

Used by `pre_merge` and `before_worktree_remove` — must block the caller and return success/failure.

These are called inside `smol::unblock` closures which have no `cx` access. The `HookRunner` is cloned before entering the closure.

```
close_worktree_dialog.rs:
  1. Clone HookRunner before cx.spawn()
  2. Inside smol::unblock closure:
     a. runner.run_hook_pty_sync(command, env_vars, ...)
        → Creates PTY + Terminal + exit waiter
        → Returns (terminal_id, exit_receiver)
     b. exit_receiver.recv() — blocks until PTY exits
     c. Return Ok/Err based on exit code
  3. After smol::unblock returns, in async block with cx access:
     a. Register terminal_id in workspace.hook_terminals via cx.update()
     b. HookMonitor status already updated by event loop
```

Note: The Terminal UI object and TerminalsRegistry insertion happen inside the `smol::unblock` closure (both are plain data/Arc<Mutex>, no `cx` needed). Only the `workspace.hook_terminals` registration requires `cx.update()` afterward.

### Exit Waiter (for sync hooks)

Use `std::sync::mpsc` (stdlib, blocking recv, no new dependency):

```rust
// In HookMonitor:
exit_waiters: HashMap<String, std::sync::mpsc::Sender<Option<u32>>>,

// Methods:
fn register_exit_waiter(&self, terminal_id: &str) -> std::sync::mpsc::Receiver<Option<u32>>
fn notify_exit(&self, terminal_id: &str, exit_code: Option<u32>)
```

The PTY event loop calls `monitor.notify_exit()`. The sync hook blocks on `receiver.recv()`. Exit code is `Option<u32>` matching `PtyEvent::Exit` (None = killed by signal, treated as failure).

### PTY Exit Handling

In `app/mod.rs` PTY event loop, after checking `service_manager.handle_service_exit()`:

```
PtyEvent::Exit { terminal_id, exit_code } →
  1. Check service_manager.handle_service_exit() — if true, done
  2. Check if terminal_id is in any project's hook_terminals
  3. If yes:
     a. Notify HookExitWatcher (for sync hooks awaiting exit)
     b. Update HookMonitor record_finish
     c. If exit_code == Some(0):
        - Update hook_terminals status to Succeeded
        - Schedule cleanup after 2s (remove from hook_terminals + terminals registry)
        - Cancel cleanup if user clicks on the tab during the 2s window
     d. Otherwise:
        - Update hook_terminals status to Failed { exit_code }
        - Auto-open service panel if not already open
        - Keep terminal for inspection
     e. Do NOT remove from terminals registry immediately (user may want to scroll output)
```

### Service Panel Integration

**Tab rendering** — extend `render_service_panel()`:

After rendering service tabs, render hook terminal tabs:
- Iterate `project.hook_terminals` from workspace
- Tab label: `entry.label` (e.g. "on_worktree_create (feature/auth)")
- Status indicator:
  - Running: yellow dot
  - Failed: red dot
  - (Succeeded hooks auto-remove after 2s, but briefly show green dot)
- Click: same pattern as `show_service()` — creates `TerminalPane` for the hook's terminal_id
- Close button (x) on hook tabs to manually dismiss (removes from `hook_terminals` + kills terminal)

**Visual distinction from services:**
- Hook tabs use a `>_` (terminal/command) icon vs service gear icon

**Auto-open on failure:**
- When a hook fails and the service panel is closed, auto-open it and activate the failed hook's tab
- This ensures failures are discoverable without the user needing to check the panel

**Auto-dismiss cancellation:**
- If the user clicks on a succeeded hook tab during the 2s auto-dismiss window, cancel the timer
- The tab then persists until manually closed

### Hook Runner Changes

The `fire_*` functions get a simpler signature change — they now accept `Option<&HookRunner>` instead of gaining 4 separate parameters:

**For hooks with `cx: &App` access** (`fire_on_project_open`, `fire_on_project_close`, `fire_on_worktree_create`, `fire_on_worktree_close`):
- Retrieve `HookRunner` from `cx.try_global()`
- Pass to `run_hook` which calls `runner.run_hook_pty()`
- Register result in workspace

**For hooks called from background threads** (`fire_pre_merge`, `fire_before_worktree_remove`, etc.):
- Caller clones `HookRunner` before `cx.spawn()`
- Passes cloned runner through to hook functions
- For sync hooks: runner creates PTY + exit waiter inside `smol::unblock`
- Caller registers terminal in workspace after `smol::unblock` returns

**Fallback:** If `HookRunner` is not available (e.g., in tests), hooks fall back to the current `std::process::Command` behavior. This is the `Option<&HookRunner>` — None means "run headless."

### Label Construction

Labels are built from hook_type + context:

| Hook | Context source | Example label |
|------|---------------|---------------|
| `on_project_open` | project_name | `on_project_open (my-app)` |
| `on_project_close` | project_name | `on_project_close (my-app)` |
| `on_worktree_create` | OKENA_BRANCH env var | `on_worktree_create (feature/auth)` |
| `on_worktree_close` | project_name | `on_worktree_close (my-app (feat))` |
| `pre_merge` | OKENA_BRANCH | `pre_merge (feature/auth)` |
| `post_merge` | OKENA_BRANCH | `post_merge (feature/auth)` |
| `before_worktree_remove` | OKENA_BRANCH | `before_worktree_remove (feature/auth)` |
| `worktree_removed` | OKENA_BRANCH | `worktree_removed (feature/auth)` |
| `on_rebase_conflict` | OKENA_BRANCH | `on_rebase_conflict (feature/auth)` |
| `on_dirty_worktree_close` | OKENA_BRANCH | `on_dirty_worktree_close (feature/auth)` |

### HookMonitor Updates

Add `terminal_id: Option<String>` to `HookExecution` so the hook log overlay can link to the terminal:

```rust
pub struct HookExecution {
    pub id: u64,
    pub hook_type: &'static str,
    pub command: String,
    pub project_name: String,
    pub started_at: Instant,
    pub status: HookStatus,
    pub terminal_id: Option<String>,  // NEW
}
```

Add exit waiter support (for sync hooks):

```rust
// In HookMonitorInner:
exit_waiters: HashMap<String, std::sync::mpsc::Sender<Option<u32>>>,
```

Methods:
- `register_exit_waiter(terminal_id) -> std::sync::mpsc::Receiver<Option<u32>>` — called by sync hook runner before blocking
- `notify_exit(terminal_id, exit_code)` — called by PTY event loop; sends through channel and removes waiter

### Cleanup

**Success cleanup (2s delay):**
- After PTY exits with code 0, spawn a delayed task via `cx.spawn`:
  - Wait 2s
  - Remove from `project.hook_terminals`
  - Remove `Terminal` from `TerminalsRegistry`
  - Track the spawn handle so it can be cancelled if the user clicks the tab

**Manual cleanup:**
- Close button (x) on failed hook tabs calls `remove_hook_terminal`
- Also accessible from hook log overlay (click on entry with terminal_id)

### `terminal:` Hook Actions

The existing `terminal:` prefix in hook commands (which spawns a full layout pane) remains unchanged. That feature creates a regular `LayoutNode::Terminal` via `add_terminal_with_command`. The new PTY-backed execution only applies to background hook commands (lines without `terminal:` prefix).

## Testing

### What to test

- `HookRunner::run_hook_pty` creates terminal and inserts into registry (GPUI test with mock backend)
- Exit waiter: `register_exit_waiter` + `notify_exit` round-trip delivers correct exit code
- Label construction from hook_type + env vars
- `hook_terminals` map operations: add, remove, status update
- Fallback: when `HookRunner` is None, hooks still execute via `std::process::Command`

### What NOT to test

- PTY creation internals (already tested by terminal/service infrastructure)
- Service panel rendering (UI-level, not unit-testable)
- `#[serde(skip)]` correctness (trivially correct by construction)

## Files to Modify

| File | Change |
|------|--------|
| `src/workspace/state.rs` | Add `hook_terminals: HashMap` (serde skip) to `ProjectData`, `HookTerminalEntry`, `HookTerminalStatus` |
| `src/workspace/hooks.rs` | Add `HookRunner` struct, rewrite `run_hook`/`run_hook_sync` to use PTY when runner available, fallback to Command when not |
| `src/workspace/hook_monitor.rs` | Add `terminal_id` to `HookExecution`, add `exit_waiters` map + `register_exit_waiter`/`notify_exit` |
| `src/workspace/mod.rs` | Re-export `HookRunner`, `HookTerminalEntry`, `HookTerminalStatus` |
| `src/app/mod.rs` | Initialize `HookRunner` as GPUI global, handle hook terminal exits in PTY event loop |
| `src/views/panels/project_column.rs` | Render hook tabs in service panel, auto-open on failure |
| `src/views/overlays/hook_log.rs` | Add "show in panel" action for entries with terminal_id |
| `src/views/overlays/close_worktree_dialog.rs` | Clone `HookRunner` before spawn, pass to hook functions, register terminals after sync hooks return |
| All `fire_*` call sites | Pass `Option<&HookRunner>` (retrieved from cx global or cloned) |
