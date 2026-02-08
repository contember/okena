use serde::{Deserialize, Serialize};
#[cfg(unix)]
use std::process::Command;

/// Backend for persistent terminal sessions
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SessionBackend {
    /// No persistence - direct shell
    None,
    /// Use tmux for session persistence
    Tmux,
    /// Use screen for session persistence
    Screen,
    /// Use dtach for minimal session persistence (no scrollback management)
    Dtach,
    /// Auto-detect: prefer dtach, fallback to tmux, screen, then none (default)
    #[default]
    Auto,
}

impl SessionBackend {
    /// Parse from string (for env variable override)
    #[allow(dead_code)]
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "tmux" => Self::Tmux,
            "screen" => Self::Screen,
            "dtach" => Self::Dtach,
            "none" | "off" | "false" | "0" => Self::None,
            "auto" | "smart" | "on" | "true" | "1" => Self::Auto,
            _ => Self::None,
        }
    }

    /// Load from environment variable TERM_MANAGER_SESSION_BACKEND
    /// Defaults to Auto if not set
    #[allow(dead_code)]
    pub fn from_env() -> Self {
        std::env::var("TERM_MANAGER_SESSION_BACKEND")
            .map(|s| Self::from_str(&s))
            .unwrap_or_default()
    }

    /// Resolve Auto to a concrete backend based on availability
    pub fn resolve(self) -> ResolvedBackend {
        match self {
            Self::None => ResolvedBackend::None,
            Self::Tmux => {
                if is_tmux_available() {
                    ResolvedBackend::Tmux
                } else {
                    log::warn!("tmux requested but not available, falling back to none");
                    ResolvedBackend::None
                }
            }
            Self::Screen => {
                if is_screen_available() {
                    ResolvedBackend::Screen
                } else {
                    log::warn!("screen requested but not available, falling back to none");
                    ResolvedBackend::None
                }
            }
            Self::Dtach => {
                if is_dtach_available() {
                    ResolvedBackend::Dtach
                } else {
                    log::warn!("dtach requested but not available, falling back to none");
                    ResolvedBackend::None
                }
            }
            Self::Auto => {
                // Prefer dtach (minimal, no scrollback interference)
                // then tmux, then screen
                if is_dtach_available() {
                    log::info!("Auto-detected dtach for session persistence");
                    ResolvedBackend::Dtach
                } else if is_tmux_available() {
                    log::info!("Auto-detected tmux for session persistence");
                    ResolvedBackend::Tmux
                } else if is_screen_available() {
                    log::info!("Auto-detected screen for session persistence");
                    ResolvedBackend::Screen
                } else {
                    log::info!("No session backend available, sessions won't persist");
                    ResolvedBackend::None
                }
            }
        }
    }

    /// Get display name for UI
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::None => "None (Direct Shell)",
            Self::Auto => "Auto (dtach > tmux > screen)",
            Self::Tmux => "tmux",
            Self::Screen => "screen",
            Self::Dtach => "dtach (minimal)",
        }
    }

    /// Get all variants for UI dropdown
    pub fn all_variants() -> &'static [SessionBackend] {
        &[
            SessionBackend::Auto,
            SessionBackend::Dtach,
            SessionBackend::Tmux,
            SessionBackend::Screen,
            SessionBackend::None,
        ]
    }
}

/// Resolved (concrete) backend - no Auto variant
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolvedBackend {
    None,
    Tmux,
    Screen,
    Dtach,
}

impl ResolvedBackend {
    /// Check if this backend supports session persistence
    pub fn supports_persistence(&self) -> bool {
        !matches!(self, Self::None)
    }

    /// Generate a session name for a terminal ID
    /// Uses a prefix to avoid conflicts with user sessions
    pub fn session_name(&self, terminal_id: &str) -> String {
        // Use short prefix + first 8 chars of UUID to keep it manageable
        let short_id = if terminal_id.len() > 8 {
            &terminal_id[..8]
        } else {
            terminal_id
        };
        format!("tm-{}", short_id)
    }

    /// Get the socket path for dtach sessions
    /// Returns None for non-dtach backends
    #[allow(dead_code)]
    pub fn socket_path(&self, terminal_id: &str) -> Option<std::path::PathBuf> {
        if !matches!(self, Self::Dtach) {
            return None;
        }
        Some(get_dtach_socket_path(terminal_id))
    }

