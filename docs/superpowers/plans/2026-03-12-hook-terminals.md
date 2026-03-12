# Hook Terminals Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Run hooks through PTY-backed terminals so users can see output in the service panel, with auto-dismiss on success and persistence on failure.

**Architecture:** Add `HookRunner` (GPUI global bundling backend + terminals registry + monitor) that replaces `std::process::Command` in `run_hook`/`run_hook_sync`. Hook terminals appear as tabs in the existing service panel. PTY exit events route through `app/mod.rs` event loop.

**Tech Stack:** Rust, GPUI, `portable-pty` (via `PtyManager`), `std::sync::mpsc` for exit waiters.

**Spec:** `docs/superpowers/specs/2026-03-12-hook-terminals-design.md`

---

## File Structure

| File | Responsibility |
|------|---------------|
| `src/workspace/hooks.rs` | `HookRunner` struct, PTY-backed `run_hook`/`run_hook_sync`, all `fire_*` functions |
| `src/workspace/hook_monitor.rs` | `terminal_id` on `HookExecution`, exit waiter channel management |
| `src/workspace/state.rs` | `HookTerminalEntry`, `HookTerminalStatus`, `hook_terminals` field on `ProjectData` |
| `src/app/mod.rs` | Initialize `HookRunner` global, handle hook terminal exits in PTY event loop |
| `src/views/panels/project_column.rs` | Render hook tabs in service panel, auto-open on failure |
| `src/views/overlays/close_worktree_dialog.rs` | Clone `HookRunner`, pass to hook functions, register sync hook terminals |
| `src/main.rs` | No changes needed (HookMonitor already initialized here) |

## Critical Design Notes

### Sync hook deadlock prevention

Sync hooks (`pre_merge`, `before_worktree_remove`) block inside `smol::unblock` waiting for `exit_waiter.recv()`. The PTY event loop in `app/mod.rs` sends exit codes through the waiter. To avoid deadlock:

- `monitor.notify_exit()` MUST be called for ALL `PtyEvent::Exit` events, BEFORE checking `workspace.is_hook_terminal()`. The `notify_exit` method is a no-op when no waiter is registered for that terminal_id.
- Workspace registration of sync hook terminals happens AFTER `smol::unblock` returns (via `cx.update()`), so the PTY event loop cannot use `is_hook_terminal()` to identify them. Instead, the event loop calls `notify_exit()` unconditionally, and uses a separate `HookMonitor.is_hook_terminal()` method (checking by terminal_id in execution history) for status updates.

### All ProjectData constructors need hook_terminals field

Since `#[serde(skip)]` does not use `#[serde(default)]`, direct struct constructors need `hook_terminals: HashMap::new()`. There are ~15 locations across:
- `src/workspace/actions/project.rs` (2 prod + 2 test)
- `src/workspace/persistence.rs` (1 prod + 1 test)
- `src/workspace/actions/folder.rs` (2 test)
- `src/workspace/actions/layout.rs` (2 test)
- `src/views/root/mod.rs` (1 prod — remote project sync)
- `src/workspace/state.rs` (3 test)

---

## Chunk 1: Data Model & HookRunner Foundation

### Task 1: Add HookTerminalEntry and hook_terminals field to ProjectData

**Files:**
- Modify: `src/workspace/state.rs`

- [ ] **Step 1: Add HookTerminalEntry and HookTerminalStatus types**

Add after the `WorktreeMetadata` struct (around line 78):

```rust
/// Status of a hook terminal in the service panel.
#[derive(Clone, Debug, PartialEq)]
pub enum HookTerminalStatus {
    Running,
    Succeeded,
    Failed { exit_code: i32 },
}

/// Entry for a hook terminal displayed in the service panel.
#[derive(Clone, Debug)]
pub struct HookTerminalEntry {
    pub hook_type: String,
    pub label: String,
    pub project_id: String,
    pub status: HookTerminalStatus,
}
```

- [ ] **Step 2: Add hook_terminals field to ProjectData**

Add after `remote_git_status` field (around line 120), using `#[serde(skip)]` like `remote_services`:

```rust
    /// Hook terminals currently displayed in the service panel (transient, not persisted)
    #[serde(skip)]
    pub hook_terminals: HashMap<String, HookTerminalEntry>,
```

- [ ] **Step 3: Add workspace methods for hook terminal management**

Add to the `impl Workspace` block (after `sync_service_terminals` around line 327):

```rust
    /// Register a hook terminal for display in the service panel.
    pub fn register_hook_terminal(
        &mut self,
        project_id: &str,
        terminal_id: &str,
        entry: HookTerminalEntry,
        cx: &mut Context<Self>,
    ) {
        if let Some(project) = self.data.projects.iter_mut().find(|p| p.id == project_id) {
            project.hook_terminals.insert(terminal_id.to_string(), entry);
            cx.notify();
        }
    }

    /// Update hook terminal status.
    pub fn update_hook_terminal_status(
        &mut self,
        terminal_id: &str,
        status: HookTerminalStatus,
        cx: &mut Context<Self>,
    ) {
        for project in &mut self.data.projects {
            if let Some(entry) = project.hook_terminals.get_mut(terminal_id) {
                entry.status = status;
                cx.notify();
                return;
            }
        }
    }

    /// Remove a hook terminal.
    pub fn remove_hook_terminal(
        &mut self,
        terminal_id: &str,
        cx: &mut Context<Self>,
    ) {
        for project in &mut self.data.projects {
            if project.hook_terminals.remove(terminal_id).is_some() {
                cx.notify();
                return;
            }
        }
    }

    /// Check if a terminal_id belongs to a hook terminal. Returns the project_id if found.
    pub fn is_hook_terminal(&self, terminal_id: &str) -> Option<String> {
        for project in &self.data.projects {
            if project.hook_terminals.contains_key(terminal_id) {
                return Some(project.id.clone());
            }
        }
        None
    }
```

