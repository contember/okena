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

/// Open a URL in the default browser. Fire-and-forget (spawn, don't wait).
pub fn open_url(url: &str) {
    #[cfg(target_os = "linux")]
    {
        let _ = command("xdg-open").arg(url).spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = command("open").arg(url).spawn();
    }
    #[cfg(windows)]
    {
        let _ = command("cmd").args(["/c", "start", url]).spawn();
    }
}

/// Run `Command::output()` while catching panics from the standard library's
/// internal `read2().unwrap()`, which can panic with EBADF under rare race
/// conditions (e.g. FD pressure during PTY shutdown). Converts the panic
/// into a normal `io::Error` so callers handle it gracefully.
pub fn safe_output(cmd: &mut std::process::Command) -> std::io::Result<std::process::Output> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| cmd.output())) {
        Ok(result) => result,
        Err(panic) => {
            let msg = if let Some(s) = panic.downcast_ref::<String>() {
                s.clone()
            } else if let Some(s) = panic.downcast_ref::<&str>() {
                s.to_string()
            } else {
                "unknown panic".to_string()
            };
            log::error!("Command::output() panicked: {}", msg);
            Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("Command::output() panicked: {}", msg),
            ))
        }
    }
}
