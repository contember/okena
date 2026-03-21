//! Shell configuration for Windows and cross-platform terminal support
//!
//! Provides shell type detection and command building for different shells:
//! - cmd.exe (Command Prompt)
//! - powershell.exe (Windows PowerShell)
//! - pwsh.exe (PowerShell Core)
//! - WSL (Windows Subsystem for Linux)
//! - Custom shell paths

use portable_pty::CommandBuilder;
use serde::{Deserialize, Serialize};

/// Shell type for terminal creation
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type")]
pub enum ShellType {
    /// Use system default shell (CommandBuilder::new_default_prog())
    Default,

    /// Windows Command Prompt (cmd.exe)
    #[cfg(windows)]
    Cmd,

    /// Windows PowerShell or PowerShell Core
    #[cfg(windows)]
    PowerShell {
        /// Use pwsh.exe (PowerShell Core) instead of powershell.exe
        #[serde(default)]
        core: bool,
    },

    /// Windows Subsystem for Linux
    #[cfg(windows)]
    Wsl {
        /// Specific distro name, or None for default
        #[serde(default)]
        distro: Option<String>,
    },

    /// Custom shell with path and arguments
    Custom {
        path: String,
        #[serde(default)]
        args: Vec<String>,
    },
}

impl Default for ShellType {
    fn default() -> Self {
        ShellType::Default
    }
}

impl ShellType {
    /// Create a shell type that runs a single command via the user's shell.
    /// Uses `$SHELL -ic` on Unix (interactive, so .bashrc/.zshrc is sourced)
    /// and `cmd /C` on Windows.
    pub fn for_command(command: String) -> Self {
        if cfg!(windows) {
            ShellType::Custom {
                path: "cmd".to_string(),
                args: vec!["/C".to_string(), command],
            }
        } else {
            let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
            ShellType::Custom {
                path: shell,
                args: vec!["-ic".to_string(), command],
            }
        }
    }

    /// Resolve `ShellType::Default` into a concrete shell by checking
    /// the project's default shell first, then the global setting.
    /// Non-Default variants are returned unchanged.
    pub fn resolve_default(self, project_shell: Option<&ShellType>, global_shell: &ShellType) -> ShellType {
        if self == ShellType::Default {
            project_shell.cloned().unwrap_or_else(|| global_shell.clone())
        } else {
            self
        }
    }

    /// Get a display name for this shell type
    pub fn display_name(&self) -> String {
        match self {
            ShellType::Default => "System Default".to_string(),
            #[cfg(windows)]
            ShellType::Cmd => "Command Prompt".to_string(),
            #[cfg(windows)]
            ShellType::PowerShell { core: false } => "Windows PowerShell".to_string(),
            #[cfg(windows)]
            ShellType::PowerShell { core: true } => "PowerShell Core".to_string(),
            #[cfg(windows)]
            ShellType::Wsl { distro: None } => "WSL (Default)".to_string(),
            #[cfg(windows)]
            ShellType::Wsl { distro: Some(d) } => format!("WSL ({})", d),
            ShellType::Custom { path, .. } => {
                // Extract filename from path
                std::path::Path::new(path)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(path)
                    .to_string()
            }
        }
    }

    /// Get a short display name for compact UI elements (e.g., shell indicator chips)
    pub fn short_display_name(&self) -> &'static str {
        match self {
            ShellType::Default => "Default",
            #[cfg(windows)]
            ShellType::Cmd => "CMD",
            #[cfg(windows)]
            ShellType::PowerShell { core } => {
                if *core { "pwsh" } else { "PS" }
            }
            #[cfg(windows)]
            ShellType::Wsl { .. } => "WSL",
            ShellType::Custom { .. } => "Custom",
        }
    }

    /// Convert to the full command string (executable + args).
    /// Used by shell_wrapper to produce the correct command to wrap.
    pub fn to_command_string(&self) -> String {
        match self {
            ShellType::Default => "${SHELL:-sh}".to_string(),
            #[cfg(windows)]
            ShellType::Cmd => "cmd.exe".to_string(),
            #[cfg(windows)]
            ShellType::PowerShell { core } => {
                if *core { "pwsh.exe -NoLogo" } else { "powershell.exe -NoLogo" }.to_string()
            }
            #[cfg(windows)]
            ShellType::Wsl { distro } => {
                match distro {
                    Some(d) => format!("wsl.exe -d {}", d),
                    None => "wsl.exe".to_string(),
                }
            }
            ShellType::Custom { path, args } => {
                if args.is_empty() {
                    shell_quote(path)
                } else {
                    let quoted_args: Vec<String> = args.iter().map(|a| shell_quote(a)).collect();
                    format!("{} {}", shell_quote(path), quoted_args.join(" "))
                }
            }
        }
    }

    /// Build a CommandBuilder for this shell type
    pub fn build_command(&self, cwd: &str) -> CommandBuilder {
        match self {
            ShellType::Default => {
                let mut cmd = CommandBuilder::new_default_prog();
                cmd.cwd(cwd);
                cmd
            }
            #[cfg(windows)]
            ShellType::Cmd => {
                let mut cmd = CommandBuilder::new("cmd.exe");
                cmd.cwd(cwd);
                cmd
            }
            #[cfg(windows)]
            ShellType::PowerShell { core } => {
                let exe = if *core { "pwsh.exe" } else { "powershell.exe" };
                let mut cmd = CommandBuilder::new(exe);
                // -NoLogo reduces startup noise
                cmd.arg("-NoLogo");
                cmd.cwd(cwd);
                cmd
            }
            #[cfg(windows)]
            ShellType::Wsl { distro } => {
                let mut cmd = CommandBuilder::new("wsl.exe");
                if let Some(d) = distro {
                    cmd.arg("-d");
                    cmd.arg(d);
                }
                // Convert Windows path to WSL path
                let wsl_path = windows_path_to_wsl(cwd);
                cmd.arg("--cd");
                cmd.arg(&wsl_path);
                cmd
            }
            ShellType::Custom { path, args } => {
                let mut cmd = CommandBuilder::new(path);
                for arg in args {
                    cmd.arg(arg);
                }
                cmd.cwd(cwd);
                cmd
            }
        }
    }
}

