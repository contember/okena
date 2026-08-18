//! External process execution.
//!
//! All *one-shot* commands (spawn, capture output, exit) flow through a single
//! global [`CommandBus`]: it bounds how many child processes run concurrently
//! across the whole app, audits every invocation, and supports timeouts and
//! group cancellation. See [`bus`] for the rationale (one global FD budget ⇒
//! one global cap).
//!
//! Most callers don't touch the bus directly — they keep building a
//! [`std::process::Command`] via [`command`] and pass it to [`safe_output`] /
//! [`safe_output_with_timeout`], which transparently route it through the bus
//! on the current thread's [`Lane`] (default [`Lane::Interactive`]; pollers opt
//! into [`Lane::Poll`] with [`with_lane`]).

mod bus;

pub use bus::{
    CommandBus, CommandCancellation, CommandHandle, CommandSpec, Lane, current_lane, with_lane,
};

/// Create a [`std::process::Command`] that does **not** flash a console
/// window on Windows.  On other platforms this is identical to
/// `std::process::Command::new(program)`.
pub fn command(program: &str) -> std::process::Command {
    #![allow(unused_mut)]
    let mut cmd = std::process::Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// Submit a fully-described command to the global bus and block until it
/// finishes. The structured entry point for callers that want to set a lane,
/// label, timeout, or cancellation scope explicitly.
pub fn run(spec: CommandSpec) -> std::io::Result<std::process::Output> {
    CommandBus::global().submit(spec).wait()
}

/// Spawn a child process and reap it on a background thread.
///
/// Fire-and-forget with no output capture, so it bypasses the bus (nothing to
/// bound or audit — used for openers and detached relaunches).
pub fn spawn_and_reap(cmd: &mut std::process::Command) -> std::io::Result<()> {
    let mut child = cmd.spawn()?;
    std::thread::spawn(move || {
        if let Err(err) = child.wait() {
            log::warn!("Failed to reap child process: {}", err);
        }
    });
    Ok(())
}

/// Run a command and capture its output, routed through the global command bus
/// (bounded concurrency + audit). Concurrency is enforced per [`Lane`]; the
/// lane defaults to the current thread's (see [`with_lane`]).
///
/// Catches the rare EBADF panic from the standard library's pipe reader under
/// FD pressure and converts it into a normal `io::Error`.
pub fn safe_output(cmd: &mut std::process::Command) -> std::io::Result<std::process::Output> {
    run(CommandSpec::from_command(cmd))
}

/// Like [`safe_output`] but kills the child if it does not finish within
/// `timeout`. Useful for Docker CLI calls that can hang when the daemon is not
/// running.
pub fn safe_output_with_timeout(
    cmd: &mut std::process::Command,
    timeout: std::time::Duration,
) -> std::io::Result<std::process::Output> {
    run(CommandSpec::from_command(cmd).timeout(timeout))
}

/// Open a URL in the default browser and reap the opener process.
pub fn open_url(url: &str) {
    #[cfg(target_os = "linux")]
    let result = spawn_and_reap(command("xdg-open").arg(url));
    #[cfg(target_os = "macos")]
    let result = spawn_and_reap(command("open").arg(url));
    #[cfg(windows)]
    let result = spawn_and_reap(command("cmd").args(["/C", "start", "", url]));
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    let result: std::io::Result<()> = {
        log::warn!("open_url is not supported on this platform: {url:?}");
        Ok(())
    };

    if let Err(e) = result {
        log::warn!("failed to open URL {url:?}: {e}");
    }
}

/// Whether `pid` has exited but not yet been reaped by its parent.
///
/// macOS: `proc_pidinfo(PROC_PIDTBSDINFO)` fills a record for a live process
/// and fails with `ESRCH` once it has exited — including while it is still a
/// zombie and `kill(pid, 0)` reports it present.
#[cfg(target_os = "macos")]
fn is_zombie(pid: u32) -> bool {
    /// `PROC_PIDTBSDINFO` from `<sys/proc_info.h>`.
    const PROC_PIDTBSDINFO: libc::c_int = 3;

    // The record is 136 bytes today; over-size the buffer so a larger one on a
    // future release still fits (too small is an error, too large is fine).
    let mut info = [0u8; 512];
    let written = unsafe {
        // Apple's wrapper collapses the kernel's -1 into a return of 0 and
        // reports the reason through errno, so the return value alone cannot
        // tell "this pid is gone" from "the call failed". Clear errno first so
        // an unrelated earlier failure can't be mistaken for ours.
        *libc::__error() = 0;
        libc::proc_pidinfo(
            pid as libc::c_int,
            PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            info.len() as libc::c_int,
        )
    };
    if written > 0 {
        return false;
    }

    // Only "no such process" means the pid has exited. The other ways this
    // returns 0 — EPERM for a process we may not inspect, ENOMEM for a buffer
    // the kernel considers too small — say nothing about liveness, and we
    // already know from `kill(pid, 0)` that the pid exists. Treat those as
    // running: keeping the old answer beats declaring a live daemon dead and
    // respawning on top of it.
    std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
}

/// Linux: field 3 of `/proc/<pid>/stat` is the state character, `Z` for zombie.
/// The preceding `comm` field is parenthesised and may itself contain spaces
/// and parentheses, so the scan starts after its last `)`.
#[cfg(target_os = "linux")]
fn is_zombie(pid: u32) -> bool {
    // Read bytes, not a String: `comm` is the raw executable name and need not
    // be valid UTF-8, which would otherwise fail the read and report a zombie
    // as running.
    let Ok(stat) = std::fs::read(format!("/proc/{pid}/stat")) else {
        return false;
    };
    let stat = String::from_utf8_lossy(&stat);
    let Some((_, after_comm)) = stat.rsplit_once(')') else {
        return false;
    };
    after_comm.split_whitespace().next() == Some("Z")
}

/// Other unix targets keep the old behaviour: no cheap zombie probe, so a
/// pid in the table counts as alive.
#[cfg(all(unix, not(any(target_os = "macos", target_os = "linux"))))]
fn is_zombie(_pid: u32) -> bool {
    false
}

/// Check whether a process with the given PID is still running.
///
/// A process that has exited but whose parent has not reaped it yet keeps its
/// pid in the table, so `kill(pid, 0)` still succeeds for it. Every caller here
/// means *running* — a zombie answers no requests, serves no port and holds no
/// lock — so zombies count as dead. This matters for processes we spawn
/// ourselves and reap late: the desktop holds its daemon's `Child` handle, so a
/// daemon that exited stayed "alive" to every pid probe until the handle was
/// finally waited on, which made a clean shutdown burn its whole timeout and a
/// crashed daemon look reachable.
///
/// Windows needs no equivalent: its process handle is signalled at exit.
pub fn is_process_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // signal 0 probes for existence without delivering a signal.
        if unsafe { libc::kill(pid as libc::pid_t, 0) } != 0 {
            return false;
        }
        !is_zombie(pid)
    }
    #[cfg(windows)]
    {
        // A process handle is signaled after exit. This avoids treating an
        // actual exit code of 259 (`STILL_ACTIVE`) as forever alive.
        use windows_sys::Win32::Foundation::{CloseHandle, WAIT_TIMEOUT};
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
        };
        unsafe {
            let handle = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
            if handle.is_null() {
                return false;
            }
            let result = WaitForSingleObject(handle, 0);
            CloseHandle(handle);
            result == WAIT_TIMEOUT
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        // Without a cheap liveness probe, assume alive and let callers fall
        // back to their existing connection/time-out paths.
        let _ = pid;
        true
    }
}

