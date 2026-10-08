use std::path::{Path, PathBuf};

#[cfg(feature = "gpui-ui")]
use gpui::*;

/// A checkout-backed Okena executable that can rebuild itself with Cargo.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalCheckout {
    root: PathBuf,
    target_dir: PathBuf,
    release_executable: PathBuf,
}

impl LocalCheckout {
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn release_executable(&self) -> &Path {
        &self.release_executable
    }

    pub fn target_dir(&self) -> &Path {
        &self.target_dir
    }
}

/// Detect a source binary in a recognized Cargo artifact directory.
pub fn detect_local_checkout() -> Option<LocalCheckout> {
    let executable = std::env::current_exe().ok()?;
    let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = crate_dir.parent()?.parent()?;
    let runtime_target = std::env::var_os("CARGO_TARGET_DIR")
        .filter(|value| !value.is_empty())
        .and_then(|value| std::path::absolute(PathBuf::from(value)).ok());
    let compiled_target = option_env!("CARGO_TARGET_DIR")
        .filter(|value| !value.is_empty())
        .map(Path::new)
        .filter(|path| path.is_absolute());
    detect_local_checkout_from(
        &executable,
        workspace_root,
        runtime_target.as_deref(),
        compiled_target,
    )
}

fn detect_local_checkout_from(
    executable: &Path,
    workspace_root: &Path,
    runtime_target: Option<&Path>,
    compiled_target: Option<&Path>,
) -> Option<LocalCheckout> {
    let file_name = executable.file_name()?.to_str()?;
    let is_okena_binary = if cfg!(windows) {
        matches!(file_name, "okena.exe" | "okena-daemon.exe")
    } else {
        matches!(file_name, "okena" | "okena-daemon")
    };
    if !is_okena_binary {
        return None;
    }

    if !workspace_root.join("Cargo.toml").is_file() || !workspace_root.join(".git").exists() {
        return None;
    }

    let executable = executable
        .canonicalize()
        .unwrap_or_else(|_| executable.to_path_buf());
    let default_target = workspace_root.join("target");
    let target_dir = [
        runtime_target,
        compiled_target,
        Some(default_target.as_path()),
    ]
    .into_iter()
    .flatten()
    .map(|target| {
        target
            .canonicalize()
            .unwrap_or_else(|_| target.to_path_buf())
    })
    .filter(|target| executable.starts_with(target))
    .max_by_key(|target| target.components().count())?;
    let release_name = if cfg!(windows) { "okena.exe" } else { "okena" };
    Some(LocalCheckout {
        root: workspace_root.to_path_buf(),
        release_executable: target_dir.join("release").join(release_name),
        target_dir,
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

    fn workspace_root() -> &'static Path {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
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
            let checkout = detect_local_checkout_from(&executable, root, None, None).unwrap();
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
        assert!(detect_local_checkout_from(&installed, root, None, None).is_none());
        assert!(
            detect_local_checkout_from(&root.join("target/debug/helper"), root, None, None)
                .is_none()
        );
    }

    #[test]
    fn detects_external_shared_target_for_desktop_and_daemon() {
        let root = workspace_root();
        let target = root.parent().unwrap().join("shared-cargo-target");
        let daemon_name = if cfg!(windows) {
            "okena-daemon.exe"
        } else {
            "okena-daemon"
        };
        for name in [binary_name(), daemon_name] {
            for profile in ["debug", "release"] {
                let executable = target.join(profile).join(name);
                let checkout = detect_local_checkout_from(&executable, root, Some(&target), None)
                    .expect("recognized shared target");
                assert_eq!(checkout.root(), root);
                assert_eq!(checkout.target_dir(), target);
                assert_eq!(
                    checkout.release_executable(),
                    target.join("release").join(binary_name())
                );
            }
        }
    }

    #[test]
    fn compiled_target_detects_gui_started_without_target_environment() {
        let root = workspace_root();
        let target = root.parent().unwrap().join("compiled-cargo-target");
        let executable = target.join("release").join(binary_name());
        let checkout = detect_local_checkout_from(&executable, root, None, Some(&target))
            .expect("compiled target remains recognized without shell environment");
        assert_eq!(checkout.target_dir(), target);
        assert_eq!(checkout.release_executable(), executable);
    }

    #[test]
    fn executable_path_selects_target_when_runtime_and_compiled_values_disagree() {
        let root = workspace_root();
        let runtime = root.parent().unwrap().join("runtime-cargo-target");
        let compiled = root.parent().unwrap().join("compiled-cargo-target");
        for target in [&runtime, &compiled] {
            let executable = target.join("debug").join(binary_name());
            let checkout =
                detect_local_checkout_from(&executable, root, Some(&runtime), Some(&compiled))
                    .expect("the executable identifies which target directory to rebuild");
            assert_eq!(checkout.target_dir(), target);
            assert_eq!(
                checkout.release_executable(),
                target.join("release").join(binary_name())
            );
        }
    }

    #[test]
    fn nested_target_values_select_the_most_specific_artifact_directory() {
        let root = workspace_root();
        let runtime = root.parent().unwrap().join("shared-cargo-target");
        let compiled = runtime.join("nested-target");
        let executable = compiled.join("debug").join(binary_name());
        let checkout =
            detect_local_checkout_from(&executable, root, Some(&runtime), Some(&compiled))
                .expect("the compiled artifact directory is more specific than the runtime value");
        assert_eq!(checkout.target_dir(), compiled);
        assert_eq!(
            checkout.release_executable(),
            compiled.join("release").join(binary_name())
        );
    }

    #[test]
    fn configured_shared_targets_do_not_identify_installed_or_wrong_target_binaries() {
        let root = workspace_root();
        let target = root.parent().unwrap().join("shared-cargo-target");
        let installed = root
            .parent()
            .unwrap()
            .join("installed/bin")
            .join(binary_name());
        let wrong_target = root
            .parent()
            .unwrap()
            .join("shared-cargo-target-other/debug")
            .join(binary_name());
        for executable in [installed, wrong_target, target.join("debug/helper")] {
            assert!(
                detect_local_checkout_from(&executable, root, Some(&target), Some(&target))
                    .is_none()
            );
        }
    }

    #[test]
    fn default_target_remains_recognized_when_environment_points_elsewhere() {
        let root = workspace_root();
        let target = root.join("target");
        let external = root.parent().unwrap().join("shared-cargo-target");
        let executable = target.join("debug").join(binary_name());
        let checkout =
            detect_local_checkout_from(&executable, root, Some(&external), Some(&external))
                .expect("existing default artifact directory remains recognized");
        assert_eq!(
            checkout.target_dir(),
            target.canonicalize().unwrap_or(target)
        );
    }

    #[cfg(feature = "gpui-ui")]
    #[test]
    fn rebuild_requires_managed_daemon_and_non_active_status() {
        let checkout = LocalCheckout {
            root: "/repo".into(),
            target_dir: "/repo/target".into(),
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
            target_dir: "/repo/target".into(),
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
