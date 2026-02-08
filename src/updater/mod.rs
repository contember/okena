pub mod checker;
pub mod downloader;
pub mod installer;

use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

/// Status of the update process.
#[derive(Clone, Debug)]
pub enum UpdateStatus {
    Idle,
    Checking,
    #[allow(dead_code)]
    Available {
        version: String,
        asset_url: String,
        asset_name: String,
    },
    Downloading {
        version: String,
        progress: u8,
    },
    Ready {
        version: String,
        path: std::path::PathBuf,
    },
    Installing {
        version: String,
    },
    ReadyToRestart {
        version: String,
    },
    BrewUpdate {
        version: String,
    },
    Failed {
        error: String,
    },
}

struct UpdateInfoInner {
    status: UpdateStatus,
    dismissed: bool,
    is_homebrew: bool,
    /// Guards against concurrent manual check-for-updates operations.
    manual_check_active: bool,
}

/// Thread-safe shared update state, readable from any thread/view.
#[derive(Clone)]
pub struct UpdateInfo {
    inner: Arc<Mutex<UpdateInfoInner>>,
    /// Set to `true` when a checker loop is active; prevents duplicate loops.
    running: Arc<AtomicBool>,
    /// Monotonically increasing token; incremented on cancel so stale loops
    /// can detect they've been superseded.
    cancel_token: Arc<AtomicU64>,
}

impl UpdateInfo {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(UpdateInfoInner {
                status: UpdateStatus::Idle,
                dismissed: false,
                is_homebrew: is_homebrew_install(),
                manual_check_active: false,
            })),
            running: Arc::new(AtomicBool::new(false)),
            cancel_token: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn status(&self) -> UpdateStatus {
        self.inner.lock().status.clone()
    }

    pub fn set_status(&self, status: UpdateStatus) {
        let mut inner = self.inner.lock();
        // Only reset dismissed for states the user should notice
        if matches!(
            status,
            UpdateStatus::Available { .. }
                | UpdateStatus::Downloading { .. }
                | UpdateStatus::Ready { .. }
                | UpdateStatus::Installing { .. }
                | UpdateStatus::ReadyToRestart { .. }
                | UpdateStatus::BrewUpdate { .. }
                | UpdateStatus::Failed { .. }
        ) {
            inner.dismissed = false;
        }
        inner.status = status;
    }

    pub fn is_homebrew(&self) -> bool {
        self.inner.lock().is_homebrew
    }

    pub fn is_dismissed(&self) -> bool {
        self.inner.lock().dismissed
    }

    pub fn dismiss(&self) {
        self.inner.lock().dismissed = true;
    }

    /// Try to claim a manual (one-shot) check. Returns `false` if one is already
    /// active or if the auto-checker is currently checking/downloading.
    pub fn try_start_manual(&self) -> bool {
        let mut inner = self.inner.lock();
        if inner.manual_check_active {
            return false;
        }
        // Don't start a manual check while the auto-checker is actively working
        if matches!(
            inner.status,
            UpdateStatus::Checking | UpdateStatus::Downloading { .. }
        ) {
            return false;
        }
        inner.manual_check_active = true;
        inner.dismissed = false;
        true
    }

    /// Check whether a manual check is currently in progress.
    pub fn is_manual_active(&self) -> bool {
        self.inner.lock().manual_check_active
    }

    /// Release the manual check guard.
    pub fn finish_manual(&self) {
        self.inner.lock().manual_check_active = false;
    }

    /// Try to claim the checker loop. Returns the cancel token on success,
    /// `None` if a loop is already running or a manual check is active.
    pub fn try_start(&self) -> Option<u64> {
        if self.inner.lock().manual_check_active {
            return None;
        }
        if self
            .running
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            Some(self.cancel_token.load(Ordering::SeqCst))
        } else {
            None
        }
    }

    /// Signal the running checker loop to stop and allow a new one to start
    /// immediately. Increments the cancel token so the old loop detects it
    /// has been superseded, and clears `running` so `try_start` can succeed.
    pub fn cancel(&self) {
        self.cancel_token.fetch_add(1, Ordering::SeqCst);
        self.running.store(false, Ordering::SeqCst);
    }

    /// Check whether this loop's token is still current.
    pub fn is_cancelled(&self, token: u64) -> bool {
        self.cancel_token.load(Ordering::SeqCst) != token
    }

    /// Return the current cancel token (for one-shot operations).
    pub fn current_token(&self) -> u64 {
        self.cancel_token.load(Ordering::SeqCst)
    }

    /// Mark the checker loop as no longer running, but only if the token is
    /// still current (prevents a stale loop from clearing state for a new one).
    pub fn mark_stopped(&self, token: u64) {
        if self.cancel_token.load(Ordering::SeqCst) == token {
            self.running.store(false, Ordering::SeqCst);
        }
    }
}

/// Detect if running from a Homebrew installation.
pub fn is_homebrew_install() -> bool {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.canonicalize().ok())
        .map(|p| {
            let s = p.to_string_lossy();
            s.contains("/Caskroom/") || s.contains("/Cellar/")
        })
        .unwrap_or(false)
}

/// GPUI global wrapper for UpdateInfo.
#[derive(Clone)]
pub struct GlobalUpdateInfo(pub UpdateInfo);

impl gpui::Global for GlobalUpdateInfo {}
