use std::path::{Path, PathBuf};

#[cfg(feature = "gpui-ui")]
use gpui::*;

/// A checkout-backed Okena executable that can rebuild itself with Cargo.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalCheckout {
    root: PathBuf,
    release_executable: PathBuf,
}

impl LocalCheckout {
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn release_executable(&self) -> &Path {
        &self.release_executable
    }
}

/// Detect a binary running from this source checkout's `target` directory.
pub fn detect_local_checkout() -> Option<LocalCheckout> {
    let executable = std::env::current_exe().ok()?;
    let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = crate_dir.parent()?.parent()?;
    detect_local_checkout_from(&executable, workspace_root)
}

fn detect_local_checkout_from(executable: &Path, workspace_root: &Path) -> Option<LocalCheckout> {
    let file_name = executable.file_name()?.to_str()?;
    let is_okena_binary = if cfg!(windows) {
        matches!(file_name, "okena.exe" | "okena-daemon.exe")
    } else {
        matches!(file_name, "okena" | "okena-daemon")
    };
    if !is_okena_binary {
        return None;
    }

    let target_dir = workspace_root.join("target");
    if !executable.starts_with(&target_dir) {
        return None;
    }
    if !workspace_root.join("Cargo.toml").is_file() || !workspace_root.join(".git").exists() {
        return None;
    }

    let release_name = if cfg!(windows) { "okena.exe" } else { "okena" };
    Some(LocalCheckout {
        root: workspace_root.to_path_buf(),
        release_executable: target_dir.join("release").join(release_name),
    })
}

#[cfg(feature = "gpui-ui")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LocalBuildStatus {
    Idle,
    Building,
    ReadyToRestart,
    RestartingDaemon,
    RestartingApp,
    Failed { error: String },
}

#[cfg(feature = "gpui-ui")]
pub struct LocalBuildState {
    checkout: LocalCheckout,
    status: LocalBuildStatus,
    daemon_ui_owned: Option<bool>,
}

#[cfg(feature = "gpui-ui")]
impl LocalBuildState {
    pub fn new(checkout: LocalCheckout) -> Self {
        Self {
            checkout,
            status: LocalBuildStatus::Idle,
            daemon_ui_owned: None,
        }
    }

    pub fn checkout(&self) -> &LocalCheckout {
        &self.checkout
    }

    pub fn status(&self) -> &LocalBuildStatus {
        &self.status
    }

    pub fn daemon_ui_owned(&self) -> Option<bool> {
        self.daemon_ui_owned
    }

    pub fn set_daemon_ui_owned(&mut self, ui_owned: bool, cx: &mut Context<Self>) {
        self.daemon_ui_owned = Some(ui_owned);
        cx.notify();
    }

    pub fn try_start_build(&mut self, cx: &mut Context<Self>) -> Option<LocalCheckout> {
        if !self.can_build() {
            return None;
        }
        self.status = LocalBuildStatus::Building;
        cx.notify();
        Some(self.checkout.clone())
    }

    pub fn try_start_restart(&mut self, cx: &mut Context<Self>) -> Option<LocalCheckout> {
        if !self.can_restart() {
            return None;
        }
        self.status = LocalBuildStatus::RestartingDaemon;
        cx.notify();
        Some(self.checkout.clone())
    }

    pub fn set_status(&mut self, status: LocalBuildStatus, cx: &mut Context<Self>) {
        self.status = status;
        cx.notify();
    }

    fn can_build(&self) -> bool {
        self.daemon_ui_owned == Some(true)
            && !matches!(
                self.status,
                LocalBuildStatus::Building
                    | LocalBuildStatus::ReadyToRestart
                    | LocalBuildStatus::RestartingDaemon
                    | LocalBuildStatus::RestartingApp
            )
    }

    fn can_restart(&self) -> bool {
        self.daemon_ui_owned == Some(true)
            && matches!(self.status, LocalBuildStatus::ReadyToRestart)
    }
}

#[cfg(feature = "gpui-ui")]
#[derive(Clone)]
pub struct GlobalLocalBuild(pub Entity<LocalBuildState>);

#[cfg(feature = "gpui-ui")]
impl Global for GlobalLocalBuild {}

#[cfg(test)]
mod tests {
    use super::detect_local_checkout_from;
    #[cfg(feature = "gpui-ui")]
    use super::{LocalBuildState, LocalBuildStatus, LocalCheckout};
    use std::path::Path;

    fn binary_name() -> &'static str {
        if cfg!(windows) { "okena.exe" } else { "okena" }
    }

    #[test]
    fn detects_debug_and_release_binaries_inside_workspace_target() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap();
        for profile in ["debug", "release"] {
            let executable = root.join("target").join(profile).join(binary_name());
            let checkout = detect_local_checkout_from(&executable, root).unwrap();
            assert_eq!(checkout.root(), root);
            assert_eq!(
                checkout.release_executable(),
                root.join("target").join("release").join(binary_name())
            );
        }
    }

    #[test]
    fn rejects_installed_and_non_okena_binaries() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap();
        let installed = Path::new("/usr/local/bin").join(binary_name());
        assert!(detect_local_checkout_from(&installed, root).is_none());
        assert!(detect_local_checkout_from(&root.join("target/debug/helper"), root).is_none());
    }

    #[cfg(feature = "gpui-ui")]
    #[test]
    fn rebuild_requires_managed_daemon_and_non_active_status() {
        let checkout = LocalCheckout {
            root: "/repo".into(),
            release_executable: "/repo/target/release/okena".into(),
        };
        let mut state = LocalBuildState::new(checkout);
        assert!(!state.can_build());

        state.daemon_ui_owned = Some(true);
        assert!(state.can_build());
        state.status = LocalBuildStatus::Failed {
            error: "failed".to_string(),
        };
        assert!(state.can_build());

        for status in [
            LocalBuildStatus::Building,
            LocalBuildStatus::ReadyToRestart,
            LocalBuildStatus::RestartingDaemon,
            LocalBuildStatus::RestartingApp,
        ] {
            state.status = status;
            assert!(!state.can_build());
        }
    }

    #[cfg(feature = "gpui-ui")]
    #[test]
    fn restart_requires_completed_build_and_managed_daemon() {
        let checkout = LocalCheckout {
            root: "/repo".into(),
            release_executable: "/repo/target/release/okena".into(),
        };
        let mut state = LocalBuildState::new(checkout);
        state.status = LocalBuildStatus::ReadyToRestart;
        assert!(!state.can_restart());

        state.daemon_ui_owned = Some(true);
        assert!(state.can_restart());

        state.status = LocalBuildStatus::Idle;
        assert!(!state.can_restart());
    }
}
