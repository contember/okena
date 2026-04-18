/// Check if a process has any child processes.
///
/// On Linux, this reads `/proc/<pid>/task/*/children` directly — sub-millisecond,
/// safe to call synchronously from UI handlers (e.g. click / key-down).
/// On other Unix, falls back to `pgrep -P` (~5–20 ms fork+exec).
/// On non-Unix, always returns false.
#[cfg(target_os = "linux")]
pub fn has_child_processes(pid: u32) -> bool {
    let task_dir = format!("/proc/{}/task", pid);
    let Ok(entries) = std::fs::read_dir(&task_dir) else {
        return false;
    };
    for entry in entries.flatten() {
        let Some(tid) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let path = format!("/proc/{}/task/{}/children", pid, tid);
        if let Ok(s) = std::fs::read_to_string(&path) {
            if !s.trim().is_empty() {
                return true;
            }
        }
    }
    false
}

#[cfg(all(unix, not(target_os = "linux")))]
pub fn has_child_processes(pid: u32) -> bool {
    std::process::Command::new("pgrep")
        .args(["-P", &pid.to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(not(unix))]
pub fn has_child_processes(_pid: u32) -> bool {
    false
}
