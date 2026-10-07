//! clap command tree for the agent-friendly CLI.
//!
//! [`Cli`] is the top-level parser; [`Command`] enumerates every subcommand.
//! [`subcommand_names`] feeds the gate in `try_handle_cli` so the GUI / profile
//! launch path stays untouched for anything that isn't one of our commands.

use clap::{ArgGroup, Args, Parser, Subcommand};

/// Okena CLI — control a running Okena instance over its remote HTTP API.
#[derive(Parser)]
#[command(
    name = "okena",
    version,
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
        /// Output JSON instead of the default plain text
        #[arg(long)]
        json: bool,
    },
    /// Print raw workspace state (JSON)
    State,
    /// Execute a raw action (JSON ActionRequest)
    ///
    /// Escape hatch for actions without a dedicated subcommand. The body is a
    /// snake_case-tagged object like `{"action":"focus_terminal", ...}`. See
    /// `okena skill show` for the action surface and `okena state` for the ids.
    Action {
        /// The JSON ActionRequest body
        json: String,
    },
    /// List services and their status
    Services {
        /// Optional project filter (id / name)
        project: Option<String>,
        /// Output JSON instead of the default plain text
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
        /// Output JSON instead of the default plain text
        #[arg(long)]
        json: bool,
    },

    // ── Orientation ──────────────────────────────────────────────────────────
    /// Compact overview of windows, projects and layout
    Ls {
        /// Output JSON instead of the default plain text
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
    /// Inspect the mission assigned to this terminal or agent conversation
    #[command(
        after_help = "Mission arguments accept an exact ID or a unique case-insensitive title.\nExamples:\n  okena mission list --json\n  okena mission create \"CSV export\" --goal \"Export records\" --current-terminal\n  okena mission attach <MISSION> --agent claude-code --session-id <UUID>\n  okena mission show <MISSION> --json\n\nMembership selectors are explicit and mutually exclusive. --current-terminal reads $OKENA_TERMINAL_ID; conversations always require their own identity. Mutations print the mission ID, or {\"mission_id\":\"...\"} with --json."
    )]
    Mission {
        #[command(subcommand)]
        cmd: MissionCmd,
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
    ///
    /// With --wait the command finishes synchronously and the CLI exits with
    /// its status. Because the command is a trailing arg, --wait/--timeout must
    /// come *before* the terminal: `okena run --wait <term> <cmd>`.
    Run {
        /// Block until the command finishes, then exit with its status.
        /// Non-interactive commands only (appends a completion marker).
        #[arg(long)]
        wait: bool,
        /// With --wait: seconds to wait for completion before giving up.
        #[arg(long, default_value_t = 300)]
        timeout: u64,
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
        /// Key name: enter, esc, tab, up/down/left/right, home, end, pageup,
        /// pagedown, backspace, delete, or a ctrl-<a-z> chord (e.g. ctrl-c, ctrl-l)
        key: String,
    },
    /// Read the visible content of a terminal
    Read {
        /// Terminal address (id, project/name, or project:index)
        terminal: String,
        /// Output JSON instead of the default plain text
        #[arg(long)]
        json: bool,
    },

    /// Print or install the agent skill (a concise CLI reference for agents)
    Skill {
        #[command(subcommand)]
        cmd: SkillCmd,
    },

    /// Inspect or change app settings
    Settings {
        #[command(subcommand)]
        cmd: SettingsCmd,
    },
    /// Inspect or change the theme (incl. editing a custom theme)
    Theme {
        #[command(subcommand)]
        cmd: ThemeCmd,
    },
    /// List or invoke command-palette actions
    #[command(name = "command")]
    Cmd {
        #[command(subcommand)]
        cmd: PaletteCmd,
    },
    /// Inspect updates or revert to an older stable release
    Update {
        #[command(subcommand)]
        cmd: UpdateCmd,
    },
}

#[derive(Subcommand)]
pub enum MissionCmd {
    /// List all missions on the selected daemon
    List {
        /// Output structured missions
        #[arg(long, conflicts_with = "quiet")]
        json: bool,
        /// Print only mission IDs
        #[arg(short, long, conflicts_with = "json")]
        quiet: bool,
    },
    /// Show a mission's full membership and last-reported attachments
    Show {
        /// Exact mission ID or unique case-insensitive title
        mission: String,
        #[arg(long)]
        json: bool,
    },
    /// Create a mission; prints its ID (membership is optional and explicit)
    Create {
        title: String,
        #[arg(long)]
        goal: Option<String>,
        /// Home context, addressed by project ID, name or path
        #[arg(long)]
        home_project: Option<String>,
        #[command(flatten)]
        member: MissionMemberArgs,
        #[arg(long)]
        json: bool,
    },
    /// Attach a member; conflicting primary membership requires move
    Attach(MissionMembershipArgs),
    /// Detach a member; respects the daemon's persistent exclusions
    Detach(MissionMembershipArgs),
    /// Move a member to the target mission
    Move(MissionMembershipArgs),
    /// Read a bounded mission briefing without changing membership or attention
    #[command(
        after_help = "Examples:\n  okena mission context\n  okena mission context --json\n  okena mission context --agent claude-code --session-id <UUID>\n\n--claude-hook reads Claude Code event JSON from stdin and emits hook JSON only when context changes. Hook failures never block the agent."
    )]
    Context(MissionContextArgs),
}

