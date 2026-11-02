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

/// Name of the process `pid` as the user would call it, `None` when unknown.
///
/// On Linux `comm`, unless it looks like a version: then the file name of
/// `argv[0]` from `/proc/<pid>/cmdline`. On macOS the file name of `argv[0]`,
/// falling back to libproc's name. Claude Code's native installer runs
/// `~/.local/bin/claude`, a symlink to `…/claude/versions/2.1.316`, and libproc
/// names the process after the resolved file, `2.1.316`; Okena compares this
/// name with `claude` to tell a running Claude from anything else.
#[cfg(target_os = "linux")]
pub fn process_name(pid: u32) -> Option<String> {
    let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
    let comm = comm.trim();
    if comm.is_empty() {
        return None;
    }
    if looks_like_version(comm) {
        let cmdline = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
        let arg0 = cmdline.split(|&b| b == 0).next().unwrap_or_default();
        if let Some(name) = std::str::from_utf8(arg0).ok().and_then(command_name) {
            return Some(name);
        }
    }
    Some(comm.to_string())
}

#[cfg(target_os = "macos")]
pub fn process_name(pid: u32) -> Option<String> {
    crate::macos_proc::process_arg0(pid)
        .as_deref()
        .and_then(command_name)
        .or_else(|| crate::macos_proc::process_name(pid))
}

/// The command name `arg0` (a process's `argv[0]`) stands for: its file name,
/// without the `-` a login shell gets.
#[cfg(any(target_os = "linux", target_os = "macos", test))]
fn command_name(arg0: &str) -> Option<String> {
    let name = arg0.rsplit('/').next()?.trim_start_matches('-');
    (!name.is_empty()).then(|| name.to_string())
}

/// Whether `name` is a dotted version like `2.1.316`, the file name of a
/// versioned executable rather than of a command.
#[cfg(any(target_os = "linux", test))]
fn looks_like_version(name: &str) -> bool {
    name.contains('.')
        && name
            .split('.')
            .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
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
        let Some(name) = child_pid.parse().ok().and_then(process_name) else {
            continue;
        };
        return Some(name);
    }
    None
}

#[cfg(target_os = "macos")]
pub fn foreground_command(pid: u32) -> Option<String> {
    crate::macos_proc::first_child_pid(pid).and_then(process_name)
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

#[cfg(test)]
mod name_tests {
    use super::*;

    #[test]
    fn command_names_drop_the_path_and_a_login_dash() {
        assert_eq!(command_name("claude").as_deref(), Some("claude"));
        assert_eq!(
            command_name("/Users/u/.local/bin/claude").as_deref(),
            Some("claude")
        );
        assert_eq!(command_name("-zsh").as_deref(), Some("zsh"));
        assert_eq!(command_name(""), None);
        assert_eq!(command_name("/usr/bin/"), None);
    }

    #[test]
    fn versions_are_dotted_numbers() {
        assert!(looks_like_version("2.1.316"));
        assert!(looks_like_version("1.0"));
        assert!(!looks_like_version("claude"));
        assert!(!looks_like_version("2"));
        assert!(!looks_like_version("python3.12"));
        assert!(!looks_like_version("1..2"));
    }

    /// The case behind this: a command reached through a symlink to a
    /// differently named file keeps the symlink's name.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_symlinked_command_is_named_after_the_symlink() {
        let dir = std::env::temp_dir().join(format!("okena-argv0-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let link = dir.join("claude");
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink("/bin/sleep", &link).unwrap();
        let mut child = std::process::Command::new(&link).arg("30").spawn().unwrap();
        let named = process_name(child.id());
        let resolved = crate::macos_proc::process_name(child.id());
        child.kill().unwrap();
        child.wait().unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(resolved.as_deref(), Some("sleep"));
        assert_eq!(named.as_deref(), Some("claude"));
    }
}
