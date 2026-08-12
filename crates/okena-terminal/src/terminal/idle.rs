use std::sync::atomic::Ordering;
use std::time::Instant;

use super::Terminal;
use super::child_processes::has_child_processes;

fn can_rewrite_shell_input(shell_pid: Option<u32>, has_children: impl FnOnce(u32) -> bool) -> bool {
    shell_pid.is_some_and(|pid| !has_children(pid))
}

impl Terminal {
    /// Set the shell process PID (for foreground process checking)
    pub fn set_shell_pid(&self, pid: u32) {
        *self.shell_pid.lock() = Some(pid);
    }

    /// Read the cached "waiting for input" state (cheap, no subprocess).
    /// This is safe to call from render paths. Updated by `update_waiting_state()`.
    pub fn is_waiting_for_input(&self) -> bool {
        self.waiting_for_input.load(Ordering::Relaxed)
    }

    /// Human-readable idle duration string (e.g., "5s", "2m", "1h").
    /// Shows time since the unseen output arrived.
    /// Only meaningful when `is_waiting_for_input()` is true.
    pub fn idle_duration_display(&self) -> String {
        let secs = self.last_viewed_time.lock().elapsed().as_secs();
        if secs < 60 {
            format!("{}s", secs)
        } else if secs < 3600 {
            format!("{}m", secs / 60)
        } else {
            format!("{}h", secs / 3600)
        }
    }

    /// Get the shell PID (for background thread to run pgrep off the main thread)
    pub fn shell_pid(&self) -> Option<u32> {
        *self.shell_pid.lock()
    }

    /// Get the last output time (for background thread idle check)
    pub fn last_output_time(&self) -> Instant {
        *self.last_output_time.lock()
    }

    /// Whether the user has ever sent input to this terminal
    pub fn had_user_input(&self) -> bool {
        self.had_user_input.load(Ordering::Relaxed)
    }

    /// Update the cached waiting state (called from background thread only)
    pub fn set_waiting_for_input(&self, waiting: bool) {
        self.waiting_for_input.store(waiting, Ordering::Relaxed);
    }

    /// Whether shell-only input rewriting is safe for this terminal.
    /// Performs a synchronous, low-overhead child check (direct `/proc` read on
    /// Linux, `pgrep -P` fallback elsewhere). An unknown shell PID is unsafe:
    /// remote mirrors cannot inspect the daemon's process tree.
    ///
    /// Note: `shell_pid` is expected to be the *real* shell pid, not a session
    /// proxy (dtach / tmux attach client). Session-backend resolution is done
    /// when the terminal is created (see `TerminalBackend::get_foreground_shell_pid`).
    pub fn can_rewrite_shell_input(&self) -> bool {
        can_rewrite_shell_input(*self.shell_pid.lock(), has_child_processes)
    }

    /// Reset the idle timer to now, clearing the waiting state.
    /// Called when the terminal receives focus so it won't immediately re-trigger.
    pub fn clear_waiting(&self) {
        self.waiting_for_input.store(false, Ordering::Relaxed);
        *self.last_output_time.lock() = Instant::now();
        *self.last_viewed_time.lock() = Instant::now();
    }

    /// Record that the user has seen this terminal's output (called on blur).
    /// After this, the terminal won't be flagged as waiting unless new output arrives.
    pub fn mark_as_viewed(&self) {
        *self.last_viewed_time.lock() = Instant::now();
    }

    /// Whether new output has arrived since the user last viewed this terminal.
    pub fn has_unseen_output(&self) -> bool {
        *self.last_output_time.lock() > *self.last_viewed_time.lock()
    }
}

#[cfg(test)]
mod tests {
    use super::can_rewrite_shell_input;

    #[test]
    fn unknown_shell_pid_blocks_input_rewriting() {
        assert!(!can_rewrite_shell_input(None, |_| {
            panic!("unknown PID must not inspect the client process tree")
        }));
    }

    #[test]
    fn known_plain_shell_allows_input_rewriting() {
        assert!(can_rewrite_shell_input(Some(42), |_| false));
    }

    #[test]
    fn known_shell_with_running_child_blocks_input_rewriting() {
        assert!(!can_rewrite_shell_input(Some(42), |_| true));
    }
}
