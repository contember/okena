use okena_core::api::{ApiHookExecution, ApiHookStatus};
use okena_state::Toast;
use parking_lot::Mutex;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Maximum number of hook executions to keep in history.
const MAX_HISTORY: usize = 50;

/// Exit events can beat sync-hook waiter registration immediately after PTY spawn.
const MAX_EARLY_EXITS: usize = 128;
const EARLY_EXIT_TTL: Duration = Duration::from_secs(30);

/// Status of a hook execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookStatus {
    Running,
    Succeeded {
        duration: Duration,
    },
    Failed {
        duration: Duration,
        exit_code: i32,
        stderr: String,
    },
    SpawnError {
        message: String,
    },
}

/// A single hook execution record.
#[derive(Debug, Clone)]
pub struct HookExecution {
    pub id: u64,
    pub hook_type: &'static str,
    pub command: String,
    pub project_name: String,
    pub started_at: Instant,
    pub status: HookStatus,
    pub terminal_id: Option<String>,
}

/// Intern a wire hook-type string back to the `&'static str` the domain type
/// uses. The set is closed (every `hook_type` passed to `record_start` is a
/// literal); an unrecognized value maps to `"unknown"` rather than leaking.
fn intern_hook_type(s: &str) -> &'static str {
    match s {
        "on_project_open" => "on_project_open",
        "on_project_close" => "on_project_close",
        "on_worktree_create" => "on_worktree_create",
        "on_worktree_close" => "on_worktree_close",
        "pre_merge" => "pre_merge",
        "post_merge" => "post_merge",
        "before_worktree_remove" => "before_worktree_remove",
        "worktree_removed" => "worktree_removed",
        "on_rebase_conflict" => "on_rebase_conflict",
        "on_dirty_worktree_close" => "on_dirty_worktree_close",
        "terminal.on_close" => "terminal.on_close",
        _ => "unknown",
    }
}

fn same_snapshot_execution(a: &HookExecution, b: &HookExecution) -> bool {
    a.id == b.id
        && a.hook_type == b.hook_type
        && a.command == b.command
        && a.project_name == b.project_name
        && a.status == b.status
        && a.terminal_id == b.terminal_id
}

impl HookExecution {
    /// Project onto the wire mirror ([`ApiHookExecution`]). Durations collapse
    /// to whole milliseconds; `started_at` is dropped (process-local `Instant`).
    pub fn to_api(&self) -> ApiHookExecution {
        let status = match &self.status {
            HookStatus::Running => ApiHookStatus::Running,
            HookStatus::Succeeded { duration } => ApiHookStatus::Succeeded {
                duration_ms: duration.as_millis() as u64,
            },
            HookStatus::Failed {
                duration,
                exit_code,
                stderr,
            } => ApiHookStatus::Failed {
                duration_ms: duration.as_millis() as u64,
                exit_code: *exit_code,
                stderr: stderr.clone(),
            },
            HookStatus::SpawnError { message } => ApiHookStatus::SpawnError {
                message: message.clone(),
            },
        };
        ApiHookExecution {
            id: self.id,
            hook_type: self.hook_type.to_string(),
            command: self.command.clone(),
            project_name: self.project_name.clone(),
            status,
            terminal_id: self.terminal_id.clone(),
        }
    }

    /// Rebuild from the wire mirror on a thin client. `started_at` is stamped
    /// to now (see [`ApiHookExecution`]); `hook_type` is re-interned.
    pub fn from_api(api: &ApiHookExecution) -> Self {
        let status = match &api.status {
            ApiHookStatus::Running => HookStatus::Running,
            ApiHookStatus::Succeeded { duration_ms } => HookStatus::Succeeded {
                duration: Duration::from_millis(*duration_ms),
            },
            ApiHookStatus::Failed {
                duration_ms,
                exit_code,
                stderr,
            } => HookStatus::Failed {
                duration: Duration::from_millis(*duration_ms),
                exit_code: *exit_code,
                stderr: stderr.clone(),
            },
            ApiHookStatus::SpawnError { message } => HookStatus::SpawnError {
                message: message.clone(),
            },
        };
        HookExecution {
            id: api.id,
            hook_type: intern_hook_type(&api.hook_type),
            command: api.command.clone(),
            project_name: api.project_name.clone(),
            started_at: Instant::now(),
            status,
            terminal_id: api.terminal_id.clone(),
        }
    }
}

