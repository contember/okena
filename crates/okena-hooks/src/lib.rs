//! okena-hooks — Lifecycle hook execution.
//!
//! Runs project/terminal/worktree lifecycle hooks as shell commands, either
//! through a PTY-backed terminal (when a `HookRunner` is registered as a
//! GPUI Global) or headlessly via `sh -c` / `cmd /C`. The `HookMonitor`
//! tracks in-flight and completed executions for the UI.
//!
//! This crate intentionally does not depend on `okena-workspace`: hook
//! callers pass project metadata in, and the result (`HookTerminalResult`)
//! flows back through a plain struct so the workspace can attach it to its
//! entity state.

pub mod hook_monitor;
pub mod hooks;

pub use hook_monitor::{HookExecution, HookMonitor, HookStatus};
pub use hooks::{
    HookRunner, HookTerminalResult,
    apply_shell_wrapper, fire_before_worktree_remove, fire_before_worktree_remove_async,
    fire_on_dirty_worktree_close, fire_on_project_close, fire_on_project_open,
    fire_on_rebase_conflict, fire_on_worktree_close, fire_on_worktree_create,
    fire_post_merge, fire_pre_merge, fire_terminal_on_close, fire_worktree_removed,
    resolve_terminal_on_create, terminal_hook_env, try_monitor, try_runner,
};