- [ ] **Step 4: Add hook_terminals field to ALL ProjectData constructors**

Search for all `ProjectData {` in the codebase and add `hook_terminals: HashMap::new()` to each. Locations:

Production code:
- `src/workspace/actions/project.rs:38` — `add_project`
- `src/workspace/actions/project.rs:264` — `create_worktree_project`
- `src/workspace/persistence.rs:327` — default workspace creation
- `src/views/root/mod.rs:350` — remote project sync

Test helpers:
- `src/workspace/actions/project.rs:349` and `:476` — test `make_project`
- `src/workspace/persistence.rs:358` — test `make_project`
- `src/workspace/actions/folder.rs:149` and `:260` — test `make_project`
- `src/workspace/actions/layout.rs:1257` and `:1503` — test `make_project` / `make_project_with_layout`
- `src/workspace/state.rs:1165`, `:2044`, `:2319` — test `make_project`

Run: `cargo test 2>&1 | head -50`
Expected: compiles and passes

- [ ] **Step 5: Commit**

```bash
git add src/workspace/state.rs src/workspace/actions/project.rs
git commit -m "feat: add HookTerminalEntry data model and workspace methods"
```

---

### Task 2: Add terminal_id and exit waiter support to HookMonitor

**Files:**
- Modify: `src/workspace/hook_monitor.rs`

- [ ] **Step 1: Add terminal_id to HookExecution**

In `HookExecution` struct (line 22), add after `status`:

```rust
    pub terminal_id: Option<String>,
```

- [ ] **Step 2: Update record_start to accept optional terminal_id**

Change the `record_start` signature and body (line 59):

```rust
    pub fn record_start(
        &self,
        hook_type: &'static str,
        command: &str,
        project_name: &str,
        terminal_id: Option<String>,
    ) -> u64 {
```

And in the `HookExecution` initializer inside, add:

```rust
            terminal_id,
```

- [ ] **Step 3: Add exit waiter fields and methods**

Add to `HookMonitorInner` struct (line 32):

```rust
    exit_waiters: HashMap<String, std::sync::mpsc::Sender<Option<u32>>>,
```

Add methods to `impl HookMonitor`:

```rust
    /// Register a waiter for a terminal's exit event. Returns a receiver that
    /// blocks until the PTY exits. Used by sync hooks.
    pub fn register_exit_waiter(&self, terminal_id: &str) -> std::sync::mpsc::Receiver<Option<u32>> {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut inner = self.0.lock();
        inner.exit_waiters.insert(terminal_id.to_string(), tx);
        rx
    }

    /// Notify that a hook terminal has exited. Sends exit code through the
    /// waiter channel (if any) and removes the waiter.
    pub fn notify_exit(&self, terminal_id: &str, exit_code: Option<u32>) {
        let mut inner = self.0.lock();
        if let Some(tx) = inner.exit_waiters.remove(terminal_id) {
            let _ = tx.send(exit_code);
        }
    }
```

- [ ] **Step 4: Update all record_start call sites**

