use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

const VALIDATION_TIMEOUT: Duration = Duration::from_secs(10);

static LAUNCH_PATH: OnceLock<PathBuf> = OnceLock::new();

/// Path this process was launched from, resolved once and cached.
///
/// On Linux `current_exe()` reads `/proc/self/exe`, which follows the inode; an
/// in-place update renames it, so a later call yields `…okena.old (deleted)`.
pub fn remember_launch_path() -> Result<PathBuf> {
    if let Some(path) = LAUNCH_PATH.get() {
        return Ok(path.clone());
    }
    let path = std::env::current_exe().context("failed to get current exe path")?;
    let _ = LAUNCH_PATH.set(path.clone());
    Ok(LAUNCH_PATH.get().cloned().unwrap_or(path))
}

/// Extract the archive and replace the current binary.
pub fn install_update(archive_path: &Path) -> Result<PathBuf> {
    let current_exe = remember_launch_path()?;

    let extract_dir = archive_path
        .parent()
        .context("archive has no parent dir")?
        .join("extracted");

    let _ = std::fs::remove_dir_all(&extract_dir);
    std::fs::create_dir_all(&extract_dir).context("failed to create extraction dir")?;

    extract_archive(archive_path, &extract_dir)?;

    install_sibling_if_present(&current_exe, &extract_dir, main_binary_name())?;
    install_sibling_if_present(&current_exe, &extract_dir, daemon_binary_name())?;

    let current_name = current_exe
        .file_name()
        .and_then(|name| name.to_str())
        .context("current executable has no file name")?;
    let new_binary = find_binary_named(&extract_dir, current_name)?;

    replace_binary(&current_exe, &new_binary)?;

    validate_binary(&current_exe)?;

    let _ = std::fs::remove_dir_all(&extract_dir);
    let _ = std::fs::remove_file(archive_path);

    Ok(current_exe)
}

fn install_sibling_if_present(
    current_exe: &Path,
    extract_dir: &Path,
    binary_name: &str,
) -> Result<()> {
    let Some(current_name) = current_exe.file_name().and_then(|name| name.to_str()) else {
        return Ok(());
    };
    if current_name == binary_name {
        return Ok(());
    }

    let Some(parent) = current_exe.parent() else {
        return Ok(());
    };
    let target = parent.join(binary_name);
    if !target.exists() {
        return Ok(());
    }

    match find_binary_named(extract_dir, binary_name) {
        Ok(new_binary) => {
            replace_binary(&target, &new_binary)?;
            validate_binary(&target)?;
        }
        Err(e) => {
            log::warn!("Update archive does not contain sibling binary {binary_name}: {e}");
        }
    }

    Ok(())
}

/// Restart the application by spawning a new process and quitting.
///
/// Quits only once the successor is spawned, so a caller can keep its UI usable
/// when the spawn fails.
#[cfg(feature = "gpui-ui")]
pub fn restart_app(cx: &mut gpui::App) -> Result<()> {
    spawn_successor(&remember_launch_path()?)?;
    log::info!("Restarting okena...");
    cx.quit();
    Ok(())
}

#[cfg(feature = "gpui-ui")]
fn spawn_successor(exe: &Path) -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    crate::process::command(&exe.to_string_lossy())
        .args(&args)
        .spawn()
        .with_context(|| format!("failed to restart from {}", exe.display()))?;
    Ok(())
}

/// Remove leftover `.old` binary from a previous update.
///
/// Kept while a config restore is pending: the `.old` binary is the only one
/// that understands the revert handoff (see `local::spawn_replacement_daemon`).
pub fn cleanup_old_binary() {
    // Prime the cache before any early return: this is the daemon's only
    // priming hook, and `init()` does not run there.
    let exe = remember_launch_path();

    if okena_core::profiles::try_current()
        .is_some_and(|paths| paths.pending_config_restore().is_file())
    {
        log::info!("Keeping the .old binary: a config restore is still pending");
        return;
    }
    if let Ok(exe) = exe {
        let old_path = old_binary_path(&exe);
        if old_path.exists() {
            match std::fs::remove_file(&old_path) {
                Ok(()) => log::info!("Cleaned up old binary: {:?}", old_path),
                Err(e) => log::warn!("Failed to clean up old binary {:?}: {}", old_path, e),
            }
        }
    }
}