/// Raise the soft open-file-descriptor limit toward the hard limit at startup.
///
/// The command bus caps concurrent child processes (~20), but interactive PTY
/// shells, network sockets, watchers and log files all draw on the same
/// per-process FD budget. macOS ships a stingy `RLIMIT_NOFILE` soft default
/// (256) that a busy multiplexer can brush against; raising the soft limit to
/// the hard limit gives PTYs and sockets headroom without changing the bus's
/// own bound. No-op on Windows (no `RLIMIT_NOFILE`).
#[cfg(unix)]
pub fn raise_fd_limit() {
    // SAFETY: plain libc getrlimit/setrlimit calls with a stack-local struct.
    unsafe {
        let mut lim = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        if libc::getrlimit(libc::RLIMIT_NOFILE, &mut lim) != 0 {
            return;
        }
        if lim.rlim_cur >= lim.rlim_max {
            return;
        }
        let target = lim.rlim_max;
        let new = libc::rlimit {
            rlim_cur: target,
            rlim_max: lim.rlim_max,
        };
        if libc::setrlimit(libc::RLIMIT_NOFILE, &new) == 0 {
            log::info!(
                "Raised RLIMIT_NOFILE soft limit {} -> {}",
                lim.rlim_cur,
                target
            );
        }
    }
}

