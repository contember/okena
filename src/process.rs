pub use okena_core::process::{command, safe_output};

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