#[derive(Args)]
pub struct MissionMembershipArgs {
    /// Exact mission ID or unique case-insensitive title
    #[arg(requires = "mission_member")]
    pub mission: String,
    #[command(flatten)]
    pub member: MissionMemberArgs,
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
#[group(skip)]
#[command(group(ArgGroup::new("mission_member")
    .args(["current_terminal", "terminal", "repository", "worktree", "agent"])
    .multiple(false)))]
pub struct MissionMemberArgs {
    /// Select this pane using $OKENA_TERMINAL_ID
    #[arg(long)]
    pub current_terminal: bool,
    /// Terminal ID, project/name or project:index
    #[arg(long)]
    pub terminal: Option<String>,
    /// Repository project ID, name or path (does not assign its terminals)
    #[arg(long)]
    pub repository: Option<String>,
    /// Worktree project ID, name or path
    #[arg(long)]
    pub worktree: Option<String>,
    /// Harness ID for the conversation; requires its own --session-id
    #[arg(long, requires = "session_id")]
    pub agent: Option<String>,
    /// Harness-native conversation ID, never inferred from the pane's previous agent
    #[arg(long, requires = "agent")]
    pub session_id: Option<String>,
}

#[derive(Args)]
pub struct MissionContextArgs {
    /// Exact daemon-local terminal ID; defaults to $OKENA_TERMINAL_ID
    #[arg(long)]
    pub terminal: Option<String>,
    /// Current harness ID; pair with --session-id to avoid a previous pane session
    #[arg(long, requires = "session_id", conflicts_with = "claude_hook")]
    pub agent: Option<String>,
    /// Current harness-native conversation ID; preserves its mission when resumed elsewhere
    #[arg(long, requires = "agent", conflicts_with = "claude_hook")]
    pub session_id: Option<String>,
    /// Output structured context instead of the text briefing
    #[arg(long, conflicts_with = "claude_hook")]
    pub json: bool,
    /// Read Claude hook JSON from stdin; output additionalContext on changes
    #[arg(long)]
    pub claude_hook: bool,
}

