use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// Extract the archive and replace the current binary.
/// Returns the path to the new binary.
pub fn install_update(archive_path: &Path) -> Result<PathBuf> {
    let current_exe = std::env::current_exe()
        .context("failed to get current exe path")?;

    let extract_dir = archive_path
        .parent()
        .context("archive has no parent dir")?
        .join("extracted");

    // Clean up previous extraction
    let _ = std::fs::remove_dir_all(&extract_dir);
    std::fs::create_dir_all(&extract_dir)
        .context("failed to create extraction dir")?;

    // Extract the archive
    extract_archive(archive_path, &extract_dir)?;

    // Find the binary in the extracted contents
    let new_binary = find_binary(&extract_dir)?;

    // Replace the current binary
    replace_binary(&current_exe, &new_binary)?;

    // Validate the new binary can execute
    validate_binary(&current_exe)?;

    // Clean up extraction dir and downloaded archive
    let _ = std::fs::remove_dir_all(&extract_dir);
    let _ = std::fs::remove_file(archive_path);

    Ok(current_exe)
}

/// Restart the application by spawning a new process and quitting.
pub fn restart_app(cx: &mut gpui::App) {
    if let Ok(exe) = std::env::current_exe() {
        let args: Vec<String> = std::env::args().skip(1).collect();
        // Spawn the new instance first, then quit. If spawn fails we
        // keep the current process running instead of leaving the user
        // with nothing.
        match std::process::Command::new(&exe).args(&args).spawn() {
            Ok(_) => {
                log::info!("Restarting okena...");
                cx.quit();
            }
            Err(e) => log::error!("Failed to restart: {}", e),
        }
    }
}

/// Run the binary with `--version` to verify it's not corrupt.
/// If validation fails or times out, roll back to the `.old` binary.
fn validate_binary(binary: &Path) -> Result<()> {
    let old_path = binary.with_extension(if cfg!(windows) { "exe.old" } else { "old" });

    let mut child = std::process::Command::new(binary)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("failed to spawn binary for validation")?;

    let timeout = std::time::Duration::from_secs(10);
    let start = std::time::Instant::now();

    let status = loop {
        match child.try_wait().context("failed to wait on validation process")? {
            Some(status) => break status,
            None => {
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    log::error!("Binary validation timed out, rolling back");
                    let _ = std::fs::remove_file(binary);
                    let _ = std::fs::rename(&old_path, binary);
                    anyhow::bail!("binary validation timed out after {}s", timeout.as_secs());
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }
    };

    if status.success() {
        log::info!("Binary validation passed");
        Ok(())
    } else {
        log::error!("Binary validation failed (exit {}), rolling back", status);
        let _ = std::fs::remove_file(binary);
        let _ = std::fs::rename(&old_path, binary);
        anyhow::bail!("new binary failed validation (exit {})", status);
    }
}

/// Remove leftover `.old` binary from a previous update, if any.
pub fn cleanup_old_binary() {
    if let Ok(exe) = std::env::current_exe() {
        let old_path = exe.with_extension(if cfg!(windows) { "exe.old" } else { "old" });
        if old_path.exists() {
            match std::fs::remove_file(&old_path) {
                Ok(()) => log::info!("Cleaned up old binary: {:?}", old_path),
                Err(e) => log::warn!("Failed to clean up old binary {:?}: {}", old_path, e),
            }
        }
    }
}

fn extract_archive(archive: &Path, dest: &Path) -> Result<()> {
    let name = archive
        .file_name()
        .unwrap_or_default()
        .to_string_lossy();

    if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        // Linux: tar xzf
        let status = std::process::Command::new("tar")
            .args(["xzf", &archive.to_string_lossy(), "-C", &dest.to_string_lossy()])
            .status()
            .context("failed to run tar")?;
        if !status.success() {
            anyhow::bail!("tar extraction failed with status {}", status);
        }
    } else if name.ends_with(".zip") {
        // macOS / Windows
        #[cfg(unix)]
        {
            let status = std::process::Command::new("unzip")
                .args(["-o", &archive.to_string_lossy().into_owned(), "-d", &dest.to_string_lossy().into_owned()])
                .status()
                .context("failed to run unzip")?;
            if !status.success() {
                anyhow::bail!("unzip failed with status {}", status);
            }
        }
        #[cfg(windows)]
        {
            // Windows 10+ has tar that can handle zip
            let status = std::process::Command::new("tar")
                .args(["-xf", &archive.to_string_lossy(), "-C", &dest.to_string_lossy()])
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

/// Search the extracted directory for the okena binary (up to 3 levels deep).
/// This handles archives containing bare binaries, subdirectory-wrapped binaries,
/// and macOS .app bundles (e.g. Okena.app/Contents/MacOS/okena).
fn find_binary(dir: &Path) -> Result<PathBuf> {
    #[cfg(unix)]
    let binary_name = "okena";
    #[cfg(windows)]
    let binary_name = "okena.exe";

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
            if path.is_dir() {
                if let Ok(found) = find_binary_recursive(&path, name, depth - 1) {
                    return Ok(found);
                }
            }
        }
    }

    anyhow::bail!("not found at this level")
}

fn replace_binary(current: &Path, new_binary: &Path) -> Result<()> {
    let target = current.to_path_buf();

    let old_path = target.with_extension(if cfg!(windows) { "exe.old" } else { "old" });

    // Remove previous .old if it exists
    let _ = std::fs::remove_file(&old_path);

    // Rename current binary to .old
    // On Windows, running executables can usually be renamed but AV software
    // may briefly lock the file, so retry a few times.
    #[cfg(windows)]
    {
        let mut last_err = None;
        for _ in 0..5 {
            match std::fs::rename(&target, &old_path) {
                Ok(()) => {
                    last_err = None;
                    break;
                }
                Err(e) => {
                    last_err = Some(e);
                    std::thread::sleep(std::time::Duration::from_millis(500));
                }
            }
        }
        if let Some(e) = last_err {
            anyhow::bail!(
                "failed to rename current binary (file may be locked by antivirus): {}",
                e
            );
        }
    }

    #[cfg(not(windows))]
    std::fs::rename(&target, &old_path)
        .context("failed to rename current binary")?;

    // Copy new binary into place — rollback on failure
    if let Err(e) = std::fs::copy(new_binary, &target) {
        log::error!("Failed to copy new binary, rolling back: {}", e);
        let _ = std::fs::rename(&old_path, &target);
        return Err(e).context("failed to copy new binary");
    }

    // Set executable permission on Unix — rollback on failure
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
