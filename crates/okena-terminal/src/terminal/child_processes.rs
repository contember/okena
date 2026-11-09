/// Direct children of `pid`, oldest first; `None` when `pid` is gone or its
/// children cannot be read.
///
/// Reads `/proc/<pid>/task/*/children` when the kernel has those files
/// (`CONFIG_PROC_CHILDREN`). A kernel built without them (the default kernel
/// of Apple's `container` VMs, some minimal VM and embedded kernels) gets a
/// scan of every `/proc/<n>/stat` for that parent instead. Which of the two
/// applies is decided once, from this process's own thread.
#[cfg(target_os = "linux")]
pub(crate) fn linux_children(pid: u32) -> Option<Vec<u32>> {
    static CHILDREN_FILES: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let children_files = *CHILDREN_FILES.get_or_init(|| {
        // `thread-self` does not depend on our pid matching the pid namespace
        // of `/proc`, but only exists since Linux 3.17; the `children` files
        // since 3.5.
        std::path::Path::new("/proc/thread-self/children").exists()
            || std::path::Path::new(&format!("/proc/self/task/{}/children", std::process::id()))
                .exists()
    });
    proc_children(std::path::Path::new("/proc"), pid, children_files)
}

/// [`linux_children`] under any `/proc` root, so tests can hand it a fake one.
#[cfg(any(target_os = "linux", test))]
fn proc_children(proc_root: &std::path::Path, pid: u32, children_files: bool) -> Option<Vec<u32>> {
    let process = proc_root.join(pid.to_string());
    let tasks = std::fs::read_dir(process.join("task")).ok()?;
    if !children_files {
        let children = children_by_parent(proc_root, pid);
        // The scan alone cannot tell a childless `pid` from one that just exited.
        return process.exists().then_some(children);
    }
    let mut children = Vec::new();
    let mut read_any = false;
    for task in tasks {
        let task = task.ok()?;
        match std::fs::read_to_string(task.path().join("children")) {
            Ok(list) => {
                read_any = true;
                children.extend(
                    list.split_whitespace()
                        .filter_map(|c| c.parse::<u32>().ok()),
                );
            }
            // That thread exited since the listing; its children moved to another.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return None,
        }
    }
    read_any.then_some(children)
}

/// Every process whose parent is `parent`, oldest first, from `/proc/<n>/stat`.
///
/// Sees only the processes `/proc` shows: with `hidepid=1/2` another user's
/// child (`sudo`, `su`) is missing, which the `children` files would list.
#[cfg(any(target_os = "linux", test))]
fn children_by_parent(proc_root: &std::path::Path, parent: u32) -> Vec<u32> {
    let Ok(entries) = std::fs::read_dir(proc_root) else {
        return Vec::new();
    };
    let mut found: Vec<(u64, u32)> = entries
        .flatten()
        .filter_map(|entry| {
            let pid = entry.file_name().to_str()?.parse::<u32>().ok()?;
            // Bytes: `comm` is raw and cut at 15 bytes, possibly mid-character.
            let stat = std::fs::read(entry.path().join("stat")).ok()?;
            let (ppid, start) = parent_and_start(&stat)?;
            (ppid == parent).then_some((start, pid))
        })
        .collect();
    found.sort_unstable();
    found.into_iter().map(|(_, pid)| pid).collect()
}

/// Parent pid and start time (field 22) of a `/proc/<n>/stat` line:
/// `pid (comm) state ppid …`. `comm` may hold spaces and parentheses, so the
/// fields are counted from its last `)`.
#[cfg(any(target_os = "linux", test))]
fn parent_and_start(stat: &[u8]) -> Option<(u32, u64)> {
    let after_comm = &stat[stat.iter().rposition(|&b| b == b')')? + 1..];
    let fields: Vec<&str> = std::str::from_utf8(after_comm)
        .ok()?
        .split_whitespace()
        .collect();
    let ppid = fields.get(1)?.parse().ok()?;
    let start = fields
        .get(19)
        .and_then(|f| f.parse().ok())
        .unwrap_or(u64::MAX);
    Some((ppid, start))
}

/// Check if a process has any child processes.
///
/// On Linux, this reads `/proc` directly ([`linux_children`]): sub-millisecond,
/// safe to call synchronously from UI handlers (e.g. click / key-down).
/// On macOS, it uses `libproc` (`proc_listpids`) — no fork+exec.
/// On other Unix, falls back to `pgrep -P` (~5–20 ms fork+exec).
/// On non-Unix, always returns false.
#[cfg(target_os = "linux")]
pub fn has_child_processes(pid: u32) -> bool {
    linux_children(pid).is_some_and(|c| !c.is_empty())
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
    linux_children(pid).is_some_and(|c| c.is_empty())
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
    Some(name_for_version(comm, arg0, || {
        std::fs::read_link(format!("/proc/{pid}/exe"))
            .ok()
            .and_then(|path| path.to_str().map(str::to_string))
    }))
}