/// No-op on Windows — there is no `RLIMIT_NOFILE`.
#[cfg(not(unix))]
pub fn raise_fd_limit() {}

/// Test helpers for intercepting bus commands without spawning real processes.
#[cfg(any(test, feature = "test-support"))]
pub mod testing {
    use super::*;
    use std::process::Output;

    /// Guard that restores real execution when dropped.
    pub struct MockGuard;

    impl Drop for MockGuard {
        fn drop(&mut self) {
            CommandBus::global().set_mock(None);
        }
    }

    /// Replace real process execution with `f` until the returned guard drops.
    /// Use in tests to assert on submitted commands or return canned output
    /// without touching the OS.
    pub fn mock(
        f: impl Fn(&CommandSpec) -> std::io::Result<Output> + Send + Sync + 'static,
    ) -> MockGuard {
        CommandBus::global().set_mock(Some(Box::new(f)));
        MockGuard
    }
}

#[cfg(test)]
mod tests {
    /// Every bus child must lead its own session, so it has no controlling
    /// terminal to be stopped on. Without this a `git clone` that hits a
    /// credential prompt takes SIGTTIN and hangs forever instead of failing.
    ///
    /// Session id equal to the pid is exactly what `setsid` produces, and it
    /// is also the process-group identity the bus kills on cancel.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_bus_child_leads_its_own_session() {
        let output =
            super::run(super::CommandSpec::new("sh").args(["-c", "ps -o sid= -o pid= -p $$"]))
                .expect("run ps");
        let text = String::from_utf8_lossy(&output.stdout);
        let mut fields = text.split_whitespace();
        let sid: i64 = fields.next().and_then(|f| f.parse().ok()).expect("sid");
        let pid: i64 = fields.next().and_then(|f| f.parse().ok()).expect("pid");
        assert_eq!(sid, pid, "child should be a session leader, got {text:?}");
    }

    use super::*;
    use std::sync::Mutex;
    use std::time::Duration;

    // The bus and its mock slot are process-global, so bus tests must not run
    // concurrently (one test's mock would intercept another's commands).
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn guard() -> std::sync::MutexGuard<'static, ()> {
        TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn safe_output_runs_through_bus() {
        let _g = guard();
        let mut cmd = command("echo");
        cmd.arg("hello");
        let out = safe_output(&mut cmd).expect("echo runs");
        assert!(out.status.success());
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "hello");
    }

    #[test]
    fn from_command_extracts_args_cwd() {
        let _g = guard();
        let mut cmd = command("git");
        cmd.args(["status", "--short"]).current_dir("/tmp");
        let spec = CommandSpec::from_command(&cmd);
        assert_eq!(spec.program, "git");
        assert_eq!(spec.args, vec!["status", "--short"]);
        assert_eq!(spec.cwd.as_deref(), Some(std::path::Path::new("/tmp")));
    }

    #[test]
    fn timeout_kills_slow_command() {
        let _g = guard();
        let spec = CommandSpec::new("sleep")
            .arg("5")
            .timeout(Duration::from_millis(100));
        let err = run(spec).expect_err("should time out");
        assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);
    }

    #[cfg(unix)]
    #[test]
    fn timeout_kills_foreground_tree_without_touching_unrelated_process() {
        let _g = guard();
        let pid_file = tempfile::NamedTempFile::new().expect("pid file");
        let pid_path = pid_file.path().to_string_lossy().into_owned();
        let mut unrelated = std::process::Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .expect("unrelated process");

        let started = std::time::Instant::now();
        let err = run(CommandSpec::new("/bin/sh")
            .args([
                "-c",
                "sleep 30 & echo $! > \"$1\"; wait $!",
                "okena-test",
                &pid_path,
            ])
            .timeout(Duration::from_millis(500)))
        .expect_err("process tree should time out");
        let elapsed = started.elapsed();
        let descendant_pid = read_test_pid(pid_file.path());
        let descendant_dead = wait_for_test_process_exit(descendant_pid, Duration::from_secs(1));
        let unrelated_alive = unrelated.try_wait().expect("probe unrelated").is_none();

        let _ = unrelated.kill();
        let _ = unrelated.wait();
        if !descendant_dead {
            kill_test_process(descendant_pid);
        }

        assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);
        assert!(elapsed < Duration::from_secs(2), "elapsed: {elapsed:?}");
        assert!(descendant_dead, "foreground descendant survived timeout");
        assert!(unrelated_alive, "unrelated process was terminated");
    }

    #[cfg(unix)]
    #[test]
    fn exited_parent_with_background_pipe_descendant_finishes_bounded() {
        let _g = guard();
        let pid_file = tempfile::NamedTempFile::new().expect("pid file");
        let pid_path = pid_file.path().to_string_lossy().into_owned();

        let started = std::time::Instant::now();
        let output = run(CommandSpec::new("/bin/sh").args([
            "-c",
            "printf 'retained\\n'; sleep 30 & echo $! > \"$1\"",
            "okena-test",
            &pid_path,
        ]))
        .expect("parent result");
        let elapsed = started.elapsed();
        let descendant_pid = read_test_pid(pid_file.path());
        let descendant_dead = wait_for_test_process_exit(descendant_pid, Duration::from_secs(1));
        if !descendant_dead {
            kill_test_process(descendant_pid);
        }

        assert!(output.status.success());
        assert_eq!(output.stdout, b"retained\n");
        assert!(elapsed < Duration::from_secs(2), "elapsed: {elapsed:?}");
        assert!(descendant_dead, "background descendant survived collection");
    }

    #[cfg(unix)]
    fn read_test_pid(path: &std::path::Path) -> u32 {
        std::fs::read_to_string(path)
            .expect("read descendant pid")
            .trim()
            .parse()
            .expect("parse descendant pid")
    }

    #[cfg(unix)]
    fn wait_for_test_pid(path: &std::path::Path, timeout: Duration) -> u32 {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if let Ok(contents) = std::fs::read_to_string(path)
                && let Ok(pid) = contents.trim().parse()
            {
                return pid;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for descendant pid"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[cfg(unix)]
    fn wait_for_test_process_exit(pid: u32, timeout: Duration) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        while std::time::Instant::now() < deadline {
            if !is_process_alive(pid) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        !is_process_alive(pid)
    }

    #[cfg(unix)]
    fn kill_test_process(pid: u32) {
        let Ok(pid) = libc::pid_t::try_from(pid) else {
            return;
        };
        // SAFETY: this fallback only targets the PID written by the test child.
        let _ = unsafe { libc::kill(pid, libc::SIGKILL) };
    }

    #[test]
    fn lane_default_is_interactive() {
        let _g = guard();
        assert_eq!(current_lane(), Lane::Interactive);
        with_lane(Lane::Poll, || {
            assert_eq!(current_lane(), Lane::Poll);
            assert_eq!(CommandSpec::new("git").lane, Lane::Poll);
        });
        assert_eq!(current_lane(), Lane::Interactive);
    }

    #[test]
    fn poll_lane_serializes_under_cap() {
        let _g = guard();
        // More submissions than the lane has workers: all must still complete.
        let handles: Vec<_> = (0..12)
            .map(|_| {
                std::thread::spawn(|| {
                    run(CommandSpec::new("true").lane(Lane::Poll)).map(|o| o.status.success())
                })
            })
            .collect();
        for h in handles {
            assert!(h.join().expect("thread").expect("ran"));
        }
    }

    #[test]
    fn cancel_kills_running_command() {
        let _g = guard();
        let handle = CommandBus::global().submit(CommandSpec::new("sleep").arg("30"));
        let cancellation = handle.cancellation();
        // Give the worker a moment to spawn the child, then cancel.
        std::thread::sleep(Duration::from_millis(80));
        cancellation.cancel();
        let err = handle.wait().expect_err("cancelled");
        assert_eq!(err.kind(), std::io::ErrorKind::Interrupted);
    }

    #[cfg(unix)]
    #[test]
    fn cancel_kills_running_process_tree() {
        let _g = guard();
        let pid_file = tempfile::NamedTempFile::new().expect("pid file");
        let pid_path = pid_file.path().to_string_lossy().into_owned();
        let handle = CommandBus::global().submit(CommandSpec::new("/bin/sh").args([
            "-c",
            "sleep 30 & echo $! > \"$1\"; wait $!",
            "okena-test",
            &pid_path,
        ]));
        let descendant_pid = wait_for_test_pid(pid_file.path(), Duration::from_secs(2));

        let started = std::time::Instant::now();
        handle.cancel();
        let err = handle.wait().expect_err("cancelled");
        let elapsed = started.elapsed();
        let descendant_dead = wait_for_test_process_exit(descendant_pid, Duration::from_secs(1));
        if !descendant_dead {
            kill_test_process(descendant_pid);
        }

        assert_eq!(err.kind(), std::io::ErrorKind::Interrupted);
        assert!(elapsed < Duration::from_secs(2), "elapsed: {elapsed:?}");
        assert!(
            descendant_dead,
            "foreground descendant survived cancellation"
        );
    }

    #[test]
    fn cancel_scope_kills_group() {
        let _g = guard();
        let bus = CommandBus::global();
        let a = bus.submit(CommandSpec::new("sleep").arg("30").scope(42));
        let b = bus.submit(CommandSpec::new("sleep").arg("30").scope(42));
        std::thread::sleep(Duration::from_millis(80));
        bus.cancel_scope(42);
        assert_eq!(
            a.wait().unwrap_err().kind(),
            std::io::ErrorKind::Interrupted
        );
        assert_eq!(
            b.wait().unwrap_err().kind(),
            std::io::ErrorKind::Interrupted
        );
    }

    // Synthesizes an `ExitStatus` through the Unix `ExitStatusExt`.
    #[test]
    #[cfg(unix)]
    fn mock_intercepts_without_spawning() {
        use std::os::unix::process::ExitStatusExt;
        let _g = guard();
        let _mock = testing::mock(|spec| {
            assert_eq!(spec.program, "git");
            Ok(std::process::Output {
                status: std::process::ExitStatus::from_raw(0),
                stdout: b"mocked".to_vec(),
                stderr: Vec::new(),
            })
        });
        let mut cmd = command("git");
        cmd.arg("status");
        let out = safe_output(&mut cmd).expect("mock");
        assert_eq!(out.stdout, b"mocked");
    }

    /// A child that exited but has not been `wait`ed on is a zombie: its pid is
    /// still in the table, so `kill(pid, 0)` succeeds. Reporting that as alive
    /// made the desktop's shutdown wait sit out its whole timeout on a corpse
    /// and a crashed daemon look reachable.
    #[cfg(unix)]
    #[test]
    fn exited_but_unreaped_child_is_not_alive() {
        let mut child = std::process::Command::new("/bin/sh")
            .args(["-c", "exit 0"])
            .spawn()
            .expect("spawn");
        let pid = child.id();
        // Let it exit. Deliberately do NOT reap it yet.
        std::thread::sleep(Duration::from_millis(300));

        let alive = is_process_alive(pid);
        let _ = child.wait();

        assert!(
            !alive,
            "an exited, unreaped child must not count as running"
        );
    }

    #[cfg(unix)]
    #[test]
    fn running_processes_are_alive() {
        assert!(is_process_alive(std::process::id()), "this test process");

        let mut child = std::process::Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .expect("spawn");
        std::thread::sleep(Duration::from_millis(300));
        let alive = is_process_alive(child.id());
        let _ = child.kill();
        let _ = child.wait();

        assert!(alive, "a live child must count as running");
    }
}
