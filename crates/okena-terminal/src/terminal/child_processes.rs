/// Check if a process has any child processes.
///
/// On Linux, this reads `/proc/<pid>/task/*/children` directly — sub-millisecond,
/// safe to call synchronously from UI handlers (e.g. click / key-down).
/// On macOS, it uses `libproc` (`proc_listpids`) — no fork+exec.
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
            && !s.trim().is_empty()
        {
            return true;
        }
    }
    false
}

#[cfg(target_os = "macos")]
pub fn has_child_processes(pid: u32) -> bool {
    crate::macos_proc::has_children(pid)
}

#[cfg(all(unix, not(target_os = "linux"), not(target_os = "macos")))]
pub fn has_child_processes(pid: u32) -> bool {
    crate::process::safe_output(crate::process::command("pgrep").args(["-P", &pid.to_string()]))
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[cfg(not(unix))]
pub fn has_child_processes(_pid: u32) -> bool {
    false
}

/// Whether `pid` is known to run no child process. Unlike
/// `!has_child_processes`, false when the check fails or is unsupported.
#[cfg(target_os = "linux")]
pub fn has_no_child_processes(pid: u32) -> bool {
    let Ok(entries) = std::fs::read_dir(format!("/proc/{pid}/task")) else {
        return false;
    };
    let mut checked = false;
    for entry in entries {
        let Ok(entry) = entry else {
            return false;
        };
        match std::fs::read_to_string(entry.path().join("children")) {
            Ok(children) if children.trim().is_empty() => checked = true,
            _ => return false,
        }
    }
    checked
}

#[cfg(target_os = "macos")]
pub fn has_no_child_processes(pid: u32) -> bool {
    crate::macos_proc::process_name(pid).is_some()
        && crate::macos_proc::checked_child_pids(pid).is_some_and(|children| children.is_empty())
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn has_no_child_processes(_pid: u32) -> bool {
    false
}

/// Name of the process `pid` (`comm` on Linux), `None` when unknown.
#[cfg(target_os = "linux")]
pub fn process_name(pid: u32) -> Option<String> {
    let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
    Some(comm.trim().to_string()).filter(|name| !name.is_empty())
}

#[cfg(target_os = "macos")]
pub fn process_name(pid: u32) -> Option<String> {
    crate::macos_proc::process_name(pid)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn process_name(_pid: u32) -> Option<String> {
    None
}

/// Best-effort name of the foreground command running under `pid` (the shell):
/// the first child process's name (e.g. "make", "vim", "cargo"). On Linux reads
/// `/proc/<child>/comm`; on macOS uses `libproc`; returns `None` elsewhere or
/// when the shell has no child. Gives the soft-close toast a "what am I killing"
/// hint.
#[cfg(target_os = "linux")]
pub fn foreground_command(pid: u32) -> Option<String> {
    let task_dir = format!("/proc/{pid}/task");
    for entry in std::fs::read_dir(&task_dir).ok()?.flatten() {
        let file_name = entry.file_name();
        let Some(tid) = file_name.to_str() else {
            continue;
        };
        let Ok(children) = std::fs::read_to_string(format!("/proc/{pid}/task/{tid}/children"))
        else {
            continue;
        };
        let Some(child_pid) = children.split_whitespace().next() else {
            continue;
        };
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

#[cfg(target_os = "macos")]
pub fn foreground_command(pid: u32) -> Option<String> {
    crate::macos_proc::first_child_pid(pid).and_then(crate::macos_proc::process_name)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn foreground_command(_pid: u32) -> Option<String> {
    None
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn no_child_processes_is_known_only_for_a_live_childless_process() {
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn sleep");
        assert!(has_no_child_processes(child.id()));
        assert!(!has_no_child_processes(std::process::id()));
        assert_eq!(process_name(child.id()).as_deref(), Some("sleep"));

        child.kill().expect("kill sleep");
        child.wait().expect("reap sleep");
        assert!(
            !has_no_child_processes(child.id()),
            "a missing pid is unknown"
        );
        assert_eq!(process_name(child.id()), None);
    }
}