#[cfg(target_os = "macos")]
pub fn process_name(pid: u32) -> Option<String> {
    let name = crate::macos_proc::process_name(pid)?;
    if !looks_like_version(&name) {
        return Some(name);
    }
    let arg0 = crate::macos_proc::process_arg0(pid);
    Some(name_for_version(&name, arg0.as_deref(), || {
        crate::macos_proc::process_path(pid)
    }))
}

/// What to call a process the kernel named `version`: the command `arg0`
/// names, else the command a versioned `arg0` or executable path belongs to
/// (the executable is looked up only then), else `version`. A versioned argv[0]
/// still names the command after an update deleted that version's file.
#[cfg(any(target_os = "linux", target_os = "macos", test))]
fn name_for_version(
    version: &str,
    arg0: Option<&str>,
    exe: impl FnOnce() -> Option<String>,
) -> String {
    arg0.and_then(command_name)
        .filter(|name| !looks_like_version(name))
        .or_else(|| arg0.and_then(versioned_command))
        .or_else(|| exe().as_deref().and_then(versioned_command))
        .unwrap_or_else(|| version.to_string())
}

/// The command name `arg0` (a process's `argv[0]`) stands for: its file name,
/// without the one `-` a login shell gets. `None` for anything that is not a
/// plain command name: a rewritten process title (spaces), an environment
/// entry, or an option (what an empty argv[0] leaves the parser with).
#[cfg(any(target_os = "linux", target_os = "macos", test))]
fn command_name(arg0: &str) -> Option<String> {
    if arg0.contains([' ', '=']) {
        return None;
    }
    let name = arg0.rsplit('/').next()?;
    let name = name.strip_prefix('-').unwrap_or(name);
    (!name.is_empty() && !name.starts_with('-')).then(|| name.to_string())
}