In `src/workspace/hooks.rs`, update all calls to `record_start` to pass `None` as the last argument (we'll update them to pass real terminal_ids in Task 3):

Search for `m.record_start(` and add `, None` before the closing `)`.

There are two call sites in hooks.rs:
- `run_hook` (around line 86): `m.record_start(hook_type, &command, project_name, None)`
- `run_hook_sync` (around line 155): `m.record_start(hook_type, command, project_name, None)`

Also update existing tests in `hook_monitor.rs` that call `record_start` — add `None` as the 4th arg:
- `record_start_and_finish_success`: `monitor.record_start("on_project_open", "echo hi", "my-project", None)`
- `record_failure_queues_toast`: `monitor.record_start("pre_merge", "exit 1", "test-project", None)`
- `history_capped_at_max`: `monitor.record_start("test", &format!("cmd-{}", i), "proj", None)`
- `history_returned_newest_first`: both calls
- `spawn_error_queues_toast`: `monitor.record_start("on_project_open", "bad-cmd", "proj", None)`

- [ ] **Step 5: Add exit waiter test**

Add to the `#[cfg(test)]` module in `hook_monitor.rs`:

```rust
    #[test]
    fn exit_waiter_delivers_exit_code() {
        let monitor = HookMonitor::new();
        let rx = monitor.register_exit_waiter("terminal-1");
        monitor.notify_exit("terminal-1", Some(0));
        assert_eq!(rx.recv().unwrap(), Some(0));
    }

    #[test]
    fn exit_waiter_delivers_none_for_signal_kill() {
        let monitor = HookMonitor::new();
        let rx = monitor.register_exit_waiter("terminal-2");
        monitor.notify_exit("terminal-2", None);
        assert_eq!(rx.recv().unwrap(), None);
    }

    #[test]
    fn exit_waiter_removed_after_notify() {
        let monitor = HookMonitor::new();
        let _rx = monitor.register_exit_waiter("terminal-3");
        monitor.notify_exit("terminal-3", Some(1));
        // Second notify should not panic (waiter already consumed)
        monitor.notify_exit("terminal-3", Some(1));
    }
```

- [ ] **Step 6: Run tests**

Run: `cargo test hook_monitor`
Expected: all tests pass (including new exit waiter tests)

- [ ] **Step 7: Commit**

```bash
git add src/workspace/hook_monitor.rs src/workspace/hooks.rs
git commit -m "feat: add terminal_id to HookExecution and exit waiter support"
```

---

### Task 3: Create HookRunner struct and GPUI global

**Files:**
- Modify: `src/workspace/hooks.rs`
- Modify: `src/workspace/mod.rs`

- [ ] **Step 1: Add HookRunner struct**

Add after the imports at the top of `hooks.rs`:

```rust
use crate::terminal::backend::TerminalBackend;
use crate::terminal::terminal::{Terminal, TerminalSize};
use crate::terminal::shell_config::ShellType;
use crate::views::root::TerminalsRegistry;
use std::sync::Arc;

/// Bundles the dependencies needed to run hooks through PTY terminals.
/// Stored as a GPUI Global. All fields are Clone + Send + Sync.
#[derive(Clone)]
pub struct HookRunner {
    pub backend: Arc<dyn TerminalBackend>,
    pub terminals: TerminalsRegistry,
}

impl gpui::Global for HookRunner {}
```

- [ ] **Step 2: Add run_hook_pty method**

Add to `impl HookRunner`:

```rust
impl HookRunner {
    /// Create a PTY-backed terminal for a hook command.
    /// Returns the terminal_id. The terminal is registered in the TerminalsRegistry.
    fn create_hook_terminal(
        &self,
        command: &str,
        env_vars: &HashMap<String, String>,
        project_path: &str,
    ) -> Result<String, String> {
        // Build environment prefix for the command
        let full_cmd = if cfg!(windows) {
            // Windows: set KEY=value && set KEY2=value2 && command
            let env_prefix = env_vars
                .iter()
                .map(|(k, v)| format!("set {}={}", k, v))
                .collect::<Vec<_>>()
                .join(" && ");
            if env_prefix.is_empty() {
                command.to_string()
            } else {
                format!("{} && {}", env_prefix, command)
            }
        } else {
            // Unix: KEY='value' KEY2='value2' command
            let env_prefix = env_vars
                .iter()
                .map(|(k, v)| format!("{}='{}'", k, v.replace('\'', "'\\''")))
                .collect::<Vec<_>>()
                .join(" ");
            if env_prefix.is_empty() {
                command.to_string()
            } else {
                format!("{} {}", env_prefix, command)
            }
        };

        let shell = if cfg!(windows) {
            ShellType::Custom {
                path: "cmd".to_string(),
                args: vec!["/C".to_string(), full_cmd],
            }
        } else {
            ShellType::Custom {
                path: "sh".to_string(),
                args: vec!["-c".to_string(), full_cmd],
            }
        };

        let terminal_id = self.backend.create_terminal(project_path, Some(&shell))
            .map_err(|e| format!("Failed to create hook terminal: {}", e))?;

        let terminal = Arc::new(Terminal::new(
            terminal_id.clone(),
            TerminalSize::default(),
            self.backend.transport(),
            project_path.to_string(),
        ));
        self.terminals.lock().insert(terminal_id.clone(), terminal);

        Ok(terminal_id)
    }
}
```

- [ ] **Step 3: Add build_hook_label helper**

```rust
/// Build a display label for a hook terminal tab.
fn build_hook_label(hook_type: &str, env_vars: &HashMap<String, String>, project_name: &str) -> String {
    let context = env_vars.get("OKENA_BRANCH")
        .map(|s| s.as_str())
        .unwrap_or(project_name);
    format!("{} ({})", hook_type, context)
}
```

- [ ] **Step 4: Add re-export in mod.rs**

In `src/workspace/mod.rs`, the module `hooks` is already declared. Add a re-export if needed, or just ensure `HookRunner` is `pub`. Since `hooks` is `pub mod`, `HookRunner` is accessible as `crate::workspace::hooks::HookRunner`.

- [ ] **Step 5: Verify compilation**

Run: `cargo check`
Expected: compiles (HookRunner is defined but not yet used)

- [ ] **Step 6: Commit**

```bash
git add src/workspace/hooks.rs src/workspace/mod.rs
git commit -m "feat: add HookRunner struct with PTY terminal creation"
```

---

## Chunk 2: Hook Execution via PTY

### Task 4: Rewrite run_hook to use PTY when HookRunner available

**Files:**
- Modify: `src/workspace/hooks.rs`

- [ ] **Step 1: Add HookRunner parameter to run_hook**

Change `run_hook` signature to accept an optional `HookRunner`:

```rust
fn run_hook(
    command: String,
    env_vars: HashMap<String, String>,
    monitor: Option<&HookMonitor>,
    hook_type: &'static str,
    project_name: &str,
    runner: Option<&HookRunner>,
) {
```

- [ ] **Step 2: Add PTY path at the top of run_hook**

At the beginning of `run_hook`, before the existing `std::thread::spawn`, add the PTY path:

```rust
    // PTY path: create a real terminal so output is visible in the service panel
    if let Some(runner) = runner {
        let project_path = env_vars.get("OKENA_PROJECT_PATH").cloned().unwrap_or_default();
        let label = build_hook_label(hook_type, &env_vars, project_name);

        match runner.create_hook_terminal(&command, &env_vars, &project_path) {
            Ok(terminal_id) => {
                let exec_id = monitor.map(|m| m.record_start(hook_type, &command, project_name, Some(terminal_id.clone())));
                log::info!("Hook '{}' started in terminal {} (label: {})", hook_type, terminal_id, label);
                // Exit is handled by the PTY event loop in app/mod.rs
                // which will call monitor.record_finish() and update hook_terminals
                let _ = (exec_id, label); // exec_id tracked by monitor, label used by caller
            }
            Err(e) => {
                log::error!("Failed to create hook terminal for '{}': {}", hook_type, e);
                if let Some(m) = monitor {
                    let id = m.record_start(hook_type, &command, project_name, None);
                    m.record_finish(id, HookStatus::SpawnError { message: e });
                }
            }
        }
        return;
    }

    // Fallback: headless execution (no HookRunner, e.g. in tests)
    // ... existing std::thread::spawn code stays here unchanged ...
```

- [ ] **Step 3: Update run_hook to return terminal info**

Actually, the caller needs the terminal_id and label to register in workspace. Change return type:

```rust
/// Result of a hook execution via PTY.
#[derive(Clone)]
pub struct HookTerminalResult {
    pub terminal_id: String,
    pub label: String,
    pub hook_type: String,
    pub project_id: String,
}

fn run_hook(
    command: String,
    env_vars: HashMap<String, String>,
    monitor: Option<&HookMonitor>,
    hook_type: &'static str,
    project_name: &str,
    runner: Option<&HookRunner>,
    project_id: &str,
) -> Option<HookTerminalResult> {
```

In the PTY success path, return:

```rust
                return Some(HookTerminalResult {
                    terminal_id,
                    label,
                    hook_type: hook_type.to_string(),
                    project_id: project_id.to_string(),
                });
```

The fallback `std::thread::spawn` path must explicitly return `None` at the end of the function (after the spawn). The error path in the PTY branch also returns `None`.

- [ ] **Step 4: Update run_hook_actions to pass runner through**

Update `run_hook_actions` signature to also take `runner: Option<&HookRunner>` and `project_id: &str`, and pass them to `run_hook`. Collect any `HookTerminalResult` values and return them alongside terminal actions:

```rust
fn run_hook_actions(
    command: &str,
    env_vars: HashMap<String, String>,
    monitor: Option<&HookMonitor>,
    hook_type: &'static str,
    project_name: &str,
    runner: Option<&HookRunner>,
    project_id: &str,
) -> (Vec<(String, HashMap<String, String>)>, Vec<HookTerminalResult>) {
    let actions = parse_hook_actions(command);
    let mut terminal_actions = Vec::new();
    let mut hook_results = Vec::new();

    for action in actions {
        match action {
            HookAction::Background(cmd) => {
                if let Some(result) = run_hook(cmd, env_vars.clone(), monitor, hook_type, project_name, runner, project_id) {
                    hook_results.push(result);
                }
            }
            HookAction::Terminal(cmd) => {
                terminal_actions.push((cmd, env_vars.clone()));
            }
        }
    }

    (terminal_actions, hook_results)
}
```

- [ ] **Step 5: Verify compilation**

Run: `cargo check`
Expected: compilation errors in `fire_*` functions (they call `run_hook`/`run_hook_actions` with old signatures). That's expected — we'll fix them in Task 5.

- [ ] **Step 6: Commit work-in-progress**

```bash
git add src/workspace/hooks.rs
git commit -m "wip: rewrite run_hook to use PTY via HookRunner"
```

---

### Task 5: Rewrite run_hook_sync for PTY with exit waiting

**Files:**
- Modify: `src/workspace/hooks.rs`

- [ ] **Step 1: Update run_hook_sync signature**

```rust
fn run_hook_sync(
    command: &str,
    env_vars: HashMap<String, String>,
    monitor: Option<&HookMonitor>,
    hook_type: &'static str,
    project_name: &str,
    runner: Option<&HookRunner>,
    project_id: &str,
) -> Result<Option<HookTerminalResult>, String> {
```

Returns `Ok(Some(result))` with terminal info on success via PTY, `Ok(None)` on headless success, `Err` on failure.

- [ ] **Step 2: Add PTY path to run_hook_sync**

At the beginning, before existing Command code:

```rust
    if let Some(runner) = runner {
        let project_path = env_vars.get("OKENA_PROJECT_PATH").cloned().unwrap_or_default();
        let label = build_hook_label(hook_type, &env_vars, project_name);
        let start = Instant::now();

        let terminal_id = runner.create_hook_terminal(command, &env_vars, &project_path)?;

        let exec_id = monitor.map(|m| m.record_start(hook_type, command, project_name, Some(terminal_id.clone())));

        // Register exit waiter and block until the PTY process exits
        let rx = monitor
            .map(|m| m.register_exit_waiter(&terminal_id))
            .ok_or_else(|| "HookMonitor required for sync PTY hooks".to_string())?;

        let exit_code = rx.recv().map_err(|_| "Hook terminal exit channel closed unexpectedly".to_string())?;
        let duration = start.elapsed();

        let success = exit_code == Some(0);

        if success {
            if let (Some(m), Some(id)) = (monitor, exec_id) {
                m.record_finish(id, HookStatus::Succeeded { duration });
            }
            return Ok(Some(HookTerminalResult {
                terminal_id,
                label,
                hook_type: hook_type.to_string(),
                project_id: project_id.to_string(),
            }));
        } else {
            let code = exit_code.map(|c| c as i32).unwrap_or(-1);
            if let (Some(m), Some(id)) = (monitor, exec_id) {
                m.record_finish(id, HookStatus::Failed {
                    duration,
                    exit_code: code,
                    stderr: String::new(),
                });
            }
            return Err(format!("Hook failed (exit {})", code));
        }
    }

    // Fallback: headless (existing code, returns Ok(None) on success)
```

Update the existing fallback success path to return `Ok(None)` instead of `Ok(())`, and `Err(...)` stays the same.

- [ ] **Step 4: Verify compilation**

Run: `cargo check`
Expected: Still errors in `fire_*` functions — next task fixes them.

- [ ] **Step 5: Commit**

```bash
git add src/workspace/hooks.rs
git commit -m "wip: rewrite run_hook_sync to use PTY with exit waiting"
```

---

### Task 6: Update all fire_* functions to use HookRunner

**Files:**
- Modify: `src/workspace/hooks.rs`

- [ ] **Step 1: Add try_runner helper**

Next to the existing `try_monitor`:

```rust
/// Try to get the global HookRunner from GPUI context.
pub fn try_runner(cx: &App) -> Option<HookRunner> {
    cx.try_global::<HookRunner>().cloned()
}
```

- [ ] **Step 2: Update fire_on_project_open**

Add HookRunner retrieval and workspace registration. The function needs to also accept `workspace: &Entity<Workspace>` and `cx` must be `&mut App` (or we need to pass workspace separately). Looking at the current call site in `project.rs:61`, it's called from `&mut Context<Workspace>` which derefs to `&App`.

Actually, since `fire_on_project_open` is called from `Workspace` methods that have `cx: &mut Context<Self>`, we can't update workspace from within the hook function (we're already borrowing workspace). Instead, return the `HookTerminalResult` and let the caller register it.