/// Shell-quote a string for embedding in a shell command.
/// Returns the string as-is if it contains no special characters,
/// otherwise wraps in single quotes with proper escaping.
fn shell_quote(s: &str) -> String {
    if s.is_empty() {
        return "''".to_string();
    }
    // If it only contains safe characters, no quoting needed
    if s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'/' || b == b'.' || b == b'-' || b == b'_' || b == b'=' || b == b':') {
        return s.to_string();
    }
    // Single-quote and escape embedded single quotes
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Information about an available shell
#[derive(Clone, Debug)]
pub struct AvailableShell {
    pub shell_type: ShellType,
    pub name: String,
    pub available: bool,
}

/// Detect all available shells on the system
pub fn available_shells() -> Vec<AvailableShell> {
    let mut shells = vec![AvailableShell {
        shell_type: ShellType::Default,
        name: "System Default".to_string(),
        available: true,
    }];

    #[cfg(windows)]
    {
        // Command Prompt is always available on Windows
        shells.push(AvailableShell {
            shell_type: ShellType::Cmd,
            name: "Command Prompt".to_string(),
            available: true,
        });

        // Windows PowerShell is always available on modern Windows
        shells.push(AvailableShell {
            shell_type: ShellType::PowerShell { core: false },
            name: "Windows PowerShell".to_string(),
            available: true,
        });

        // Check for PowerShell Core (pwsh.exe)
        let pwsh_available = is_pwsh_available();
        shells.push(AvailableShell {
            shell_type: ShellType::PowerShell { core: true },
            name: "PowerShell Core".to_string(),
            available: pwsh_available,
        });

        // Check for WSL
        let wsl_distros = detect_wsl_distros();
        if !wsl_distros.is_empty() {
            // Add default WSL option
            shells.push(AvailableShell {
                shell_type: ShellType::Wsl { distro: None },
                name: "WSL (Default)".to_string(),
                available: true,
            });

            // Add each specific distro
            for distro in wsl_distros {
                shells.push(AvailableShell {
                    shell_type: ShellType::Wsl {
                        distro: Some(distro.clone()),
                    },
                    name: format!("WSL ({})", distro),
                    available: true,
                });
            }
        }
    }

    #[cfg(not(windows))]
    {
        // On Unix, check for common shells
        let unix_shells = [
            ("/bin/bash", "Bash", "Bourne Again Shell"),
            ("/bin/zsh", "Zsh", "Z Shell"),
            ("/bin/fish", "Fish", "Friendly Interactive Shell"),
            ("/bin/sh", "sh", "Bourne Shell"),
        ];

        for (path, name, _desc) in unix_shells {
            if std::path::Path::new(path).exists() {
                shells.push(AvailableShell {
                    shell_type: ShellType::Custom {
                        path: path.to_string(),
                        args: vec![],
                    },
                    name: name.to_string(),
                    available: true,
                });
            }
        }
    }

    shells
}