/// `claude` for `…/claude/versions/2.1.316`: the directory a versioned
/// executable's `versions` folder sits in.
#[cfg(any(target_os = "linux", target_os = "macos", test))]
fn versioned_command(exe: &str) -> Option<String> {
    // Linux marks the executable of a process whose file is gone.
    let exe = exe.strip_suffix(" (deleted)").unwrap_or(exe);
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
    linux_children(pid)?.into_iter().find_map(process_name)
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
        // An empty argv[0] leaves the parser on argv[1].
        assert_eq!(command_name("--resume"), None);
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
        let exe = || Some(versioned.to_string());
        // Started through the `claude` symlink: argv[0] says so, and the
        // executable is not even looked up.
        assert_eq!(
            name_for_version("2.1.316", Some("/Users/u/.local/bin/claude"), || {
                panic!("not needed")
            }),
            "claude"
        );
        // Started by the versioned path (a relaunch after an update).
        assert_eq!(name_for_version("2.1.316", Some(versioned), exe), "claude");
        assert_eq!(name_for_version("2.1.316", None, exe), "claude");
        // That version's file was deleted by a later update.
        assert_eq!(
            name_for_version("2.1.316", Some(versioned), || None),
            "claude"
        );
        assert_eq!(
            name_for_version("2.1.316", None, || Some(format!("{versioned} (deleted)"))),
            "claude"
        );
        // Nothing better known: the kernel's name stays.
        assert_eq!(
            name_for_version("1.2", Some("1.2"), || Some("/opt/x/1.2".into())),
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
        let _ = std::fs::remove_dir_all(&root);
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

/// The `/proc` walk over a fake root, so it runs on any host; the real
/// kernel is covered by `linux_children_match_on_this_kernel` below.
#[cfg(test)]
mod proc_tests {
    use super::*;
    use std::path::{Path, PathBuf};

    /// A `/proc` with `(pid, ppid, start time, comm)` processes, each with one
    /// task. `children` writes the kernel's `children` files, as with
    /// `CONFIG_PROC_CHILDREN`.
    fn fake_proc(name: &str, processes: &[(u32, u32, u64, &str)], children: bool) -> PathBuf {
        let root = std::env::temp_dir().join(format!("okena-proc-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for &(pid, ppid, start, comm) in processes {
            let task = root
                .join(pid.to_string())
                .join("task")
                .join(pid.to_string());
            std::fs::create_dir_all(&task).unwrap();
            // Fields 5..=21 between ppid (4) and starttime (22) are irrelevant here.
            let filler = vec!["0"; 17].join(" ");
            std::fs::write(
                root.join(pid.to_string()).join("stat"),
                format!("{pid} ({comm}) S {ppid} {filler} {start} 0 0\n"),
            )
            .unwrap();
            if children {
                let mut kids: Vec<_> = processes.iter().filter(|p| p.1 == pid).collect();
                kids.sort_by_key(|p| p.2);
                let list: Vec<String> = kids.iter().map(|p| format!("{} ", p.0)).collect();
                std::fs::write(task.join("children"), list.concat()).unwrap();
            }
        }
        // Not a process: /proc also holds files like these.
        std::fs::create_dir_all(root.join("sys")).unwrap();
        std::fs::write(root.join("uptime"), "1.0 1.0\n").unwrap();
        root
    }

    const SHELL: u32 = 100;
    /// A shell with two children started out of pid order, a grandchild, an
    /// unrelated process and one whose `comm` imitates a child of the shell.
    const PROCESSES: &[(u32, u32, u64, &str)] = &[
        (1, 0, 1, "init"),
        (SHELL, 1, 5, "zsh"),
        (101, SHELL, 50, "claude"),
        (102, SHELL, 10, "git status"),
        (103, 101, 60, "node"),
        (104, 1, 70, "evil) S 100 (x"),
    ];

    fn children(root: &Path, pid: u32, children_files: bool) -> Option<Vec<u32>> {
        proc_children(root, pid, children_files)
    }

    #[test]
    fn a_kernel_without_children_files_is_scanned_by_parent() {
        let root = fake_proc("scan", PROCESSES, false);
        assert_eq!(children(&root, SHELL, false), Some(vec![102, 101]));
        assert_eq!(children(&root, 101, false), Some(vec![103]));
        assert_eq!(children(&root, 103, false), Some(vec![]));
        assert_eq!(
            children(&root, 999, false),
            None,
            "a missing pid is unknown"
        );
        // Reading the missing files instead would know nothing.
        assert_eq!(children(&root, SHELL, true), None);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_kernel_with_children_files_reads_them() {
        let root = fake_proc("files", PROCESSES, true);
        assert_eq!(children(&root, SHELL, true), Some(vec![102, 101]));
        assert_eq!(children(&root, 103, true), Some(vec![]));
        assert_eq!(children(&root, 999, true), None);
        // The scan agrees with the files.
        assert_eq!(children(&root, SHELL, false), Some(vec![102, 101]));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn stat_fields_count_from_the_last_parenthesis() {
        // Fields 5..=21 sit between ppid (4) and starttime (22).
        let filler = vec!["0"; 17].join(" ");
        assert_eq!(
            parent_and_start(format!("7 (a) b) S 3 {filler} 42 0").as_bytes()),
            Some((3, 42))
        );
        assert_eq!(parent_and_start(b"7 (sh) S 3"), Some((3, u64::MAX)));
        // A `comm` cut inside a multi-byte character is not UTF-8.
        assert_eq!(parent_and_start(b"7 (sk\xc3) S 3"), Some((3, u64::MAX)));
        assert_eq!(parent_and_start(b"7 sh S 3"), None);
        assert_eq!(parent_and_start(b""), None);
    }

    /// The real `/proc`, both ways: the scan must find what the kernel's own
    /// `children` files list (when this kernel has them).
    #[cfg(target_os = "linux")]
    #[test]
    fn linux_children_match_on_this_kernel() {
        let proc_root = Path::new("/proc");
        let mut first = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let mut second = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let me = std::process::id();

        let scanned = children(proc_root, me, false).expect("this process is alive");
        let first_at = scanned.iter().position(|&p| p == first.id());
        let second_at = scanned.iter().position(|&p| p == second.id());
        assert!(first_at.is_some() && second_at.is_some(), "{scanned:?}");
        assert!(first_at <= second_at, "oldest first: {scanned:?}");
        assert_eq!(children(proc_root, first.id(), false), Some(vec![]));
        assert_eq!(foreground_command(first.id()), None);

        // Other tests spawn and reap children of this process meanwhile, so
        // the two reads are compared on this test's own children only.
        if Path::new("/proc/thread-self/children").exists() {
            let listed = children(proc_root, me, true).unwrap();
            assert!(
                listed.contains(&first.id()) && listed.contains(&second.id()),
                "{listed:?}"
            );
        }

        for child in [&mut first, &mut second] {
            child.kill().unwrap();
            child.wait().unwrap();
        }
        assert_eq!(children(proc_root, first.id(), false), None);
    }
}