Change signature to return results:

```rust
pub fn fire_on_project_open(
    project_hooks: &HooksConfig,
    project_id: &str,
    project_name: &str,
    project_path: &str,
    cx: &App,
) -> Vec<HookTerminalResult> {
    let global_hooks = settings(cx).hooks;
    if let Some(cmd) = resolve_hook(project_hooks, &global_hooks, |h| &h.on_project_open) {
        let env = project_env(project_id, project_name, project_path);
        log::info!("Running on_project_open hook for project '{}'", project_name);
        let monitor = try_monitor(cx);
        let runner = try_runner(cx);
        if let Some(result) = run_hook(cmd, env, monitor.as_ref(), "on_project_open", project_name, runner.as_ref(), project_id) {
            return vec![result];
        }
    }
    Vec::new()
}
```

Apply the same pattern to:
- `fire_on_project_close`
- `fire_on_worktree_create`
- `fire_on_worktree_close`

- [ ] **Step 3: Update fire_pre_merge and other hooks that take explicit global_hooks**

These take `monitor: Option<&HookMonitor>`. Add `runner: Option<&HookRunner>`:

```rust
pub fn fire_pre_merge(
    project_hooks: &HooksConfig,
    global_hooks: &HooksConfig,
    project_id: &str,
    project_name: &str,
    project_path: &str,
    branch: &str,
    target_branch: &str,
    main_repo_path: &str,
    monitor: Option<&HookMonitor>,
    runner: Option<&HookRunner>,
) -> Result<Option<HookTerminalResult>, String> {
    if let Some(cmd) = resolve_hook(project_hooks, global_hooks, |h| &h.pre_merge) {
        let env = merge_env(project_id, project_name, project_path, branch, target_branch, main_repo_path);
        log::info!("Running pre_merge hook for project '{}'", project_name);
        return run_hook_sync(&cmd, env, monitor, "pre_merge", project_name, runner, project_id);
    }
    Ok(None)
}
```