fn validate_binary(binary: &Path) -> Result<()> {
    validate_binary_with_timeout(binary, VALIDATION_TIMEOUT)
}

/// Probe the freshly installed binary; every failure — spawn, wait, timeout or a
/// nonzero exit — restores the previous binary and reports whether that worked.
fn validate_binary_with_timeout(binary: &Path, timeout: Duration) -> Result<()> {
    let Err(error) = run_version_probe(binary, timeout) else {
        log::info!("Binary validation passed");
        return Ok(());
    };

    log::error!("Binary validation failed, rolling back: {error:#}");
    match restore_previous_binary(binary) {
        Ok(()) => Err(error.context(format!(
            "validation of {} failed; restored the previous binary",
            binary.display()
        ))),
        Err(rollback) => Err(error.context(format!(
            "validation of {} failed and rollback failed: {rollback:#}",
            binary.display()
        ))),
    }
}

fn run_version_probe(binary: &Path, timeout: Duration) -> Result<()> {
    let mut child = crate::process::command(&binary.to_string_lossy())
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("failed to spawn binary for validation")?;

    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => anyhow::bail!("new binary failed validation (exit {status})"),
            Ok(None) => {
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    anyhow::bail!("binary validation timed out after {:?}", timeout);
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error).context("failed to wait on validation process");
            }
        }
    }
}

/// Put the previous binary back. The rename replaces the destination in one
/// step, so the install path never sits empty even if the process dies here.
fn restore_previous_binary(binary: &Path) -> Result<()> {
    let old_path = old_binary_path(binary);
    if !old_path.exists() {
        anyhow::bail!("no previous binary at {}", old_path.display());
    }
    rename_with_retry(&old_path, binary)
        .with_context(|| format!("failed to restore {}", binary.display()))
}

/// Rename, retrying on Windows where antivirus can briefly hold a binary open.
fn rename_with_retry(from: &Path, to: &Path) -> std::io::Result<()> {
    let attempts = if cfg!(windows) { 5 } else { 1 };
    for _ in 1..attempts {
        if std::fs::rename(from, to).is_ok() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    std::fs::rename(from, to)
}

fn old_binary_path(binary: &Path) -> PathBuf {
    binary.with_extension(if cfg!(windows) { "exe.old" } else { "old" })
}

fn extract_archive(archive: &Path, dest: &Path) -> Result<()> {
    let name = archive.file_name().unwrap_or_default().to_string_lossy();

    if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        let status = crate::process::command("tar")
            .args([
                "xzf",
                &archive.to_string_lossy(),
                "-C",
                &dest.to_string_lossy(),
            ])
            .status()
            .context("failed to run tar")?;
        if !status.success() {
            anyhow::bail!("tar extraction failed with status {}", status);
        }
    } else if name.ends_with(".zip") {
        #[cfg(unix)]
        {
            let status = crate::process::command("unzip")
                .args([
                    "-o",
                    &archive.to_string_lossy(),
                    "-d",
                    &dest.to_string_lossy(),
                ])
                .status()
                .context("failed to run unzip")?;
            if !status.success() {
                anyhow::bail!("unzip failed with status {}", status);
            }
        }
        #[cfg(windows)]
        {
            let status = crate::process::command("tar")
                .args([
                    "-xf",
                    &archive.to_string_lossy(),
                    "-C",
                    &dest.to_string_lossy(),
                ])
                .status()
                .context("failed to run tar on Windows")?;
            if !status.success() {
                anyhow::bail!("tar extraction failed with status {}", status);
            }
        }
    } else {
        anyhow::bail!("unknown archive format: {}", name);
    }

    Ok(())
}

fn main_binary_name() -> &'static str {
    if cfg!(windows) { "okena.exe" } else { "okena" }
}

fn daemon_binary_name() -> &'static str {
    if cfg!(windows) {
        "okena-daemon.exe"
    } else {
        "okena-daemon"
    }
}

fn find_binary_named(dir: &Path, binary_name: &str) -> Result<PathBuf> {
    find_binary_recursive(dir, binary_name, 3)
        .with_context(|| format!("could not find '{}' in extracted archive", binary_name))
}

fn find_binary_recursive(dir: &Path, name: &str, depth: u32) -> Result<PathBuf> {
    let direct = dir.join(name);
    if direct.exists() {
        return Ok(direct);
    }

    if depth == 0 {
        anyhow::bail!("search depth exhausted");
    }

    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir()
                && let Ok(found) = find_binary_recursive(&path, name, depth - 1)
            {
                return Ok(found);
            }
        }
    }

    anyhow::bail!("not found at this level")
}