    /// Build the command to create or attach to a session
    /// Returns (program, args) tuple
    #[allow(dead_code)] // Used only on Unix
    pub fn build_command(&self, session_name: &str, cwd: &str) -> Option<(String, Vec<String>)> {
        match self {
            Self::None => None,
            Self::Tmux => {
                // Use sh -c to properly chain tmux commands
                // \; is tmux command separator - since args are passed directly via CommandBuilder
                // (not through shell parsing), we only need single escape level
                // -A: attach if exists, create if not
                // -s: session name
                // -c: start directory
                // set status off: hide tmux status bar (we have our own UI)
                // set mouse on: enable mouse for scrolling
                // set automatic-rename off: prevent shell from overwriting window name
                // rename-window: set meaningful window name from directory
                let window_name = extract_dir_name(cwd);
                let tmux_cmd = format!(
                    "tmux new-session -A -s {} -c {} \\; set status off \\; set mouse on \\; set-window-option automatic-rename off \\; rename-window {}",
                    shell_escape(session_name),
                    shell_escape(cwd),
                    shell_escape(&window_name)
                );
                Some((
                    "sh".to_string(),
                    vec!["-c".to_string(), tmux_cmd],
                ))
            }
            Self::Screen => {
                // screen -D -R <name>
                // -D -R: reattach if exists, create if not (and detach other attached sessions)
                // Note: screen doesn't have a direct way to set cwd, we'll handle that separately
                Some((
                    "screen".to_string(),
                    vec![
                        "-D".to_string(),
                        "-R".to_string(),
                        session_name.to_string(),
                    ],
                ))
            }
            Self::Dtach => {
                // dtach -A <socket> -E -r winch <shell>
                // -A: attach if exists, create if not
                // -E: disable detach character (^\ won't detach)
                // -r winch: use SIGWINCH for redraw (needed for apps like less, vim)
                //
                // We use sh -c to:
                // 1. Create the socket directory if needed
                // 2. cd to the working directory
                // 3. Run dtach with the user's shell
                let socket_path = get_dtach_socket_path(session_name);
                let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());

                let parent = socket_path.parent().and_then(|p| p.to_str())?;
                let socket = socket_path.to_str()?;
                let dtach_cmd = format!(
                    "mkdir -p {} && cd {} && exec dtach -A {} -E -r winch {}",
                    shell_escape(parent),
                    shell_escape(cwd),
                    shell_escape(socket),
                    shell_escape(&shell)
                );
                Some(("sh".to_string(), vec!["-c".to_string(), dtach_cmd]))
            }
        }
    }

    /// Kill a session
    pub fn kill_session(&self, session_name: &str) {
        match self {
            Self::None => {}
            Self::Tmux => {
                #[cfg(target_os = "macos")]
                let _ = Command::new("tmux")
                    .args(["kill-session", "-t", session_name])
                    .env("PATH", get_extended_path())
                    .output();

                #[cfg(all(unix, not(target_os = "macos")))]
                let _ = Command::new("tmux")
                    .args(["kill-session", "-t", session_name])
                    .output();
            }
            Self::Screen => {
                #[cfg(target_os = "macos")]
                let _ = Command::new("screen")
                    .args(["-S", session_name, "-X", "quit"])
                    .env("PATH", get_extended_path())
                    .output();

                #[cfg(all(unix, not(target_os = "macos")))]
                let _ = Command::new("screen")
                    .args(["-S", session_name, "-X", "quit"])
                    .output();
            }
            Self::Dtach => {
                let socket_path = get_dtach_socket_path(session_name);
                if socket_path.exists() {
                    #[cfg(unix)]
                    {
                        if let Ok(output) = Command::new("lsof")
                            .args(["-t", "-U"])
                            .arg(&socket_path)
                            .output()
                        {
                            if let Ok(pid_str) = String::from_utf8(output.stdout) {
                                for line in pid_str.lines() {
                                    if let Ok(pid) = line.trim().parse::<i32>() {
                                        unsafe {
                                            libc::kill(pid, libc::SIGTERM);
                                        }
                                        log::debug!("Sent SIGTERM to dtach process {} for session {}", pid, session_name);
                                    }
                                }
                            }
                        }
                    }
                    let _ = std::fs::remove_file(&socket_path);
                    log::debug!("Removed dtach socket: {:?}", socket_path);
                }
            }
        }
    }
}

/// Escape a string for safe use in shell commands
#[allow(dead_code)] // Used only on Unix for tmux/screen/dtach commands
fn shell_escape(s: &str) -> String {
    // Wrap in single quotes and escape any existing single quotes
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Get the socket directory for dtach sessions
#[allow(dead_code)]
fn get_dtach_socket_dir() -> std::path::PathBuf {
    // Use XDG_RUNTIME_DIR if available (Linux), otherwise fall back to temp dir
    // XDG_RUNTIME_DIR is preferred as it's user-specific and cleaned on logout
    if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
        std::path::PathBuf::from(runtime_dir).join("okena")
    } else {
        // Fallback: /tmp/okena-<uid> for security
        #[cfg(unix)]
        {
            let uid = unsafe { libc::getuid() };
            std::path::PathBuf::from(format!("/tmp/okena-{}", uid))
        }
        #[cfg(not(unix))]
        {
            std::env::temp_dir().join("okena")
        }
    }
}

/// Get the socket path for a specific dtach session
#[allow(dead_code)]
fn get_dtach_socket_path(session_name: &str) -> std::path::PathBuf {
    get_dtach_socket_dir().join(format!("{}.sock", session_name))
}

