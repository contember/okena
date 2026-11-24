//! Shell type — the serializable description of which shell a terminal runs.
//!
//! This is pure data plus pure-string helpers, so it lives in `okena-core` and
//! can be referenced by data-only crates (`okena-state`, `okena-layout`) without
//! pulling in PTY/process machinery. The behavioral part — turning a `ShellType`
//! into a spawnable `portable_pty::CommandBuilder` — lives in `okena-terminal`
//! (see `ShellCommandExt::build_command`).

use serde::{Deserialize, Serialize};

/// Shell type for terminal creation
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type")]
#[derive(Default)]
pub enum ShellType {
    /// Use system default shell (CommandBuilder::new_default_prog())
    #[default]
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
    pub fn resolve_default(
        self,
        project_shell: Option<&ShellType>,
        global_shell: &ShellType,
    ) -> ShellType {
        if self == ShellType::Default {
            project_shell
                .cloned()
                .unwrap_or_else(|| global_shell.clone())
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
                if *core {
                    "pwsh"
                } else {
                    "PS"
                }
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
            ShellType::PowerShell { core } => if *core {
                "pwsh.exe -NoLogo"
            } else {
                "powershell.exe -NoLogo"
            }
            .to_string(),
            #[cfg(windows)]
            ShellType::Wsl { distro } => match distro {
                Some(d) => format!("wsl.exe -d {}", d),
                None => "wsl.exe".to_string(),
            },
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
}

/// A shell choice stored by the Start Page client and resolved by the PTY host.
/// Windows choices remain readable on every client and daemon platform.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum StartPageShell {
    #[default]
    Default,
    Cmd,
    PowerShell {
        #[serde(default)]
        core: bool,
    },
    Wsl {
        #[serde(default)]
        distro: Option<String>,
    },
    Custom {
        path: String,
        #[serde(default)]
        args: Vec<String>,
    },
}

impl StartPageShell {
    pub fn display_name(&self) -> String {
        match self {
            Self::Default => "Project default".to_string(),
            Self::Cmd => "Command Prompt".to_string(),
            Self::PowerShell { core: false } => "Windows PowerShell".to_string(),
            Self::PowerShell { core: true } => "PowerShell Core".to_string(),
            Self::Wsl { distro: None } => "WSL (Default)".to_string(),
            Self::Wsl {
                distro: Some(distro),
            } => format!("WSL ({distro})"),
            Self::Custom { path, .. } => {
                path.rsplit(['/', '\\']).next().unwrap_or(path).to_string()
            }
        }
    }
}

impl From<ShellType> for StartPageShell {
    fn from(shell: ShellType) -> Self {
        match shell {
            ShellType::Default => Self::Default,
            #[cfg(windows)]
            ShellType::Cmd => Self::Cmd,
            #[cfg(windows)]
            ShellType::PowerShell { core } => Self::PowerShell { core },
            #[cfg(windows)]
            ShellType::Wsl { distro } => Self::Wsl { distro },
            ShellType::Custom { path, args } => Self::Custom { path, args },
        }
    }
}

impl TryFrom<StartPageShell> for ShellType {
    type Error = String;

    fn try_from(shell: StartPageShell) -> Result<Self, Self::Error> {
        match shell {
            StartPageShell::Default => Ok(Self::Default),
            #[cfg(windows)]
            StartPageShell::Cmd => Ok(Self::Cmd),
            #[cfg(windows)]
            StartPageShell::PowerShell { core } => Ok(Self::PowerShell { core }),
            #[cfg(windows)]
            StartPageShell::Wsl { distro } => Ok(Self::Wsl { distro }),
            StartPageShell::Custom { path, args } => Ok(Self::Custom { path, args }),
            #[cfg(not(windows))]
            _ => Err(format!("Shell is unavailable: {}", shell.display_name())),
        }
    }
}

/// An explicit shell offered by the daemon. Project default is always available.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartPageShellOption {
    pub shell: StartPageShell,
    pub name: String,
}

/// Shell-quote a string for embedding in a POSIX shell command (sh, bash,
/// zsh). Returns the string as-is if it contains no special characters,
/// otherwise wraps in single quotes with proper escaping. Not for fish, which
/// reads `\\` and `\'` inside single quotes as escapes.
pub fn shell_quote(s: &str) -> String {
    if s.is_empty() {
        return "''".to_string();
    }
    // If it only contains safe characters, no quoting needed. A leading `=`
    // is not safe: zsh expands `=cmd` to the command's path.
    if !s.starts_with('=')
        && s.bytes().all(|b| {
            b.is_ascii_alphanumeric()
                || b == b'/'
                || b == b'.'
                || b == b'-'
                || b == b'_'
                || b == b'='
                || b == b':'
        })
    {
        return s.to_string();
    }
    // Single-quote and escape embedded single quotes
    format!("'{}'", s.replace('\'', "'\\''"))
}

#[cfg(test)]
mod start_page_tests {
    use super::*;

    #[test]
    fn named_wsl_choice_round_trips_independent_of_host() {
        let saved = serde_json::json!({"type": "Wsl", "distro": "Ubuntu"});
        let shell: StartPageShell = serde_json::from_value(saved.clone()).unwrap();
        assert_eq!(shell.display_name(), "WSL (Ubuntu)");
        assert_eq!(serde_json::to_value(&shell).unwrap(), saved);

        #[cfg(windows)]
        assert_eq!(
            ShellType::try_from(shell).unwrap(),
            ShellType::Wsl {
                distro: Some("Ubuntu".to_string()),
            },
        );
        #[cfg(not(windows))]
        assert_eq!(
            ShellType::try_from(shell).unwrap_err(),
            "Shell is unavailable: WSL (Ubuntu)",
        );
    }
}
