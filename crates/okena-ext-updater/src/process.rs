/// Create a [`std::process::Command`] that does **not** flash a console
/// window on Windows.
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

/// Get the updates directory for the active profile.
pub fn get_config_dir() -> std::path::PathBuf {
    if let Some(p) = okena_core::profiles::try_current() {
        p.updates_dir()
    } else {
        dirs::config_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join("okena")
    }
}