Apply the same pattern to `fire_before_worktree_remove`.

For async hooks that take explicit global_hooks (`fire_post_merge`, `fire_worktree_removed`):

```rust
pub fn fire_post_merge(
    ...,
    monitor: Option<&HookMonitor>,
    runner: Option<&HookRunner>,
) -> Vec<HookTerminalResult> {
```

And call `run_hook` with the runner, collecting results.

For hooks returning terminal actions (`fire_on_rebase_conflict`, `fire_on_dirty_worktree_close`):

```rust
pub fn fire_on_rebase_conflict(
    ...,
    monitor: Option<&HookMonitor>,
    runner: Option<&HookRunner>,
) -> (Vec<(String, HashMap<String, String>)>, Vec<HookTerminalResult>) {
```

Call `run_hook_actions` with the new signature.

- [ ] **Step 4: Update callers in workspace/actions/project.rs**

In `add_project` (line 61), the return from `fire_on_project_open` is currently ignored. Now it returns `Vec<HookTerminalResult>`. Register hook terminals after the call:

```rust
        let hook_results = hooks::fire_on_project_open(&project_hooks, &id, &name, &path, cx);
        for result in hook_results {
            self.register_hook_terminal(&result.project_id, &result.terminal_id, HookTerminalEntry {
                hook_type: result.hook_type,
                label: result.label,
                project_id: result.project_id.clone(),
                status: HookTerminalStatus::Running,
            }, cx);
        }
```

Same for `delete_project` (fire_on_project_close), `create_worktree_project` (fire_on_worktree_create), `remove_worktree_project` (fire_on_worktree_close).

- [ ] **Step 5: Update callers in close_worktree_dialog.rs**

Clone `HookRunner` before `cx.spawn()`:

```rust
        let runner = hooks::try_runner(cx);
```

Pass `runner.as_ref()` to all hook calls. For sync hooks inside `smol::unblock`, clone the runner into the closure:

```rust
                    let runner = runner.clone();
                    move || {
                        hooks::fire_pre_merge(
                            ...,
                            monitor.as_ref(),
                            runner.as_ref(),
                        )
                    }
```

After `smol::unblock` returns for sync hooks, register the terminal result via `cx.update()`:

```rust
                if let Ok(Some(result)) = &pre_merge_result {
                    let result = result.clone();
                    let _ = cx.update(|cx| {
                        workspace.update(cx, |ws, cx| {
                            ws.register_hook_terminal(&result.project_id, &result.terminal_id, HookTerminalEntry {
                                hook_type: result.hook_type,
                                label: result.label,
                                project_id: result.project_id.clone(),
                                status: HookTerminalStatus::Running,
                            }, cx);
                        });
                    });
                }
```

For async hooks (fire_post_merge, fire_worktree_removed), register results similarly.

For hooks returning terminal actions + hook results, handle both return values.

- [ ] **Step 6: Update the run_hook_actions test**

The existing test `run_hook_actions_returns_terminal_actions` needs the new parameters:

```rust
    #[test]
    fn run_hook_actions_returns_terminal_actions() {
        let mut env = HashMap::new();
        env.insert("KEY".into(), "val".into());
        let (terminal_actions, _hook_results) = run_hook_actions("terminal: my-cmd\necho bg", env, None, "test", "proj", None, "proj-id");
        assert_eq!(terminal_actions.len(), 1);
        assert_eq!(terminal_actions[0].0, "my-cmd");
        assert_eq!(terminal_actions[0].1.get("KEY").unwrap(), "val");
    }
```

- [ ] **Step 7: Verify compilation and run tests**

Run: `cargo check && cargo test hooks`
Expected: compiles, all tests pass

- [ ] **Step 8: Commit**

```bash
git add src/workspace/hooks.rs src/workspace/actions/project.rs src/views/overlays/close_worktree_dialog.rs
git commit -m "feat: wire HookRunner through all fire_* functions and callers"
```

---

## Chunk 3: PTY Exit Handling & App Wiring

### Task 7: Initialize HookRunner in app/mod.rs

**Files:**
- Modify: `src/app/mod.rs`

- [ ] **Step 1: Create and set HookRunner as GPUI global**

After the `ServiceManager` initialization (around line 148), add:

```rust
        // Create HookRunner for PTY-backed hook execution
        let hook_runner = crate::workspace::hooks::HookRunner {
            backend: local_backend_for_services.clone(),
            terminals: terminals.clone(),
        };
        cx.set_global(hook_runner);
```

Note: reuse `local_backend_for_services` (same `Arc<LocalBackend>`) and `terminals` (same `TerminalsRegistry`).

- [ ] **Step 2: Verify compilation**

Run: `cargo check`
Expected: compiles

- [ ] **Step 3: Commit**

```bash
git add src/app/mod.rs
git commit -m "feat: initialize HookRunner as GPUI global in app startup"
```

---

### Task 8: Handle hook terminal exits in PTY event loop

**Files:**
- Modify: `src/app/mod.rs`

- [ ] **Step 1: Add hook terminal exit handling after service exit handling**

**CRITICAL: Deadlock prevention.** `notify_exit()` MUST be called for ALL exit events BEFORE any workspace access, because sync hooks block inside `smol::unblock` waiting on the exit waiter. If `notify_exit` is gated behind `workspace.is_hook_terminal()`, and workspace registration hasn't happened yet (it happens after `smol::unblock` returns), the sync hook will deadlock.

In the PTY event loop's exit handling block (after the service_tids check, around line 475), add hook terminal handling:

```rust
                    // FIRST: notify exit waiters for ALL terminals unconditionally.
                    // This unblocks sync hooks waiting in smol::unblock.
                    // notify_exit is a no-op for terminals without a registered waiter.
                    if let Some(monitor) = cx.try_global::<crate::workspace::hook_monitor::HookMonitor>() {
                        for (terminal_id, exit_code) in &exit_events {
                            monitor.notify_exit(terminal_id, *exit_code);
                        }
                    }

                    // THEN: identify hook terminals via workspace (only for async hooks
                    // that were already registered — sync hooks register after unblock returns)
                    let hook_tids: std::collections::HashSet<String> = {
                        let ws = this.workspace.read(cx);
                        exit_events.iter()
                            .filter(|(tid, _)| !service_tids.contains(tid))
                            .filter(|(tid, _)| ws.is_hook_terminal(tid).is_some())
                            .map(|(tid, _)| tid.clone())
                            .collect()
                    };

                    for (terminal_id, exit_code) in &exit_events {
                        if !hook_tids.contains(terminal_id) {
                            continue;
                        }

                        let success = *exit_code == Some(0);
                        let workspace = this.workspace.clone();
                        let tid = terminal_id.clone();

                        if success {
                            workspace.update(cx, |ws, cx| {
                                ws.update_hook_terminal_status(&tid, crate::workspace::state::HookTerminalStatus::Succeeded, cx);
                            });

                            // Schedule removal after 2s
                            let tid_for_cleanup = terminal_id.clone();
                            let workspace_for_cleanup = this.workspace.clone();
                            let terminals_for_cleanup = this.terminals.clone();
                            cx.spawn(async move |_this, cx| {
                                cx.background_executor().timer(std::time::Duration::from_secs(2)).await;
                                let _ = cx.update(|cx| {
                                    workspace_for_cleanup.update(cx, |ws, cx| {
                                        ws.remove_hook_terminal(&tid_for_cleanup, cx);
                                    });
                                    terminals_for_cleanup.lock().remove(&tid_for_cleanup);
                                });
                            }).detach();
                        } else {
                            let code = exit_code.map(|c| c as i32).unwrap_or(-1);
                            workspace.update(cx, |ws, cx| {
                                ws.update_hook_terminal_status(&tid, crate::workspace::state::HookTerminalStatus::Failed { exit_code: code }, cx);
                            });
                        }
                    }

                    // Remove UI Terminals for non-service, non-hook terminals
                    {
                        let mut reg = this.terminals.lock();
                        for (terminal_id, _) in &exit_events {
                            if !service_tids.contains(terminal_id) && !hook_tids.contains(terminal_id) {
                                reg.remove(terminal_id);
                            }
                        }
                    }
```

