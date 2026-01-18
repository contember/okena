use std::process::Command;

/// Backend for persistent terminal sessions
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SessionBackend {
    /// No persistence - direct shell
    None,
    /// Use tmux for session persistence
    Tmux,
    /// Use screen for session persistence
    Screen,
    /// Auto-detect: prefer tmux, fallback to screen, then none (default)
    #[default]
    Auto,
}

impl SessionBackend {
    /// Parse from string (for env variable)
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "tmux" => Self::Tmux,
            "screen" => Self::Screen,
            "none" | "off" | "false" | "0" => Self::None,
            "auto" | "smart" | "on" | "true" | "1" => Self::Auto,
            _ => Self::None,
        }
    }

    /// Load from environment variable TERM_MANAGER_SESSION_BACKEND
    /// Defaults to Auto if not set
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
            Self::Auto => {
                if is_tmux_available() {
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
}

/// Resolved (concrete) backend - no Auto variant
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolvedBackend {
    None,
    Tmux,
    Screen,
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
        }
    }

    /// Kill a session
    pub fn kill_session(&self, session_name: &str) {
        match self {
            Self::None => {}
            Self::Tmux => {
                let _ = Command::new("tmux")
                    .args(["kill-session", "-t", session_name])
                    .output();
            }
            Self::Screen => {
                let _ = Command::new("screen")
                    .args(["-S", session_name, "-X", "quit"])
                    .output();
            }
        }
    }
}

/// Escape a string for safe use in shell commands
#[allow(dead_code)] // Used only on Unix for tmux/screen commands
fn shell_escape(s: &str) -> String {
    // Wrap in single quotes and escape any existing single quotes
    format!("'{}'", s.replace('\'', "'\\''"))
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

/// Check if tmux is available on the system
/// Always returns false on Windows as tmux is not natively available
fn is_tmux_available() -> bool {
    #[cfg(windows)]
    {
        false
    }

    #[cfg(not(windows))]
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

    #[cfg(not(windows))]
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
    }
}