/// Extract directory name from a path for use as window name
#[allow(dead_code)] // Used only on Unix for tmux window naming
fn extract_dir_name(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("terminal")
        .to_string()
}

/// Get extended PATH for macOS app bundles
/// App bundles start with minimal PATH (/usr/bin:/bin:/usr/sbin:/sbin)
/// and don't include Homebrew or MacPorts paths where tmux/screen are typically installed
#[cfg(target_os = "macos")]
pub fn get_extended_path() -> String {
    let current_path = std::env::var("PATH").unwrap_or_default();
    let extra_paths = [
        "/opt/homebrew/bin",      // Homebrew on Apple Silicon
        "/usr/local/bin",         // Homebrew on Intel / manual installs
        "/opt/local/bin",         // MacPorts
        "/usr/local/sbin",
        "/opt/homebrew/sbin",
    ];

    // Prepend extra paths to current PATH
    let mut paths: Vec<&str> = extra_paths.iter().copied().collect();
    if !current_path.is_empty() {
        paths.push(&current_path);
    }
    paths.join(":")
}

/// Check if dtach is available on the system
/// Always returns false on Windows as dtach is not natively available
fn is_dtach_available() -> bool {
    #[cfg(windows)]
    {
        false
    }

    #[cfg(target_os = "macos")]
    {
        Command::new("dtach")
            .arg("-v")
            .env("PATH", get_extended_path())
            .output()
            // dtach -v exits with 0 and prints version
            .map(|o| o.status.success() || !o.stdout.is_empty() || !o.stderr.is_empty())
            .unwrap_or(false)
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        Command::new("dtach")
            .arg("-v")
            .output()
            // dtach -v exits with 0 and prints version
            .map(|o| o.status.success() || !o.stdout.is_empty() || !o.stderr.is_empty())
            .unwrap_or(false)
    }
}

/// Check if tmux is available on the system
/// Always returns false on Windows as tmux is not natively available
fn is_tmux_available() -> bool {
    #[cfg(windows)]
    {
        false
    }

    #[cfg(target_os = "macos")]
    {
        Command::new("tmux")
            .arg("-V")
            .env("PATH", get_extended_path())
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        Command::new("tmux")
            .arg("-V")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
}

/// Check if screen is available on the system
/// Always returns false on Windows as screen is not natively available
fn is_screen_available() -> bool {
    #[cfg(windows)]
    {
        false
    }

    #[cfg(target_os = "macos")]
    {
        Command::new("screen")
            .arg("-v")
            .env("PATH", get_extended_path())
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        Command::new("screen")
            .arg("-v")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_backend() {
        assert_eq!(SessionBackend::from_str("tmux"), SessionBackend::Tmux);
        assert_eq!(SessionBackend::from_str("TMUX"), SessionBackend::Tmux);
        assert_eq!(SessionBackend::from_str("screen"), SessionBackend::Screen);
        assert_eq!(SessionBackend::from_str("dtach"), SessionBackend::Dtach);
        assert_eq!(SessionBackend::from_str("DTACH"), SessionBackend::Dtach);
        assert_eq!(SessionBackend::from_str("none"), SessionBackend::None);
        assert_eq!(SessionBackend::from_str("auto"), SessionBackend::Auto);
        assert_eq!(SessionBackend::from_str("smart"), SessionBackend::Auto);
        assert_eq!(SessionBackend::from_str("invalid"), SessionBackend::None);
    }

    #[test]
    fn test_session_name() {
        let backend = ResolvedBackend::Tmux;
        let name = backend.session_name("12345678-1234-1234-1234-123456789012");
        assert_eq!(name, "tm-12345678");

        // Dtach uses same naming scheme
        let dtach_backend = ResolvedBackend::Dtach;
        let dtach_name = dtach_backend.session_name("12345678-1234-1234-1234-123456789012");
        assert_eq!(dtach_name, "tm-12345678");
    }

    #[test]
    fn test_dtach_socket_path() {
        let backend = ResolvedBackend::Dtach;
        // socket_path expects terminal_id directly, not session_name
        let path = backend.socket_path("tm-12345678");
        assert!(path.is_some());
        let path = path.unwrap();
        assert!(path.to_string_lossy().contains("tm-12345678.sock"));

        // Non-dtach backends should return None
        let tmux_backend = ResolvedBackend::Tmux;
        assert!(tmux_backend.socket_path("tm-12345678").is_none());
    }

    #[test]
    fn test_dtach_build_command() {
        let backend = ResolvedBackend::Dtach;
        let result = backend.build_command("test-session", "/home/user");
        assert!(result.is_some());
        let (program, args) = result.unwrap();
        assert_eq!(program, "sh");
        assert_eq!(args.len(), 2);
        assert_eq!(args[0], "-c");
        assert!(args[1].contains("dtach -A"));
        assert!(args[1].contains("-E -r winch"));
    }
}