Replace the existing "Remove UI Terminals for non-service terminals" block with the above (which adds hook_tids to the exclusion set and calls `notify_exit` first).

- [ ] **Step 2: Verify compilation**

Run: `cargo check`
Expected: compiles

- [ ] **Step 3: Commit**

```bash
git add src/app/mod.rs
git commit -m "feat: handle hook terminal exits in PTY event loop"
```

---

## Chunk 4: Service Panel UI Integration

### Task 9: Render hook tabs in the service panel

**Files:**
- Modify: `src/views/panels/project_column.rs`

- [ ] **Step 1: Add show_hook_terminal method**

Add a method similar to `show_service` but for hook terminals:

```rust
    pub fn show_hook_terminal(&mut self, terminal_id: &str, cx: &mut Context<Self>) {
        self.active_service_name = Some(format!("hook:{}", terminal_id));
        self.service_panel_open = true;

        let project_path = self.workspace.read(cx).project(&self.project_id)
            .map(|p| p.path.clone())
            .unwrap_or_default();

        let ws = self.workspace.clone();
        let rb = self.request_broker.clone();
        let backend = self.backend.clone();
        let terminals = self.terminals.clone();
        let pid = self.project_id.clone();
        let tid = terminal_id.to_string();

        let pane = cx.new(move |cx| {
            TerminalPane::new(
                ws,
                rb,
                pid,
                project_path,
                vec![usize::MAX],
                Some(tid),
                false,
                false,
                backend,
                terminals,
                None,
                cx,
            )
        });

        self.service_terminal_pane = Some(pane);
        cx.notify();
    }
```

- [ ] **Step 2: Add remove_hook_terminal method for close button**

```rust
    fn dismiss_hook_terminal(&mut self, terminal_id: &str, cx: &mut Context<Self>) {
        let tid = terminal_id.to_string();
        let terminals = self.terminals.clone();
        self.workspace.update(cx, |ws, cx| {
            ws.remove_hook_terminal(&tid, cx);
        });
        terminals.lock().remove(&tid);

        // If this was the active tab, go back to overview
        if self.active_service_name.as_deref() == Some(&format!("hook:{}", terminal_id)) {
            self.show_overview(cx);
        }
        cx.notify();
    }
```

- [ ] **Step 3: Render hook tabs after service tabs**

In `render_service_panel()`, after the service tabs `.children(...)` block (around line 1114), add hook terminal tabs. Read `hook_terminals` from workspace:

```rust
                            // Hook terminal tabs
                            .children({
                                let hook_terminals: Vec<(String, crate::workspace::state::HookTerminalEntry)> = self.workspace.read(cx)
                                    .project(&self.project_id)
                                    .map(|p| p.hook_terminals.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                                    .unwrap_or_default();

                                hook_terminals.into_iter().map(|(tid, entry)| {
                                    let is_active = self.active_service_name.as_deref() == Some(&format!("hook:{}", tid));
                                    let status_color = match &entry.status {
                                        crate::workspace::state::HookTerminalStatus::Running => t.term_yellow,
                                        crate::workspace::state::HookTerminalStatus::Succeeded => t.term_green,
                                        crate::workspace::state::HookTerminalStatus::Failed { .. } => t.term_red,
                                    };
                                    let tid_click = tid.clone();
                                    let tid_close = tid.clone();

                                    div()
                                        .id(ElementId::Name(format!("hook-tab-{}", tid).into()))
                                        .cursor_pointer()
                                        .h(px(34.0))
                                        .px(px(12.0))
                                        .flex()
                                        .items_center()
                                        .flex_shrink_0()
                                        .gap(px(6.0))
                                        .text_size(px(12.0))
                                        .when(is_active, |d| {
                                            d.bg(rgb(t.bg_primary))
                                                .text_color(rgb(t.text_primary))
                                        })
                                        .when(!is_active, |d| {
                                            d.text_color(rgb(t.text_secondary))
                                                .hover(|s| s.bg(rgb(t.bg_hover)))
                                        })
                                        // Status dot
                                        .child(
                                            div()
                                                .flex_shrink_0()
                                                .w(px(7.0))
                                                .h(px(7.0))
                                                .rounded(px(3.5))
                                                .bg(rgb(status_color)),
                                        )
                                        // Label
                                        .child(entry.label.clone())
                                        // Close button (only for non-running hooks)
                                        .when(entry.status != crate::workspace::state::HookTerminalStatus::Running, |d| {
                                            d.child(
                                                div()
                                                    .id(ElementId::Name(format!("hook-close-{}", tid_close).into()))
                                                    .cursor_pointer()
                                                    .ml(px(4.0))
                                                    .text_size(px(10.0))
                                                    .text_color(rgb(t.text_muted))
                                                    .hover(|s| s.text_color(rgb(t.text_primary)))
                                                    .child("×")
                                                    .on_click(cx.listener(move |this, _, _window, cx| {
                                                        this.dismiss_hook_terminal(&tid_close, cx);
                                                    }))
                                            )
                                        })
                                        .on_click(cx.listener(move |this, _, _window, cx| {
                                            this.show_hook_terminal(&tid_click, cx);
                                        }))
                                })
                            })
```

