use std::sync::atomic::Ordering;
use std::time::Instant;

use super::Terminal;
use super::child_processes::has_child_processes;

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

    /// Returns true if the shell currently has a child process running.
    /// Performs a synchronous, low-overhead check (direct `/proc` read on Linux,
    /// `pgrep -P` fallback elsewhere) and is safe to call from UI event handlers.
    ///
    /// Note: `shell_pid` is expected to be the *real* shell pid, not a session
    /// proxy (dtach / tmux attach client). Session-backend resolution is done
    /// when the terminal is created (see `TerminalBackend::get_foreground_shell_pid`).
    pub fn has_running_child(&self) -> bool {
        match *self.shell_pid.lock() {
            Some(pid) => has_child_processes(pid),
            None => false,
        }
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
