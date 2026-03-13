use parking_lot::Mutex;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::views::panels::toast::Toast;

/// Maximum number of hook executions to keep in history.
const MAX_HISTORY: usize = 50;

/// Status of a hook execution.
#[derive(Debug, Clone)]
pub enum HookStatus {
    Running,
    Succeeded { duration: Duration },
    Failed { duration: Duration, exit_code: i32, stderr: String },
    SpawnError { message: String },
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

/// Internal mutable state behind the Arc<Mutex<...>>.
struct HookMonitorInner {
    history: VecDeque<HookExecution>,
    pending_toasts: Vec<Toast>,
    next_id: u64,
    running_count: usize,
    exit_waiters: HashMap<String, mpsc::Sender<Option<u32>>>,
}

/// Thread-safe hook execution monitor.
///
/// Follows the same `Arc<Mutex<...>>` + `impl Global` pattern as `ToastManager`.
/// Hook threads write start/finish events; the UI thread drains pending toasts.
#[derive(Clone)]
pub struct HookMonitor(Arc<Mutex<HookMonitorInner>>);

impl gpui::Global for HookMonitor {}

impl HookMonitor {
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(HookMonitorInner {
            history: VecDeque::new(),
            pending_toasts: Vec::new(),
            next_id: 1,
            running_count: 0,
            exit_waiters: HashMap::new(),
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
            if let Some(entry) = removed {
                if matches!(entry.status, HookStatus::Running) {
                    inner.running_count = inner.running_count.saturating_sub(1);
                }
            }
        }

        id
    }

    /// Record hook completion (success, failure, or spawn error).
    pub fn record_finish(&self, id: u64, status: HookStatus) {
        let mut inner = self.0.lock();
        inner.running_count = inner.running_count.saturating_sub(1);

        // Find the entry's hook_type first (Copy-friendly &'static str)
        let hook_type = inner.history.iter().find(|e| e.id == id).map(|e| e.hook_type);

        // Queue a toast on failure (before mutably borrowing history)
        if let Some(hook_type) = hook_type {
            match &status {
                HookStatus::Failed { stderr, .. } => {
                    let first_line = stderr.lines().next().unwrap_or("(no output)");
                    let msg = format!(
                        "Hook `{}` failed: {}",
                        hook_type,
                        truncate(first_line, 120),
                    );
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
        }

        // Now update the entry status
        if let Some(entry) = inner.history.iter_mut().find(|e| e.id == id) {
            entry.status = status;
        }
    }

    /// Drain pending toast notifications (called by UI thread).
    pub fn drain_pending_toasts(&self) -> Vec<Toast> {
        let mut inner = self.0.lock();
        std::mem::take(&mut inner.pending_toasts)
    }

    /// Get a snapshot of the execution history (newest first).
    pub fn history(&self) -> Vec<HookExecution> {
        let inner = self.0.lock();
        inner.history.iter().rev().cloned().collect()
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
        inner.exit_waiters.insert(terminal_id.to_string(), tx);
        rx
    }

    /// Find and finish a hook execution by its terminal ID.
    /// Returns `true` if a matching running execution was found and finished.
    pub fn finish_by_terminal_id(&self, terminal_id: &str, exit_code: Option<u32>) -> bool {
        let mut inner = self.0.lock();
        if let Some(entry) = inner.history.iter_mut().find(|e| {
            e.terminal_id.as_deref() == Some(terminal_id)
                && matches!(e.status, HookStatus::Running)
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
        }
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

        monitor.record_finish(id, HookStatus::Succeeded {
            duration: Duration::from_millis(50),
        });
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

        monitor.record_finish(id, HookStatus::Failed {
            duration: Duration::from_millis(10),
            exit_code: 1,
            stderr: "something went wrong".to_string(),
        });

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
            monitor.record_finish(id, HookStatus::Succeeded {
                duration: Duration::from_millis(1),
            });
        }
        assert!(monitor.history().len() <= 50);
    }

    #[test]
    fn history_returned_newest_first() {
        let monitor = HookMonitor::new();
        let id1 = monitor.record_start("first", "echo 1", "proj", None);
        monitor.record_finish(id1, HookStatus::Succeeded { duration: Duration::from_millis(1) });
        let id2 = monitor.record_start("second", "echo 2", "proj", None);
        monitor.record_finish(id2, HookStatus::Succeeded { duration: Duration::from_millis(1) });

        let history = monitor.history();
        assert_eq!(history[0].hook_type, "second");
        assert_eq!(history[1].hook_type, "first");
    }

    #[test]
    fn spawn_error_queues_toast() {
        let monitor = HookMonitor::new();
        let id = monitor.record_start("on_project_open", "bad-cmd", "proj", None);
        monitor.record_finish(id, HookStatus::SpawnError {
            message: "command not found".to_string(),
        });

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
    fn notify_exit_without_waiter_is_noop() {
        let monitor = HookMonitor::new();
        // Should not panic
        monitor.notify_exit("nonexistent", Some(1));
    }
}