fn replace_binary(current: &Path, new_binary: &Path) -> Result<()> {
    let target = current.to_path_buf();
    let old_path = old_binary_path(&target);

    let _ = std::fs::remove_file(&old_path);

    rename_with_retry(&target, &old_path)
        .context("failed to rename the current binary (on Windows antivirus may hold it open)")?;

    if let Err(e) = std::fs::copy(new_binary, &target) {
        log::error!("Failed to copy new binary, rolling back: {}", e);
        let _ = std::fs::rename(&old_path, &target);
        return Err(e).context("failed to copy new binary");
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(e) = std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755)) {
            log::error!("Failed to set permissions, rolling back: {}", e);
            let _ = std::fs::remove_file(&target);
            let _ = std::fs::rename(&old_path, &target);
            return Err(e).context("failed to set executable permission");
        }
    }

    log::info!("Replaced binary at {:?}", target);
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "okena-updater-{tag}-{:?}-{}",
            std::thread::current().id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock after epoch")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    /// Lay out an installed binary that already has its `.old` predecessor.
    fn staged(tag: &str, body: &str, mode: u32) -> (PathBuf, PathBuf, PathBuf) {
        let dir = temp_dir(tag);
        let binary = dir.join("okena");
        std::fs::write(&binary, body).expect("write binary");
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(mode))
            .expect("set binary mode");
        let old = old_binary_path(&binary);
        std::fs::write(&old, "previous").expect("write previous binary");
        (dir, binary, old)
    }

    fn script(exit_body: &str) -> String {
        format!("#!/bin/sh\n{exit_body}\n")
    }

    fn assert_rolled_back(binary: &Path, old: &Path) {
        assert_eq!(
            std::fs::read_to_string(binary).expect("binary restored"),
            "previous"
        );
        assert!(!old.exists(), "the .old copy must be consumed by rollback");
    }

    #[test]
    fn spawn_failure_restores_the_previous_binary() {
        let (dir, binary, old) = staged("spawn", "not an executable", 0o644);
        let error = validate_binary(&binary).expect_err("validation must fail");
        assert!(format!("{error:#}").contains("restored the previous binary"));
        assert_rolled_back(&binary, &old);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn nonzero_exit_restores_the_previous_binary() {
        let (dir, binary, old) = staged("exit", &script("exit 3"), 0o755);
        let error = validate_binary(&binary).expect_err("validation must fail");
        assert!(format!("{error:#}").contains("exit"));
        assert_rolled_back(&binary, &old);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn timeout_restores_the_previous_binary() {
        let (dir, binary, old) = staged("timeout", &script("exec sleep 30"), 0o755);
        let error = validate_binary_with_timeout(&binary, Duration::from_millis(200))
            .expect_err("validation must time out");
        assert!(format!("{error:#}").contains("timed out"));
        assert_rolled_back(&binary, &old);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn rollback_failure_is_reported_and_keeps_the_binary() {
        let (dir, binary, old) = staged("norollback", "not an executable", 0o644);
        std::fs::remove_file(&old).expect("drop the previous binary");
        let error = validate_binary(&binary).expect_err("validation must fail");
        let message = format!("{error:#}");
        assert!(message.contains("rollback failed"), "{message}");
        assert!(binary.exists(), "nothing to restore, so nothing is deleted");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn successful_validation_keeps_the_new_binary() {
        let (dir, binary, old) = staged("ok", &script("exit 0"), 0o755);
        validate_binary(&binary).expect("validation must pass");
        assert!(binary.exists());
        assert!(old.exists(), "the caller owns .old cleanup, not validation");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn restore_replaces_a_present_binary() {
        let (dir, binary, old) = staged("restore", "new", 0o755);
        restore_previous_binary(&binary).expect("restore");
        assert_rolled_back(&binary, &old);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(feature = "gpui-ui")]
    #[test]
    fn a_failed_restart_spawn_is_reported() {
        let dir = temp_dir("restart");
        let missing = dir.join("okena.old");
        let error = spawn_successor(&missing).expect_err("spawn must fail");
        assert!(format!("{error:#}").contains("failed to restart from"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