- [ ] **Step 4: Auto-open panel on hook failure**

Add a method that checks for newly failed hooks and opens the panel:

```rust
    /// Called when workspace notifies — check if any hook just failed and auto-open panel
    pub fn check_hook_failures(&mut self, cx: &mut Context<Self>) {
        let has_failed = self.workspace.read(cx)
            .project(&self.project_id)
            .map(|p| p.hook_terminals.values().any(|e| matches!(e.status, crate::workspace::state::HookTerminalStatus::Failed { .. })))
            .unwrap_or(false);

        if has_failed && !self.service_panel_open {
            // Find the first failed hook and show it
            if let Some((tid, _)) = self.workspace.read(cx)
                .project(&self.project_id)
                .and_then(|p| p.hook_terminals.iter().find(|(_, e)| matches!(e.status, crate::workspace::state::HookTerminalStatus::Failed { .. })))
                .map(|(k, v)| (k.clone(), v.clone()))
            {
                self.show_hook_terminal(&tid, cx);
            }
        }
    }
```

This should be called from wherever `ProjectColumn` observes workspace changes. Look at how `ProjectColumn` currently reacts to workspace notifications — likely via a `cx.observe(&workspace, ...)` in its constructor. Add `check_hook_failures(cx)` to that observer.

- [ ] **Step 5: Fix service panel visibility for hook-only scenarios**

Two conditions to update:

**a)** The panel render condition — find where `service_panel_open` is checked and add hook terminal check:

```rust
let has_hook_terminals = self.workspace.read(cx)
    .project(&self.project_id)
    .map(|p| !p.hook_terminals.is_empty())
    .unwrap_or(false);

if !self.service_panel_open && !has_hook_terminals {
    return None;  // or skip rendering
}
```

**b)** The `services.is_empty()` early return — `render_service_panel` may return early when no services exist. Find this check and extend it to also check hook terminals:

```rust
if services.is_empty() && !has_hook_terminals {
    // ... existing early return logic
}
```

This ensures the panel renders when there are hook terminals even if no services are configured.

- [ ] **Step 6: Verify compilation**

Run: `cargo check`
Expected: compiles

- [ ] **Step 7: Manual testing**

1. Configure a hook in settings.json: `"on_project_open": "echo hello && sleep 2"`
2. Add a project — should see a hook tab appear in the service panel
3. After 2s (command finishes + 2s delay), the tab should auto-dismiss
4. Configure a failing hook: `"on_project_open": "exit 1"`
5. Add a project — should see a hook tab appear and stay (red dot)
6. Click the × on the tab to dismiss

- [ ] **Step 8: Commit**

```bash
git add src/views/panels/project_column.rs
git commit -m "feat: render hook terminal tabs in service panel with auto-dismiss"
```

---

### Task 10: Block layout manipulation of hook terminals

**Files:**
- Modify: `src/views/layout/layout_container.rs` (or wherever service terminals are blocked)

- [ ] **Step 1: Find the service terminal block**

Search for `service_terminals` in the layout code. The spec mentioned line 606-608 blocks service terminals from being moved. Add the same check for hook terminals:

```rust
// Block hook terminals from being moved (same as service terminals)
if project.hook_terminals.contains_key(terminal_id) {
    return;
}
```

- [ ] **Step 2: Verify compilation**

Run: `cargo check`
Expected: compiles

- [ ] **Step 3: Commit**

```bash
git add src/views/layout/
git commit -m "feat: prevent hook terminals from being moved in layout"
```

---

## Chunk 5: Final Integration & Cleanup

### Task 11: Update hook log overlay to link to terminals

**Files:**
- Modify: `src/views/overlays/hook_log.rs`

- [ ] **Step 1: Show terminal indicator for PTY-backed hooks**

In the hook execution row rendering, when `entry.terminal_id.is_some()`, add a small "view" indicator or icon. This is a minor UI enhancement — the main value is already delivered by the service panel tabs.

Add after the command row:

```rust
.when(entry.terminal_id.is_some(), |d| {
    d.child(
        div()
            .text_size(px(10.0))
            .text_color(rgb(t.text_muted))
            .child("(visible in panel)")
    )
})
```

- [ ] **Step 2: Verify compilation**

Run: `cargo check`
Expected: compiles

- [ ] **Step 3: Commit**

```bash
git add src/views/overlays/hook_log.rs
git commit -m "feat: show terminal indicator in hook log overlay"
```

---

### Task 12: Final integration test and cleanup

**Files:**
- All modified files

- [ ] **Step 1: Run full test suite**

Run: `cargo test`
Expected: all tests pass

- [ ] **Step 2: Run clippy**

Run: `cargo clippy -- -W clippy::all`
Expected: no new warnings in modified files

- [ ] **Step 3: Manual end-to-end test**

1. Set `on_project_open` hook to `echo "Hook started" && sleep 3 && echo "Done"`
2. Add a project
3. Verify: hook tab appears in service panel with yellow dot
4. Click the tab — terminal output visible
5. After command finishes, tab shows green briefly then auto-dismisses after 2s
6. Set `on_worktree_create` hook to `exit 1`
7. Create a worktree
8. Verify: hook tab appears with red dot, panel auto-opens
9. Click × to dismiss
10. Check hook log overlay — entries show "(visible in panel)"

- [ ] **Step 4: Final commit**

```bash
git add -A
git commit -m "feat: hook terminals — run hooks through PTY with service panel integration"
```
