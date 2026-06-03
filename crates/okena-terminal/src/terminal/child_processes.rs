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
        if let Ok(s) = std::fs::read_to_string(&path)
            && !s.trim().is_empty() {
                return true;
            }
    }
    false
}

#[cfg(all(unix, not(target_os = "linux")))]
pub fn has_child_processes(pid: u32) -> bool {
    crate::process::safe_output(
        crate::process::command("pgrep").args(["-P", &pid.to_string()]),
    )
    .map(|o| o.status.success())
    .unwrap_or(false)
}

#[cfg(not(unix))]
pub fn has_child_processes(_pid: u32) -> bool {
    false
}

/// Best-effort name of the foreground command running under `pid` (the shell):
/// the first child process's `comm` (e.g. "make", "vim", "cargo"). Linux only —
/// reads `/proc/<child>/comm`; returns `None` elsewhere or when the shell has no
/// child. Used to give the soft-close toast a "what am I killing" hint.
#[cfg(target_os = "linux")]
pub fn foreground_command(pid: u32) -> Option<String> {
    let task_dir = format!("/proc/{pid}/task");
    for entry in std::fs::read_dir(&task_dir).ok()?.flatten() {
        let file_name = entry.file_name();
        let Some(tid) = file_name.to_str() else { continue };
        let Ok(children) = std::fs::read_to_string(format!("/proc/{pid}/task/{tid}/children"))
        else {
            continue;
        };
        let Some(child_pid) = children.split_whitespace().next() else { continue };
        let Ok(comm) = std::fs::read_to_string(format!("/proc/{child_pid}/comm")) else {
            continue;
        };
        let name = comm.trim();
        if !name.is_empty() {
            return Some(name.to_string());
        }
    }
    None
}

#[cfg(not(target_os = "linux"))]
pub fn foreground_command(_pid: u32) -> Option<String> {
    None
}
