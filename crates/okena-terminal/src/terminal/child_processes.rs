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
/// Normally the kernel's name: `comm` on Linux, libproc's on macOS. Claude
/// Code's native installer keeps each release as `…/claude/versions/2.1.316`
/// behind a `~/.local/bin/claude` symlink, and the kernel may name that process
/// `2.1.316` (macOS always resolves the symlink; Linux does when it was started
/// by the versioned path). Okena compares the name with `claude`, so only a
/// version-shaped name is replaced: by the file name of `argv[0]`, else by the
/// directory a `versions/<version>` executable lives under. Other processes
/// keep the kernel's name, which a rewritten process title cannot garble.
#[cfg(target_os = "linux")]
pub fn process_name(pid: u32) -> Option<String> {
    let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
    let comm = comm.trim();
    if comm.is_empty() {
        return None;
    }
    if !looks_like_version(comm) {
        return Some(comm.to_string());
    }
    let cmdline = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
    let arg0 = cmdline
        .split(|&b| b == 0)
        .next()
        .and_then(|arg| std::str::from_utf8(arg).ok());
    let exe = std::fs::read_link(format!("/proc/{pid}/exe")).ok();
    Some(name_for_version(
        comm,
        arg0,
        exe.as_deref().and_then(|path| path.to_str()),
    ))
}

#[cfg(target_os = "macos")]
pub fn process_name(pid: u32) -> Option<String> {
    let name = crate::macos_proc::process_name(pid)?;
    if !looks_like_version(&name) {
        return Some(name);
    }
    let arg0 = crate::macos_proc::process_arg0(pid);
    let exe = crate::macos_proc::process_path(pid);
    Some(name_for_version(&name, arg0.as_deref(), exe.as_deref()))
}

/// What to call a process the kernel named `version`: the command `arg0`
/// names, else the command a versioned `exe` path belongs to, else `version`.
#[cfg(any(target_os = "linux", target_os = "macos", test))]
fn name_for_version(version: &str, arg0: Option<&str>, exe: Option<&str>) -> String {
    arg0.and_then(command_name)
        .filter(|name| !looks_like_version(name))
        .or_else(|| exe.and_then(versioned_command))
        .unwrap_or_else(|| version.to_string())
}

/// The command name `arg0` (a process's `argv[0]`) stands for: its file name,
/// without the `-` a login shell gets. `None` for anything that is not a plain
/// command name: a rewritten process title (spaces) or an environment entry.
#[cfg(any(target_os = "linux", target_os = "macos", test))]
fn command_name(arg0: &str) -> Option<String> {
    if arg0.contains([' ', '=']) {
        return None;
    }
    let name = arg0.rsplit('/').next()?.trim_start_matches('-');
    (!name.is_empty()).then(|| name.to_string())
}

/// `claude` for `…/claude/versions/2.1.316`: the directory a versioned
/// executable's `versions` folder sits in.
#[cfg(any(target_os = "linux", target_os = "macos", test))]
fn versioned_command(exe: &str) -> Option<String> {
    let mut parts = exe.rsplit('/');
    let file = parts.next()?;
    if !looks_like_version(file) || parts.next()? != "versions" {
        return None;
    }
    parts
        .next()
        .filter(|name| !name.is_empty())
        .map(str::to_string)
}

/// Whether `name` is a dotted version like `2.1.316`, the file name of a
/// versioned executable rather than of a command.
#[cfg(any(target_os = "linux", target_os = "macos", test))]
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
/// the first child process's [`process_name`] (e.g. "make", "vim", "cargo",
/// "claude"); `None` elsewhere than Linux and macOS or when the shell has no
/// child. Gives the soft-close toast a "what am I killing" hint.
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
    fn command_names_are_plain_file_names() {
        assert_eq!(command_name("claude").as_deref(), Some("claude"));
        assert_eq!(
            command_name("/Users/u/.local/bin/claude").as_deref(),
            Some("claude")
        );
        assert_eq!(command_name("-zsh").as_deref(), Some("zsh"));
        assert_eq!(command_name(""), None);
        assert_eq!(command_name("/usr/bin/"), None);
        // A rewritten process title, or the environment after an empty argv.
        assert_eq!(
            command_name("npm run dev --config ./conf/vite.config.ts"),
            None
        );
        assert_eq!(command_name("HOME=/Users/u"), None);
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

    #[test]
    fn a_version_named_process_is_named_after_its_command() {
        let versioned = "/Users/u/.local/share/claude/versions/2.1.316";
        // Started through the `claude` symlink: argv[0] says so.
        assert_eq!(
            name_for_version(
                "2.1.316",
                Some("/Users/u/.local/bin/claude"),
                Some(versioned)
            ),
            "claude"
        );
        // Started by the versioned path (a relaunch after an update).
        assert_eq!(
            name_for_version("2.1.316", Some(versioned), Some(versioned)),
            "claude"
        );
        assert_eq!(name_for_version("2.1.316", None, Some(versioned)), "claude");
        // Nothing better known: the kernel's name stays.
        assert_eq!(
            name_for_version("1.2", Some("1.2"), Some("/opt/x/1.2")),
            "1.2"
        );
        assert_eq!(versioned_command("/opt/tool/versions/claude"), None);
        assert_eq!(versioned_command("/versions/1.0"), None);
    }

    /// The case behind this, end to end: Claude's native-installer layout,
    /// started through its symlink and by its versioned path.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_native_installer_claude_is_named_claude() {
        let root = std::env::temp_dir().join(format!("okena-argv0-{}", std::process::id()));
        let versions = root.join("claude").join("versions");
        let bin = root.join("bin");
        std::fs::create_dir_all(&versions).unwrap();
        std::fs::create_dir_all(&bin).unwrap();
        let release = versions.join("2.1.316");
        std::fs::copy("/bin/sleep", &release).unwrap();
        let link = bin.join("claude");
        std::os::unix::fs::symlink(&release, &link).unwrap();

        let name_of = |program: &std::path::Path| {
            let mut child = std::process::Command::new(program)
                .arg("30")
                .spawn()
                .unwrap();
            let names = (
                crate::macos_proc::process_name(child.id()),
                process_name(child.id()),
            );
            child.kill().unwrap();
            child.wait().unwrap();
            names
        };
        let through_link = name_of(&link);
        let direct = name_of(&release);
        // Any other process keeps libproc's name.
        let mut sleep = std::process::Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let plain = process_name(sleep.id());
        sleep.kill().unwrap();
        sleep.wait().unwrap();
        std::fs::remove_dir_all(&root).unwrap();

        assert_eq!(through_link.0.as_deref(), Some("2.1.316"));
        assert_eq!(through_link.1.as_deref(), Some("claude"));
        assert_eq!(direct.1.as_deref(), Some("claude"));
        assert_eq!(plain.as_deref(), Some("sleep"));
    }
}