/// Internal mutable state behind the Arc<Mutex<...>>.
struct HookMonitorInner {
    history: VecDeque<HookExecution>,
    pending_toasts: Vec<Toast>,
    next_id: u64,
    running_count: usize,
    exit_waiters: HashMap<String, mpsc::Sender<Option<u32>>>,
    early_exits: VecDeque<(String, Option<u32>, Instant)>,
    /// Monotonic counter incremented on every mutation. Allows cheap
    /// "has anything changed?" checks without cloning the full history.
    version: u64,
}

/// Thread-safe hook execution monitor.
///
/// Follows the same `Arc<Mutex<...>>` + `impl Global` pattern as `ToastManager`.
/// Hook threads write start/finish events; the UI thread drains pending toasts.
#[derive(Clone)]
pub struct HookMonitor(Arc<Mutex<HookMonitorInner>>);

#[cfg(feature = "gpui")]
impl gpui::Global for HookMonitor {}

impl Default for HookMonitor {
    fn default() -> Self {
        Self::new()
    }
}

impl HookMonitor {
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(HookMonitorInner {
            history: VecDeque::new(),
            pending_toasts: Vec::new(),
            next_id: 1,
            running_count: 0,
            exit_waiters: HashMap::new(),
            early_exits: VecDeque::new(),
            version: 0,
        })))
    }

    /// Record a hook execution start. Returns an ID to use with `record_finish`.
    pub fn record_start(
        &self,
        hook_type: &'static str,
        command: &str,
        project_name: &str,
        terminal_id: Option<String>,
    ) -> u64 {
        let mut inner = self.0.lock();
        let id = inner.next_id;
        inner.next_id += 1;
        inner.running_count += 1;
        inner.version += 1;

        inner.history.push_back(HookExecution {
            id,
            hook_type,
            command: command.to_string(),
            project_name: project_name.to_string(),
            started_at: Instant::now(),
            status: HookStatus::Running,
            terminal_id,
        });

        // Cap history
        while inner.history.len() > MAX_HISTORY {
            let removed = inner.history.pop_front();
            // If we're removing a still-running entry (shouldn't normally happen), adjust count
            if let Some(entry) = removed
                && matches!(entry.status, HookStatus::Running)
            {
                inner.running_count = inner.running_count.saturating_sub(1);
            }
        }

        id
    }

    /// Record a hook execution whose type came from persisted/wire state.
    pub fn record_start_named(
        &self,
        hook_type: &str,
        command: &str,
        project_name: &str,
        terminal_id: Option<String>,
    ) -> u64 {
        self.record_start(
            intern_hook_type(hook_type),
            command,
            project_name,
            terminal_id,
        )
    }

    /// Record hook completion (success, failure, or spawn error).
    pub fn record_finish(&self, id: u64, status: HookStatus) {
        let mut inner = self.0.lock();
        inner.running_count = inner.running_count.saturating_sub(1);
        inner.version += 1;

        // Single-pass: find by position, then use index for both toast and status update.
        if let Some(idx) = inner.history.iter().position(|e| e.id == id) {
            let hook_type = inner.history[idx].hook_type;

            // Queue a toast on failure
            match &status {
                HookStatus::Failed { stderr, .. } => {
                    let first_line = stderr.lines().next().unwrap_or("(no output)");
                    let msg =
                        format!("Hook `{}` failed: {}", hook_type, truncate(first_line, 120),);
                    inner.pending_toasts.push(Toast::error(msg));
                }
                HookStatus::SpawnError { message } => {
                    let msg = format!(
                        "Hook `{}` could not start: {}",
                        hook_type,
                        truncate(message, 120),
                    );
                    inner.pending_toasts.push(Toast::error(msg));
                }
                _ => {}
            }

            inner.history[idx].status = status;
        }
    }

    /// Drain pending toast notifications (called by UI thread).
    pub fn drain_pending_toasts(&self) -> Vec<Toast> {
        let mut inner = self.0.lock();
        std::mem::take(&mut inner.pending_toasts)
    }

    /// Enqueue an arbitrary toast for the daemon's toast-forward loop to
    /// broadcast to clients. Used by daemon flows (e.g. soft-close) that must
    /// surface a toast but aren't hook-driven.
    pub fn push_toast(&self, toast: Toast) {
        let mut inner = self.0.lock();
        inner.pending_toasts.push(toast);
    }

    /// Replace the entire history from a daemon snapshot (thin-client ingest).
    ///
    /// `newest_first` is in the same order [`history`](Self::history) returns
    /// (newest first) — the daemon builds it from its own `history()`. To keep
    /// the log rendering unchanged, it is stored reversed (oldest-first) so
    /// `history()`'s own `.rev()` yields newest-first again.
    ///
    /// A full wire-visible comparison skips unchanged snapshots. `started_at` is
    /// intentionally ignored because it is reconstructed locally on each ingest.
    pub fn replace_history(&self, newest_first: Vec<HookExecution>) {
        let mut inner = self.0.lock();
        let unchanged = inner.history.len() == newest_first.len()
            && inner
                .history
                .iter()
                .rev()
                .zip(newest_first.iter())
                .all(|(a, b)| same_snapshot_execution(a, b));
        if unchanged {
            return;
        }
        inner.running_count = newest_first
            .iter()
            .filter(|e| matches!(e.status, HookStatus::Running))
            .count();
        inner.history = newest_first.into_iter().rev().collect();
        inner.version += 1;
    }

    /// Get a snapshot of the execution history (newest first).
    pub fn history(&self) -> Vec<HookExecution> {
        let inner = self.0.lock();
        inner.history.iter().rev().cloned().collect()
    }

    /// Monotonic version counter — incremented on every mutation.
    /// Allows cheap change detection without cloning history.
    pub fn version(&self) -> u64 {
        self.0.lock().version
    }

    /// Number of currently running hooks.
    #[cfg(test)]
    pub fn running_count(&self) -> usize {
        self.0.lock().running_count
    }

    /// Register a waiter for a terminal's exit event. Returns a receiver that
    /// blocks until the PTY exits. Used by sync hooks.
    pub fn register_exit_waiter(&self, terminal_id: &str) -> mpsc::Receiver<Option<u32>> {
        let (tx, rx) = mpsc::channel();
        let mut inner = self.0.lock();
        prune_early_exits(&mut inner.early_exits);
        if let Some(index) = inner
            .early_exits
            .iter()
            .position(|(id, _, _)| id == terminal_id)
        {
            if let Some((_, exit_code, _)) = inner.early_exits.remove(index) {
                let _ = tx.send(exit_code);
            }
        } else {
            inner.exit_waiters.insert(terminal_id.to_string(), tx);
        }
        rx
    }

    /// Find and finish a hook execution by its terminal ID.
    /// Returns `true` if a matching running execution was found and finished.
    pub fn finish_by_terminal_id(&self, terminal_id: &str, exit_code: Option<u32>) -> bool {
        let mut inner = self.0.lock();
        if let Some(entry) = inner.history.iter_mut().find(|e| {
            e.terminal_id.as_deref() == Some(terminal_id) && matches!(e.status, HookStatus::Running)
        }) {
            let duration = entry.started_at.elapsed();
            let success = exit_code == Some(0);
            if success {
                entry.status = HookStatus::Succeeded { duration };
            } else {
                let code = exit_code.map(|c| c as i32).unwrap_or(-1);
                entry.status = HookStatus::Failed {
                    duration,
                    exit_code: code,
                    stderr: String::new(),
                };
                let msg = format!("Hook `{}` failed (exit code {})", entry.hook_type, code);
                inner.pending_toasts.push(Toast::error(msg));
            }
            inner.running_count = inner.running_count.saturating_sub(1);
            inner.version += 1;
            true
        } else {
            false
        }
    }

    /// Notify that a hook terminal has exited. Sends exit code through the
    /// waiter channel (if any) and removes the waiter.
    pub fn notify_exit(&self, terminal_id: &str, exit_code: Option<u32>) {
        let mut inner = self.0.lock();
        if let Some(tx) = inner.exit_waiters.remove(terminal_id) {
            let _ = tx.send(exit_code);
        } else {
            prune_early_exits(&mut inner.early_exits);
            inner.early_exits.retain(|(id, _, _)| id != terminal_id);
            inner
                .early_exits
                .push_back((terminal_id.to_string(), exit_code, Instant::now()));
            while inner.early_exits.len() > MAX_EARLY_EXITS {
                inner.early_exits.pop_front();
            }
        }
    }
}