#[derive(Subcommand)]
pub enum UpdateCmd {
    /// Show updater state for the running daemon
    Status {
        /// Output JSON instead of tab-separated text
        #[arg(long)]
        json: bool,
    },
    /// List stable releases older than the running version
    List {
        /// Output JSON instead of tab-separated text
        #[arg(long, conflicts_with = "quiet")]
        json: bool,
        /// Print only version numbers, one per line
        #[arg(short, long, conflicts_with = "json")]
        quiet: bool,
    },
    /// Download and install an older stable release
    Revert {
        /// Exact release version, without the leading v
        version: String,
        /// Keep the current config despite possible incompatibility
        #[arg(long)]
        keep_config: bool,
        /// Preview the binary and config changes without applying them
        #[arg(long)]
        dry_run: bool,
        /// Apply without an interactive confirmation
        #[arg(long)]
        yes: bool,
        /// Restart the daemon after installation; this ends terminal sessions
        #[arg(long)]
        restart: bool,
        /// Output JSON instead of human-readable text
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
pub enum SettingsCmd {
    /// Show current settings (optionally a single dotted key, e.g. sidebar.width)
    Show {
        /// Dotted key path; omit for the whole settings object
        key: Option<String>,
    },
    /// Show the settings schema (every key with its default value)
    Schema,
    /// Set a setting: a dotted key to a JSON value (bare strings are allowed)
    Set {
        /// Dotted key path (e.g. font_size, sidebar.width)
        key: String,
        /// JSON value (e.g. 16, true, "JetBrains Mono"); bare text → string
        value: String,
    },
}

#[derive(Subcommand)]
pub enum ThemeCmd {
    /// List built-in and custom themes
    List {
        /// Output JSON instead of the default plain text
        #[arg(long)]
        json: bool,
    },
    /// Print a theme as an editable JSON blob (the active theme if id omitted)
    Show {
        /// Theme id: a built-in mode (dark/light/…) or a custom id
        id: Option<String>,
    },
    /// Activate a theme (built-in mode or custom id)
    Set {
        /// Theme id: auto/dark/light/pastel-dark/high-contrast or a custom id
        id: String,
    },
    /// Write a custom theme from a full JSON blob and (by default) activate it
    Save {
        /// Custom theme id (becomes the file name stem)
        id: String,
        /// The full CustomThemeConfig JSON; omit or '-' to read stdin
        json: Option<String>,
        /// Write the file but don't switch to it
        #[arg(long)]
        no_activate: bool,
    },
}

#[derive(Subcommand)]
pub enum PaletteCmd {
    /// List invokable commands (name, description, category)
    List {
        /// Output JSON instead of the default plain text
        #[arg(long)]
        json: bool,
    },
    /// Invoke a command by name (e.g. ToggleSidebar, NewWindow, ZoomIn)
    Run {
        /// Action name from `okena command list`
        name: String,
    },
}

#[derive(Subcommand)]
pub enum SkillCmd {
    /// Print the skill markdown to stdout
    Show,
    /// Install the skill as a Claude Code skill (SKILL.md)
    Install {
        /// Install for the current user: ~/.claude/skills/okena (default)
        #[arg(long)]
        user: bool,
        /// Install into the current project: ./.claude/skills/okena
        #[arg(long, conflicts_with = "user")]
        project: bool,
    },
}

#[derive(Subcommand)]
pub enum ServiceCmd {
    /// Start a service
    Start {
        /// Service name (see `okena services`)
        name: String,
        /// Project (id / name); omit to use the only / focused project
        project: Option<String>,
        /// Output JSON instead of the default plain text
        #[arg(long)]
        json: bool,
    },
    /// Stop a service
    Stop {
        /// Service name (see `okena services`)
        name: String,
        /// Project (id / name); omit to use the only / focused project
        project: Option<String>,
        /// Output JSON instead of the default plain text
        #[arg(long)]
        json: bool,
    },
    /// Restart a service
    Restart {
        /// Service name (see `okena services`)
        name: String,
        /// Project (id / name); omit to use the only / focused project
        project: Option<String>,
        /// Output JSON instead of the default plain text
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
    /// Clone a git repository and add the checkout as a project
    Clone {
        /// Repository URL (anything `git clone` accepts)
        url: String,
        /// Parent directory to clone into (defaults to the CWD)
        #[arg(long)]
        into: Option<String>,
        /// Directory name inside the parent (defaults to the repo name)
        #[arg(long)]
        dir: Option<String>,
        /// Display name (defaults to the directory name)
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
        /// New display name
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
        /// Remove even with uncommitted changes / unmerged work
        #[arg(long)]
        force: bool,
    },
}

#[derive(Subcommand)]
pub enum FolderCmd {
    /// Create a folder
    Add {
        /// Folder name
        name: String,
    },
    /// Delete a folder
    Rm {
        /// Folder (id / name)
        folder: String,
    },
    /// Rename a folder
    Rename {
        /// Folder (id / name)
        folder: String,
        /// New folder name
        name: String,
    },
}

#[derive(Subcommand)]
pub enum TermCmd {
    /// List terminals
    Ls {
        /// Optional project filter (id / name / path)
        project: Option<String>,
        /// Output JSON instead of the default plain text
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
        /// New terminal name
        name: String,
    },
    /// Split a terminal: h = stacked top/bottom, v = side by side left/right
    Split {
        /// Terminal address (id, project/name, or project:index)
        terminal: String,
        /// h = horizontal split (new pane below, panes stacked top/bottom);
        /// v = vertical split (new pane to the right, panes side by side)
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
        "worktree", "folder", "term", "send", "run", "key", "read", "skill", "settings", "theme",
        "command", "update", "mission",
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory as _;

    #[test]
    fn both_advertised_version_forms_report_the_app_version() {
        let version = Cli::command()
            .get_version()
            .expect("clap needs version metadata or `-V` is an unknown argument")
            .to_string();
        assert_eq!(version, env!("CARGO_PKG_VERSION"));
        // The workspace version is the shipped app version; a per-crate 0.1.0
        // here would report a version no release ever had.
        assert_ne!(version, "0.1.0");

        for form in ["-V", "--version"] {
            let kind = Cli::try_parse_from(["okena", form])
                .err()
                .map(|e| e.kind())
                .expect("version exits via an error");
            assert_eq!(kind, clap::error::ErrorKind::DisplayVersion, "{form}");
        }
    }

    #[test]
    fn command_tree_is_well_formed() {
        // clap runs its internal consistency debug-asserts here; a malformed
        // command tree would panic.
        Cli::command().debug_assert();
    }

    #[test]
    fn mission_context_requires_a_complete_identity_and_separates_hook_output() {
        assert!(Cli::try_parse_from(["okena", "mission", "context"]).is_ok());
        assert!(Cli::try_parse_from(["okena", "mission", "context", "--json"]).is_ok());
        assert!(Cli::try_parse_from(["okena", "mission", "context", "--claude-hook"]).is_ok());
        assert!(
            Cli::try_parse_from([
                "okena",
                "mission",
                "context",
                "--agent",
                "claude-code",
                "--session-id",
                "00000000-0000-0000-0000-000000000001"
            ])
            .is_ok()
        );
        assert!(
            Cli::try_parse_from(["okena", "mission", "context", "--agent", "claude-code"]).is_err()
        );
        assert!(Cli::try_parse_from(["okena", "mission", "context", "--session-id", "x"]).is_err());
        assert!(
            Cli::try_parse_from(["okena", "mission", "context", "--claude-hook", "--json"])
                .is_err()
        );
    }

    #[test]
    fn mission_membership_selectors_are_explicit_and_mutually_exclusive() {
        for verb in ["attach", "detach", "move"] {
            for selector in [
                vec!["--current-terminal"],
                vec!["--terminal", "Repo:0"],
                vec!["--repository", "Repo"],
                vec!["--worktree", "Worktree"],
                vec![
                    "--agent",
                    "claude-code",
                    "--session-id",
                    "00000000-0000-0000-0000-000000000001",
                ],
            ] {
                let mut args = vec!["okena", "mission", verb, "Export"];
                args.extend(selector);
                args.push("--json");
                assert!(Cli::try_parse_from(args).is_ok(), "{verb}");
            }
            for selector in [
                vec![],
                vec!["--agent", "claude-code"],
                vec!["--session-id", "x"],
                vec!["--current-terminal", "--repository", "Repo"],
                vec![
                    "--terminal",
                    "t",
                    "--agent",
                    "claude-code",
                    "--session-id",
                    "x",
                ],
            ] {
                let mut args = vec!["okena", "mission", verb, "Export"];
                args.extend(selector);
                assert!(Cli::try_parse_from(args).is_err(), "{verb}");
            }
        }
        assert!(Cli::try_parse_from(["okena", "mission", "create", "Export"]).is_ok());
        assert!(
            Cli::try_parse_from([
                "okena",
                "mission",
                "create",
                "Export",
                "--goal",
                "Add export",
                "--home-project",
                "Repo",
                "--current-terminal",
                "--json"
            ])
            .is_ok()
        );
        assert!(Cli::try_parse_from(["okena", "mission", "list", "--quiet", "--json"]).is_err());
        assert!(Cli::try_parse_from(["okena", "mission", "show", "Export", "--json"]).is_ok());
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
        assert!(Cli::try_parse_from(["okena", "update", "list", "--json"]).is_ok());
        assert!(
            Cli::try_parse_from([
                "okena",
                "update",
                "revert",
                "0.26.0",
                "--keep-config",
                "--dry-run",
                "--restart",
                "--json",
            ])
            .is_ok()
        );
        // `run --wait` flags precede the trailing command (trailing_var_arg).
        assert!(Cli::try_parse_from(["okena", "run", "--wait", "t1", "make"]).is_ok());
        assert!(
            Cli::try_parse_from(["okena", "run", "--wait", "--timeout", "60", "t1", "make"])
                .is_ok()
        );
        assert!(Cli::try_parse_from(["okena", "skill", "show"]).is_ok());
        assert!(Cli::try_parse_from(["okena", "skill", "install", "--project"]).is_ok());
        assert!(Cli::try_parse_from(["okena", "settings", "show"]).is_ok());
        assert!(Cli::try_parse_from(["okena", "settings", "show", "sidebar.width"]).is_ok());
        assert!(Cli::try_parse_from(["okena", "settings", "set", "font_size", "16"]).is_ok());
        assert!(Cli::try_parse_from(["okena", "theme", "list", "--json"]).is_ok());
        assert!(Cli::try_parse_from(["okena", "theme", "set", "dark"]).is_ok());
        assert!(Cli::try_parse_from(["okena", "theme", "save", "mine", "--no-activate"]).is_ok());
        assert!(Cli::try_parse_from(["okena", "command", "list"]).is_ok());
        assert!(
            Cli::try_parse_from([
                "okena",
                "command",
                "run",
                "ToggleSidebar",
                "--window",
                "main"
            ])
            .is_ok()
        );
        // --user and --project are mutually exclusive.
        assert!(Cli::try_parse_from(["okena", "skill", "install", "--user", "--project"]).is_err());
        // Missing required positional → error.
        assert!(Cli::try_parse_from(["okena", "term", "split", "p/sh"]).is_err());
    }
}
