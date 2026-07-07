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

pub use bus::{current_lane, with_lane, CommandBus, CommandHandle, CommandSpec, Lane};

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

/// Check whether a process with the given PID is still running.
pub fn is_process_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // signal 0 probes for existence without delivering a signal.
        unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
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
                lim.rlim_cur, target
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
        // Give the worker a moment to spawn the child, then cancel.
        std::thread::sleep(Duration::from_millis(80));
        handle.cancel();
        let err = handle.wait().expect_err("cancelled");
        assert_eq!(err.kind(), std::io::ErrorKind::Interrupted);
    }

    #[test]
    fn cancel_scope_kills_group() {
        let _g = guard();
        let bus = CommandBus::global();
        let a = bus.submit(CommandSpec::new("sleep").arg("30").scope(42));
        let b = bus.submit(CommandSpec::new("sleep").arg("30").scope(42));
        std::thread::sleep(Duration::from_millis(80));
        bus.cancel_scope(42);
        assert_eq!(a.wait().unwrap_err().kind(), std::io::ErrorKind::Interrupted);
        assert_eq!(b.wait().unwrap_err().kind(), std::io::ErrorKind::Interrupted);
    }

    #[test]
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
}