/// Check if PowerShell Core (pwsh.exe) is available
#[cfg(windows)]
fn is_pwsh_available() -> bool {
    crate::process::command("pwsh.exe")
        .arg("-Version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Detect installed WSL distributions
#[cfg(windows)]
pub fn detect_wsl_distros() -> Vec<String> {
    let output = match crate::process::command("wsl.exe")
        .args(["-l", "-q"])
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return Vec::new(),
    };

    // WSL outputs UTF-16LE encoded text
    let stdout = &output.stdout;
    let mut distros = Vec::new();

    // Parse UTF-16LE output
    if stdout.len() >= 2 {
        let utf16_chars: Vec<u16> = stdout
            .chunks(2)
            .filter_map(|chunk| {
                if chunk.len() == 2 {
                    Some(u16::from_le_bytes([chunk[0], chunk[1]]))
                } else {
                    None
                }
            })
            .collect();

        if let Ok(text) = String::from_utf16(&utf16_chars) {
            for line in text.lines() {
                let trimmed = line.trim().trim_matches('\0');
                if !trimmed.is_empty() {
                    distros.push(trimmed.to_string());
                }
            }
        }
    }

    distros
}

/// Parse a WSL UNC path into (distro_name, linux_path).
///
/// Recognized formats:
/// - `\\wsl.localhost\Distro\path` or `\\wsl$\Distro\path` (backslash)
/// - `//wsl.localhost/Distro/path` or `//wsl$/Distro/path` (forward-slash)
#[cfg(windows)]
pub fn parse_wsl_unc_path(path: &str) -> Option<(String, String)> {
    let normalized = path.replace('\\', "/");

    // Must start with // (UNC prefix after normalization)
    let rest = normalized.strip_prefix("//")?;

    // Check for wsl.localhost/ or wsl$/
    let after_host = rest.strip_prefix("wsl.localhost/")
        .or_else(|| rest.strip_prefix("wsl$/"))?;

    // Next segment is the distro name
    let (distro, linux_path) = match after_host.find('/') {
        Some(idx) => (&after_host[..idx], &after_host[idx..]),
        None => (after_host, "/"),
    };

    if distro.is_empty() {
        return None;
    }

    Some((distro.to_string(), linux_path.to_string()))
}

/// Convert a Windows path to WSL path format
/// Example: C:\Users\name -> /mnt/c/Users/name
/// Also handles WSL UNC paths: \\wsl.localhost\Ubuntu\home\user -> /home/user
#[cfg(windows)]
pub fn windows_path_to_wsl(windows_path: &str) -> String {
    // Check for WSL UNC paths first
    if let Some((_distro, linux_path)) = parse_wsl_unc_path(windows_path) {
        return linux_path;
    }

    let path = windows_path.replace('\\', "/");

    // Check for drive letter (e.g., C:/)
    if path.len() >= 2 && path.chars().nth(1) == Some(':') {
        if let Some(drive) = path.chars().next() {
            let rest = &path[2..];
            format!("/mnt/{}{}", drive.to_ascii_lowercase(), rest)
        } else {
            // Fallback: should not happen if len >= 2, but return path as-is
            path
        }
    } else {
        // Relative path or already Unix-style
        path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(windows)]
    fn test_windows_path_to_wsl() {
        assert_eq!(
            windows_path_to_wsl("C:\\Users\\test"),
            "/mnt/c/Users/test"
        );
        assert_eq!(
            windows_path_to_wsl("D:\\Projects\\app"),
            "/mnt/d/Projects/app"
        );
        assert_eq!(windows_path_to_wsl("/already/unix"), "/already/unix");
    }

    #[test]
    #[cfg(windows)]
    fn test_wsl_unc_path_conversion() {
        // Backslash UNC paths
        assert_eq!(
            windows_path_to_wsl("\\\\wsl.localhost\\Ubuntu\\home\\user\\project"),
            "/home/user/project"
        );
        assert_eq!(
            windows_path_to_wsl("\\\\wsl$\\Ubuntu\\home\\user"),
            "/home/user"
        );
        // Forward-slash UNC paths
        assert_eq!(
            windows_path_to_wsl("//wsl.localhost/Debian/tmp"),
            "/tmp"
        );
        assert_eq!(
            windows_path_to_wsl("//wsl$/Arch/etc/config"),
            "/etc/config"
        );
    }

    #[test]
    #[cfg(windows)]
    fn test_parse_wsl_unc_path() {
        // wsl.localhost backslash
        let (distro, path) = parse_wsl_unc_path("\\\\wsl.localhost\\Ubuntu\\home\\user").unwrap();
        assert_eq!(distro, "Ubuntu");
        assert_eq!(path, "/home/user");

        // wsl$ backslash
        let (distro, path) = parse_wsl_unc_path("\\\\wsl$\\Debian\\tmp\\file").unwrap();
        assert_eq!(distro, "Debian");
        assert_eq!(path, "/tmp/file");

        // Forward-slash variant
        let (distro, path) = parse_wsl_unc_path("//wsl.localhost/Arch/etc").unwrap();
        assert_eq!(distro, "Arch");
        assert_eq!(path, "/etc");

        // Distro only (no sub-path)
        let (distro, path) = parse_wsl_unc_path("\\\\wsl.localhost\\Ubuntu").unwrap();
        assert_eq!(distro, "Ubuntu");
        assert_eq!(path, "/");

        // Not a WSL UNC path
        assert!(parse_wsl_unc_path("C:\\Users\\test").is_none());
        assert!(parse_wsl_unc_path("/regular/path").is_none());
    }

    #[test]
    fn to_command_string_custom_no_args() {
        let shell = ShellType::Custom {
            path: "/usr/bin/fish".to_string(),
            args: vec![],
        };
        assert_eq!(shell.to_command_string(), "/usr/bin/fish");
    }

    #[test]
    fn test_shell_type_display_name() {
        assert_eq!(ShellType::Default.display_name(), "System Default");

        let custom = ShellType::Custom {
            path: "/bin/bash".to_string(),
            args: vec![],
        };
        assert_eq!(custom.display_name(), "bash");
    }
}