fn prune_early_exits(early_exits: &mut VecDeque<(String, Option<u32>, Instant)>) {
    while early_exits
        .front()
        .is_some_and(|(_, _, received_at)| received_at.elapsed() > EARLY_EXIT_TTL)
    {
        early_exits.pop_front();
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let end = s.floor_char_boundary(max);
        format!("{}...", &s[..end])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_start_and_finish_success() {
        let monitor = HookMonitor::new();
        let id = monitor.record_start("on_project_open", "echo hi", "my-project", None);
        assert_eq!(monitor.running_count(), 1);

        monitor.record_finish(
            id,
            HookStatus::Succeeded {
                duration: Duration::from_millis(50),
            },
        );
        assert_eq!(monitor.running_count(), 0);

        let history = monitor.history();
        assert_eq!(history.len(), 1);
        assert!(matches!(history[0].status, HookStatus::Succeeded { .. }));
        assert!(monitor.drain_pending_toasts().is_empty());
    }

    #[test]
    fn record_failure_queues_toast() {
        let monitor = HookMonitor::new();
        let id = monitor.record_start("pre_merge", "exit 1", "test-project", None);

        monitor.record_finish(
            id,
            HookStatus::Failed {
                duration: Duration::from_millis(10),
                exit_code: 1,
                stderr: "something went wrong".to_string(),
            },
        );

        let toasts = monitor.drain_pending_toasts();
        assert_eq!(toasts.len(), 1);
        assert!(toasts[0].message.contains("pre_merge"));
        assert!(toasts[0].message.contains("something went wrong"));
    }

    #[test]
    fn history_capped_at_max() {
        let monitor = HookMonitor::new();
        for i in 0..60 {
            let id = monitor.record_start("test", &format!("cmd-{}", i), "proj", None);
            monitor.record_finish(
                id,
                HookStatus::Succeeded {
                    duration: Duration::from_millis(1),
                },
            );
        }
        assert!(monitor.history().len() <= 50);
    }

    #[test]
    fn history_returned_newest_first() {
        let monitor = HookMonitor::new();
        let id1 = monitor.record_start("first", "echo 1", "proj", None);
        monitor.record_finish(
            id1,
            HookStatus::Succeeded {
                duration: Duration::from_millis(1),
            },
        );
        let id2 = monitor.record_start("second", "echo 2", "proj", None);
        monitor.record_finish(
            id2,
            HookStatus::Succeeded {
                duration: Duration::from_millis(1),
            },
        );

        let history = monitor.history();
        assert_eq!(history[0].hook_type, "second");
        assert_eq!(history[1].hook_type, "first");
    }

    #[test]
    fn spawn_error_queues_toast() {
        let monitor = HookMonitor::new();
        let id = monitor.record_start("on_project_open", "bad-cmd", "proj", None);
        monitor.record_finish(
            id,
            HookStatus::SpawnError {
                message: "command not found".to_string(),
            },
        );

        let toasts = monitor.drain_pending_toasts();
        assert_eq!(toasts.len(), 1);
        assert!(toasts[0].message.contains("could not start"));
    }

    #[test]
    fn exit_waiter_receives_exit_code() {
        let monitor = HookMonitor::new();
        let rx = monitor.register_exit_waiter("term-1");
        monitor.notify_exit("term-1", Some(0));
        assert_eq!(rx.recv().unwrap(), Some(0));
    }

    #[test]
    fn exit_waiter_receives_none_on_signal_kill() {
        let monitor = HookMonitor::new();
        let rx = monitor.register_exit_waiter("term-2");
        monitor.notify_exit("term-2", None);
        assert_eq!(rx.recv().unwrap(), None);
    }

    #[test]
    fn exit_waiter_receives_exit_that_arrived_before_registration() {
        let monitor = HookMonitor::new();
        monitor.notify_exit("term-early", Some(7));

        let rx = monitor.register_exit_waiter("term-early");

        assert_eq!(rx.try_recv().unwrap(), Some(7));
    }

    #[test]
    fn early_exit_can_finish_execution_recorded_after_the_event() {
        let monitor = HookMonitor::new();
        monitor.notify_exit("term-fast", Some(0));
        monitor.record_start("pre_merge", "true", "project", Some("term-fast".into()));

        let rx = monitor.register_exit_waiter("term-fast");
        let exit_code = rx.try_recv().unwrap();
        assert!(monitor.finish_by_terminal_id("term-fast", exit_code));

        assert!(matches!(
            monitor.history()[0].status,
            HookStatus::Succeeded { .. }
        ));
    }

    #[test]
    fn early_exit_buffer_is_bounded() {
        let monitor = HookMonitor::new();
        for index in 0..=MAX_EARLY_EXITS {
            monitor.notify_exit(&format!("term-{index}"), Some(0));
        }

        let evicted = monitor.register_exit_waiter("term-0");
        let newest = monitor.register_exit_waiter(&format!("term-{MAX_EARLY_EXITS}"));

        assert!(matches!(evicted.try_recv(), Err(mpsc::TryRecvError::Empty)));
        assert_eq!(newest.try_recv().unwrap(), Some(0));
    }

    #[test]
    fn record_start_named_interns_persisted_hook_type() {
        let monitor = HookMonitor::new();
        monitor.record_start_named(
            "on_project_open",
            "echo rerun",
            "project",
            Some("term-rerun".into()),
        );

        let history = monitor.history();
        assert_eq!(history[0].hook_type, "on_project_open");
        assert_eq!(history[0].terminal_id.as_deref(), Some("term-rerun"));
    }

    #[test]
    fn notify_exit_without_waiter_is_noop() {
        let monitor = HookMonitor::new();
        // Should not panic
        monitor.notify_exit("nonexistent", Some(1));
    }

    #[test]
    fn to_api_from_api_round_trips() {
        let exec = HookExecution {
            id: 7,
            hook_type: "worktree_removed",
            command: "echo bye".into(),
            project_name: "proj".into(),
            started_at: Instant::now(),
            status: HookStatus::Failed {
                duration: Duration::from_millis(1234),
                exit_code: 3,
                stderr: "boom".into(),
            },
            terminal_id: Some("t9".into()),
        };
        let api = exec.to_api();
        assert_eq!(api.hook_type, "worktree_removed");
        assert!(matches!(
            api.status,
            ApiHookStatus::Failed {
                duration_ms: 1234,
                exit_code: 3,
                ..
            }
        ));

        let back = HookExecution::from_api(&api);
        assert_eq!(back.id, 7);
        // hook_type re-interned to the same &'static str.
        assert_eq!(back.hook_type, "worktree_removed");
        assert_eq!(back.command, "echo bye");
        assert_eq!(back.terminal_id.as_deref(), Some("t9"));
        match back.status {
            HookStatus::Failed {
                duration,
                exit_code,
                ref stderr,
            } => {
                assert_eq!(duration, Duration::from_millis(1234));
                assert_eq!(exit_code, 3);
                assert_eq!(stderr, "boom");
            }
            _ => panic!("status kind not preserved"),
        }
    }

    #[test]
    fn from_api_interns_every_recorded_hook_type() {
        // Every literal passed to a `run_hook*` -> `record_start` call site in
        // hooks.rs must round-trip through interning without falling to
        // "unknown"; a thin client re-interns mirrored history and would
        // otherwise mislabel these in the Hook Log.
        for ty in [
            "on_project_open",
            "on_project_close",
            "on_worktree_create",
            "on_worktree_close",
            "pre_merge",
            "post_merge",
            "before_worktree_remove",
            "worktree_removed",
            "on_rebase_conflict",
            "on_dirty_worktree_close",
            "terminal.on_close",
        ] {
            let api = ApiHookExecution {
                id: 1,
                hook_type: ty.into(),
                command: "x".into(),
                project_name: "p".into(),
                status: ApiHookStatus::Running,
                terminal_id: None,
            };
            assert_eq!(
                HookExecution::from_api(&api).hook_type,
                ty,
                "hook type {ty:?} must intern to itself, not \"unknown\""
            );
        }
    }

    #[test]
    fn from_api_interns_unknown_hook_type() {
        let api = ApiHookExecution {
            id: 1,
            hook_type: "totally_made_up".into(),
            command: "x".into(),
            project_name: "p".into(),
            status: ApiHookStatus::Running,
            terminal_id: None,
        };
        assert_eq!(HookExecution::from_api(&api).hook_type, "unknown");
    }

    fn api_exec(id: u64, status: ApiHookStatus) -> HookExecution {
        HookExecution::from_api(&ApiHookExecution {
            id,
            hook_type: "pre_merge".into(),
            command: format!("cmd-{id}"),
            project_name: "p".into(),
            status,
            terminal_id: None,
        })
    }

    #[test]
    fn replace_history_preserves_newest_first_and_bumps_version() {
        let monitor = HookMonitor::new();
        let v0 = monitor.version();
        // Daemon `history()` yields newest-first: id 2 then id 1.
        monitor.replace_history(vec![
            api_exec(2, ApiHookStatus::Running),
            api_exec(1, ApiHookStatus::Succeeded { duration_ms: 5 }),
        ]);

        let hist = monitor.history();
        assert_eq!(hist.len(), 2);
        assert_eq!(hist[0].id, 2, "newest-first order must survive ingest");
        assert_eq!(hist[1].id, 1);
        assert!(monitor.version() > v0);
        assert_eq!(monitor.running_count(), 1);
    }

    #[test]
    fn replace_history_skips_bump_when_unchanged() {
        let monitor = HookMonitor::new();
        let snapshot = || vec![api_exec(1, ApiHookStatus::Succeeded { duration_ms: 5 })];
        monitor.replace_history(snapshot());
        let v = monitor.version();
        // Same wire-visible data → no write, no version bump.
        monitor.replace_history(snapshot());
        assert_eq!(monitor.version(), v);
    }

    #[test]
    fn replace_history_bumps_when_status_kind_changes() {
        let monitor = HookMonitor::new();
        monitor.replace_history(vec![api_exec(1, ApiHookStatus::Running)]);
        let v = monitor.version();
        // Same id, different status kind (Running → Succeeded) → must bump.
        monitor.replace_history(vec![api_exec(
            1,
            ApiHookStatus::Succeeded { duration_ms: 9 },
        )]);
        assert!(monitor.version() > v);
    }

    #[test]
    fn replace_history_replaces_reused_ids_after_daemon_restart() {
        let monitor = HookMonitor::new();
        monitor.replace_history(vec![api_exec(
            1,
            ApiHookStatus::Succeeded { duration_ms: 5 },
        )]);
        let v = monitor.version();

        let replacement = HookExecution::from_api(&ApiHookExecution {
            id: 1,
            hook_type: "pre_merge".into(),
            command: "new daemon command".into(),
            project_name: "new-project".into(),
            status: ApiHookStatus::Succeeded { duration_ms: 9 },
            terminal_id: Some("new-terminal".into()),
        });
        monitor.replace_history(vec![replacement]);

        assert!(monitor.version() > v);
        let history = monitor.history();
        assert_eq!(history[0].command, "new daemon command");
        assert_eq!(history[0].project_name, "new-project");
        assert!(matches!(
            history[0].status,
            HookStatus::Succeeded { duration } if duration == Duration::from_millis(9)
        ));
        assert_eq!(history[0].terminal_id.as_deref(), Some("new-terminal"));
    }
}
