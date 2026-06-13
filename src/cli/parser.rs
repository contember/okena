//! clap command tree for the agent-friendly CLI.
//!
//! [`Cli`] is the top-level parser; [`Command`] enumerates every subcommand.
//! [`subcommand_names`] feeds the gate in `try_handle_cli` so the GUI / profile
//! launch path stays untouched for anything that isn't one of our commands.

use clap::{Parser, Subcommand};

/// Okena CLI — control a running Okena instance over its remote HTTP API.
#[derive(Parser)]
#[command(
    name = "okena",
    about = "Control a running Okena instance",
    disable_help_subcommand = false,
    // The binary also launches the GUI; only the subcommands below are CLI.
    after_help = "Default output is tab-separated (grep/awk friendly). Use --json for structured JSON.\nAuthentication is automatic on first use."
)]
pub struct Cli {
    /// Target window for per-window commands ("main", a full window id, or a
    /// unique id prefix). When omitted, the server uses the focused window.
    #[arg(long, global = true)]
    pub window: Option<String>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    // ── KEPT (reimplemented under clap) ──────────────────────────────────────
    /// Generate a pairing code for remote clients
    Pair,
    /// Server health check
    Health {
        #[arg(long)]
        json: bool,
    },
    /// Print raw workspace state (JSON)
    State,
    /// Execute a raw action (JSON ActionRequest)
    Action {
        /// The JSON ActionRequest body
        json: String,
    },
    /// List services and their status
    Services {
        /// Optional project filter (id / name)
        project: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Start / stop / restart a service
    Service {
        #[command(subcommand)]
        cmd: ServiceCmd,
    },
    /// Identify the current terminal and project (uses $OKENA_TERMINAL_ID)
    Whoami {
        #[arg(long)]
        json: bool,
    },

    // ── Orientation ──────────────────────────────────────────────────────────
    /// Compact overview of windows, projects and layout
    Ls {
        #[arg(long)]
        json: bool,
    },

    // ── Projects ─────────────────────────────────────────────────────────────
    /// Project operations
    Project {
        #[command(subcommand)]
        cmd: ProjectCmd,
    },
    /// Worktree operations
    Worktree {
        #[command(subcommand)]
        cmd: WorktreeCmd,
    },
    /// Folder operations
    Folder {
        #[command(subcommand)]
        cmd: FolderCmd,
    },
    /// Terminal & layout operations
    Term {
        #[command(subcommand)]
        cmd: TermCmd,
    },

    // ── I/O (the agent loop) ─────────────────────────────────────────────────
    /// Send raw text to a terminal (no trailing newline)
    Send {
        /// Terminal address (id, project/name, or project:index)
        terminal: String,
        /// Text to send (joined with spaces)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true)]
        text: Vec<String>,
    },
    /// Run a command in a terminal (sends text + Enter)
    Run {
        /// Terminal address (id, project/name, or project:index)
        terminal: String,
        /// Command to run (joined with spaces)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true)]
        command: Vec<String>,
    },
    /// Send a special key to a terminal
    Key {
        /// Terminal address (id, project/name, or project:index)
        terminal: String,
        /// Key name (e.g. enter, esc, tab, up, ctrl-c)
        key: String,
    },
    /// Read the visible content of a terminal
    Read {
        /// Terminal address (id, project/name, or project:index)
        terminal: String,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
pub enum ServiceCmd {
    /// Start a service
    Start {
        name: String,
        project: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Stop a service
    Stop {
        name: String,
        project: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Restart a service
    Restart {
        name: String,
        project: Option<String>,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
pub enum ProjectCmd {
    /// Add a project at <path>
    Add {
        /// Path to the project directory (relative paths resolve against CWD)
        path: String,
        /// Display name (defaults to the path's basename)
        #[arg(long)]
        name: Option<String>,
        /// Add hidden (not shown in the overview)
        #[arg(long)]
        hidden: bool,
        /// Move into a folder (by name or id) after adding
        #[arg(long)]
        folder: Option<String>,
    },
    /// Remove a project (unlinks it from Okena; the folder on disk is kept)
    Rm {
        /// Project (id / name / path)
        project: String,
    },
    /// Show a project in the overview
    Show {
        /// Project (id / name / path)
        project: String,
    },
    /// Hide a project from the overview
    Hide {
        /// Project (id / name / path)
        project: String,
    },
    /// Rename a project
    Rename {
        /// Project (id / name / path)
        project: String,
        name: String,
    },
    /// Set a project's color
    Color {
        /// Project (id / name / path)
        project: String,
        /// Color (default, red, orange, yellow, lime, green, teal, cyan, blue, indigo, purple, pink)
        color: String,
    },
    /// Focus a project's first terminal
    Focus {
        /// Project (id / name / path)
        project: String,
    },
}

#[derive(Subcommand)]
pub enum WorktreeCmd {
    /// Create a worktree from a branch
    Add {
        /// Parent project (id / name / path)
        project: String,
        /// Branch name
        branch: String,
        /// Create the branch if it doesn't exist
        #[arg(long)]
        new_branch: bool,
    },
    /// Remove a worktree project
    Rm {
        /// Worktree project (id / name / path)
        worktree: String,
        #[arg(long)]
        force: bool,
    },
}

#[derive(Subcommand)]
pub enum FolderCmd {
    /// Create a folder
    Add { name: String },
    /// Delete a folder
    Rm {
        /// Folder (id / name)
        folder: String,
    },
    /// Rename a folder
    Rename {
        /// Folder (id / name)
        folder: String,
        name: String,
    },
}

#[derive(Subcommand)]
pub enum TermCmd {
    /// List terminals
    Ls {
        /// Optional project filter (id / name / path)
        project: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Create a new terminal in a project
    New {
        /// Project (id / name / path)
        project: String,
    },
    /// Close a terminal
    Close {
        /// Terminal address (id, project/name, or project:index)
        terminal: String,
    },
    /// Focus a terminal
    Focus {
        /// Terminal address (id, project/name, or project:index)
        terminal: String,
    },
    /// Rename a terminal
    Rename {
        /// Terminal address (id, project/name, or project:index)
        terminal: String,
        name: String,
    },
    /// Split a terminal horizontally (h) or vertically (v)
    Split {
        /// Terminal address (id, project/name, or project:index)
        terminal: String,
        /// Direction: h (horizontal) or v (vertical)
        direction: String,
    },
    /// Add a tab next to a terminal
    Tab {
        /// Terminal address (id, project/name, or project:index)
        terminal: String,
    },
    /// Toggle a terminal's minimized state
    Minimize {
        /// Terminal address (id, project/name, or project:index)
        terminal: String,
    },
    /// Fullscreen a terminal (or exit fullscreen with --off)
    Fullscreen {
        /// Terminal address (id, project/name, or project:index)
        terminal: String,
        /// Exit fullscreen instead
        #[arg(long)]
        off: bool,
    },
}

/// The set of top-level subcommand names the CLI claims. Used by the gate in
/// `try_handle_cli`: if `args[1]` isn't one of these (or a help flag), we hand
/// control back to GUI/profile launch.
pub fn subcommand_names() -> &'static [&'static str] {
    &[
        "pair", "health", "state", "action", "services", "service", "whoami", "ls", "project",
        "worktree", "folder", "term", "send", "run", "key", "read",
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory as _;

    #[test]
    fn command_tree_is_well_formed() {
        // clap runs its internal consistency debug-asserts here; a malformed
        // command tree would panic.
        Cli::command().debug_assert();
    }

    #[test]
    fn subcommand_names_cover_the_tree() {
        // Every declared top-level subcommand must appear in subcommand_names()
        // so the gate in try_handle_cli engages for it.
        let declared: Vec<String> = Cli::command()
            .get_subcommands()
            .map(|c| c.get_name().to_string())
            .collect();
        for name in &declared {
            assert!(
                subcommand_names().contains(&name.as_str()),
                "subcommand '{name}' missing from subcommand_names()"
            );
        }
    }

    #[test]
    fn parses_representative_commands() {
        // A spread of forms, including the global --window flag and trailing args.
        assert!(Cli::try_parse_from(["okena", "ls", "--json"]).is_ok());
        assert!(Cli::try_parse_from(["okena", "term", "split", "p/sh", "h"]).is_ok());
        assert!(
            Cli::try_parse_from(["okena", "project", "focus", "Proj", "--window", "main"]).is_ok()
        );
        assert!(Cli::try_parse_from(["okena", "send", "t1", "echo", "hi"]).is_ok());
        assert!(Cli::try_parse_from(["okena", "run", "t1", "ls", "-la"]).is_ok());
        assert!(Cli::try_parse_from(["okena", "key", "t1", "ctrl-c"]).is_ok());
        // Missing required positional → error.
        assert!(Cli::try_parse_from(["okena", "term", "split", "p/sh"]).is_err());
    }
}
