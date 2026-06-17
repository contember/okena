use crate::keys::SpecialKey;
use crate::theme::FolderColor;
use crate::types::{DiffMode, SplitDirection};
use serde::{Deserialize, Serialize};

// ── API request/response types ──────────────────────────────────────────────

/// GET /health response
#[derive(Serialize, Deserialize)]
pub struct HealthResponse {
    pub status: String,
    pub version: String,
    pub uptime_secs: u64,
}

/// GET /v1/state response
#[derive(Clone, Serialize, Deserialize)]
pub struct StateResponse {
    pub state_version: u64,
    pub projects: Vec<ApiProject>,
    pub focused_project_id: Option<String>,
    pub fullscreen_terminal: Option<ApiFullscreen>,
    #[serde(default)]
    pub project_order: Vec<String>,
    #[serde(default)]
    pub folders: Vec<ApiFolder>,
    #[serde(default)]
    pub windows: Vec<ApiWindow>,
}

/// OS window bounds in screen pixels.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ApiWindowBounds {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// One open OS window onto the shared workspace. Multi-window state is
/// exposed so remote/CLI clients can see what the user actually sees.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ApiWindow {
    /// "main" for the main window, or the extra window's UUID string.
    pub id: String,
    /// "main" | "extra"
    pub kind: String,
    /// True if this window currently has OS focus.
    pub active: bool,
    /// Project focused/zoomed in this window's sidebar (per-window focus).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focused_project_id: Option<String>,
    /// Terminal focused in this window, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focused_terminal_id: Option<String>,
    /// Fullscreen terminal in this window, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fullscreen: Option<ApiFullscreen>,
    /// Projects visible in this window (after hidden_project_ids + folder_filter), in display order.
    #[serde(default)]
    pub visible_project_ids: Vec<String>,
    /// Active folder filter (folder id) limiting this window's projects, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder_filter: Option<String>,
    /// OS window bounds, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bounds: Option<ApiWindowBounds>,
    /// Whether the sidebar is open in this window. None = use app default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sidebar_open: Option<bool>,
}

/// PR state from GitHub
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PrState {
    Open,
    Merged,
    Closed,
    Draft,
}

impl PrState {
    /// Display label for this PR state
    pub fn label(&self) -> &'static str {
        match self {
            PrState::Open => "Open",
            PrState::Draft => "Draft",
            PrState::Merged => "Merged",
            PrState::Closed => "Closed",
        }
    }
}

/// Overall CI check rollup status
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CiStatus {
    Success,
    Failure,
    Pending,
}

impl CiStatus {
    pub fn icon(&self) -> &'static str {
        match self {
            CiStatus::Success => "icons/check.svg",
            CiStatus::Failure => "icons/close.svg",
            CiStatus::Pending => "icons/refresh.svg",
        }
    }

    pub fn is_pending(&self) -> bool {
        matches!(self, CiStatus::Pending)
    }
}

/// A single CI check / status entry as returned by `gh pr checks`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CiCheck {
    /// Display name (e.g. "Lint", "Test (ubuntu-latest)").
    pub name: String,
    /// Workflow name (e.g. "CI", "Vercel"). `None` for non-Actions checks
    /// where `gh` doesn't expose a workflow.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow: Option<String>,
    /// Bucket-derived overall status (pass/fail/pending). Skipped checks
    /// are represented by `Pending` and `is_skipped`.
    pub status: CiStatus,
    /// True for checks whose bucket is `"skipping"` — rendered with a
    /// distinct icon and not counted toward pass/fail in the summary.
    #[serde(default)]
    pub is_skipped: bool,
    /// Direct link to the run/check on GitHub (or provider).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
    /// Human-readable description, when `gh` provides one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Elapsed time in milliseconds. `0` when unknown / still running.
    #[serde(default)]
    pub elapsed_ms: u64,
}

impl CiCheck {
    /// Format `elapsed_ms` as compact "Xs" or "XmYs" (or "—" when 0).
    pub fn elapsed_label(&self) -> String {
        if self.elapsed_ms == 0 {
            return "\u{2014}".to_string();
        }
        let secs = self.elapsed_ms / 1000;
        if secs < 60 {
            format!("{}s", secs)
        } else {
            format!("{}m{}s", secs / 60, secs % 60)
        }
    }
}

/// Summary of CI check results
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CiCheckSummary {
    pub status: CiStatus,
    pub passed: usize,
    pub failed: usize,
    pub pending: usize,
    pub total: usize,
    /// Per-check details; empty when `gh pr checks` didn't return rich
    /// info (e.g. on older `gh` versions).
    #[serde(default)]
    pub checks: Vec<CiCheck>,
}

impl CiCheckSummary {
    pub fn tooltip_text(&self) -> String {
        match self.status {
            CiStatus::Success => format!("{}/{} checks passed", self.passed, self.total),
            CiStatus::Failure => format!("{} failed, {} passed of {} checks", self.failed, self.passed, self.total),
            CiStatus::Pending => format!("{} pending, {} passed of {} checks", self.pending, self.passed, self.total),
        }
    }
}

/// Pull request info
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrInfo {
    pub url: String,
    pub state: PrState,
    pub number: u32,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ApiGitStatus {
    pub branch: Option<String>,
    pub lines_added: usize,
    pub lines_removed: usize,
    /// Pull request info for the current branch (if any). Populated on the
    /// host from `gh` and forwarded to remote clients so the status pill shows
    /// the PR badge over a connection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr_info: Option<PrInfo>,
    /// CI / pipeline status for the current branch's HEAD commit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ci_checks: Option<CiCheckSummary>,
    /// Commits the local branch is ahead of its upstream (`None` if no upstream).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ahead: Option<usize>,
    /// Commits the local branch is behind its upstream (`None` if no upstream).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub behind: Option<usize>,
    /// Commits not yet pushed to `origin/<branch>` (`None` if never pushed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unpushed: Option<usize>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ApiProject {
    pub id: String,
    pub name: String,
    pub path: String,
    #[serde(alias = "is_visible")]
    pub show_in_overview: bool,
    pub layout: Option<ApiLayoutNode>,
    pub terminal_names: std::collections::HashMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_status: Option<ApiGitStatus>,
    #[serde(default)]
    pub folder_color: FolderColor,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub services: Vec<ApiServiceInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_info: Option<ApiWorktreeMetadata>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub worktree_ids: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ApiWorktreeMetadata {
    pub parent_project_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_override: Option<FolderColor>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ApiServiceInfo {
    pub name: String,
    pub status: String, // "running", "stopped", "crashed", "starting", "restarting"
    pub terminal_id: Option<String>,
    #[serde(default)]
    pub ports: Vec<u16>,
    /// Exit code when status is "crashed"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<u32>,
    /// Service kind: "okena" or "docker_compose"
    #[serde(default = "default_service_kind")]
    pub kind: String,
    /// Docker service not listed in okena.yaml filter
    #[serde(default)]
    pub is_extra: bool,
}

fn default_service_kind() -> String {
    "okena".to_string()
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ApiFolder {
    pub id: String,
    pub name: String,
    pub project_ids: Vec<String>,
    #[serde(default)]
    pub folder_color: FolderColor,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ApiLayoutNode {
    Terminal {
        terminal_id: Option<String>,
        minimized: bool,
        detached: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cols: Option<u16>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rows: Option<u16>,
    },
    Split {
        direction: SplitDirection,
        sizes: Vec<f32>,
        children: Vec<ApiLayoutNode>,
    },
    Tabs {
        children: Vec<ApiLayoutNode>,
        active_tab: usize,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ApiFullscreen {
    pub project_id: String,
    pub terminal_id: String,
}

/// POST /v1/actions request body (tagged enum)
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum ActionRequest {
    SendText {
        terminal_id: String,
        text: String,
    },
    RunCommand {
        terminal_id: String,
        command: String,
    },
    SendSpecialKey {
        terminal_id: String,
        key: SpecialKey,
    },
    SplitTerminal {
        project_id: String,
        path: Vec<usize>,
        direction: SplitDirection,
    },
    CloseTerminal {
        project_id: String,
        terminal_id: String,
    },
    CloseTerminals {
        project_id: String,
        terminal_ids: Vec<String>,
    },
    FocusTerminal {
        project_id: String,
        terminal_id: String,
        /// Target window ("main" or an extra window's UUID). None = the focused window.
        #[serde(default)]
        window: Option<String>,
    },
    ReadContent {
        terminal_id: String,
    },
    Resize {
        terminal_id: String,
        cols: u16,
        rows: u16,
    },
    CreateTerminal {
        project_id: String,
    },
    UpdateSplitSizes {
        project_id: String,
        path: Vec<usize>,
        sizes: Vec<f32>,
    },
    ToggleMinimized {
        project_id: String,
        terminal_id: String,
    },
    SetFullscreen {
        project_id: String,
        terminal_id: Option<String>,
        /// Target window ("main" or an extra window's UUID). None = the focused window.
        #[serde(default)]
        window: Option<String>,
    },
    RenameTerminal {
        project_id: String,
        terminal_id: String,
        name: String,
    },
    AddTab {
        project_id: String,
        path: Vec<usize>,
        in_group: bool,
    },
    SetActiveTab {
        project_id: String,
        path: Vec<usize>,
        index: usize,
    },
    MoveTab {
        project_id: String,
        path: Vec<usize>,
        from_index: usize,
        to_index: usize,
    },
    MoveTerminalToTabGroup {
        project_id: String,
        terminal_id: String,
        target_path: Vec<usize>,
        position: Option<usize>,
        #[serde(default)]
        target_project_id: Option<String>,
    },
    MovePaneTo {
        project_id: String,
        terminal_id: String,
        target_project_id: String,
        target_terminal_id: String,
        zone: String,
    },
    GitStatus {
        project_id: String,
    },
    GitDiffSummary {
        project_id: String,
    },
    GitDiff {
        project_id: String,
        #[serde(default)]
        mode: DiffMode,
        #[serde(default)]
        ignore_whitespace: bool,
    },
    GitBranches {
        project_id: String,
    },
    GitFileContents {
        project_id: String,
        file_path: String,
        #[serde(default)]
        mode: DiffMode,
    },
    GitCommitGraph {
        project_id: String,
        count: usize,
        #[serde(default)]
        branch: Option<String>,
    },
    GitListBranches {
        project_id: String,
    },
    GitStageFile {
        project_id: String,
        file_path: String,
    },
    GitUnstageFile {
        project_id: String,
        file_path: String,
    },
    GitDiscardFile {
        project_id: String,
        file_path: String,
    },
    GitBlame {
        project_id: String,
        relative_path: String,
    },
    AddProject {
        name: String,
        path: String,
    },
    ReorderProjectInFolder {
        folder_id: String,
        project_id: String,
        new_index: usize,
    },
    SetProjectColor {
        project_id: String,
        color: FolderColor,
    },
    SetFolderColor {
        folder_id: String,
        color: FolderColor,
    },
    StartService {
        project_id: String,
        service_name: String,
    },
    StopService {
        project_id: String,
        service_name: String,
    },
    RestartService {
        project_id: String,
        service_name: String,
    },
    StartAllServices {
        project_id: String,
    },
    StopAllServices {
        project_id: String,
    },
    ReloadServices {
        project_id: String,
    },
    CreateWorktree {
        project_id: String,
        branch: String,
        #[serde(default)]
        create_branch: bool,
    },
    ListFiles {
        project_id: String,
        #[serde(default)]
        show_ignored: bool,
    },
    ListDirectory {
        project_id: String,
        #[serde(default)]
        relative_path: String,
        #[serde(default)]
        show_ignored: bool,
    },
    ReadFile {
        project_id: String,
        relative_path: String,
    },
    ReadFileBytes {
        project_id: String,
        relative_path: String,
    },
    FileSize {
        project_id: String,
        relative_path: String,
    },
    SearchContent {
        project_id: String,
        query: String,
        #[serde(default)]
        case_sensitive: bool,
        #[serde(default = "default_search_mode")]
        mode: String,
        #[serde(default = "default_max_results")]
        max_results: usize,
        #[serde(default)]
        file_glob: Option<String>,
        #[serde(default)]
        context_lines: usize,
    },
    RenameFile {
        project_id: String,
        relative_path: String,
        new_name: String,
    },
    DeleteFile {
        project_id: String,
        relative_path: String,
    },
    CreateFile {
        project_id: String,
        relative_path: String,
    },
    CreateDirectory {
        project_id: String,
        relative_path: String,
    },
    RenameProject {
        project_id: String,
        name: String,
    },
    RenameProjectDirectory {
        project_id: String,
        new_name: String,
    },
    DeleteProject {
        project_id: String,
    },
    SetProjectShowInOverview {
        project_id: String,
        show: bool,
        /// Target window ("main" or an extra window's UUID). None = the focused window.
        #[serde(default)]
        window: Option<String>,
    },
    RemoveWorktreeProject {
        project_id: String,
        #[serde(default)]
        force: bool,
    },
    CreateFolder {
        name: String,
    },
    DeleteFolder {
        folder_id: String,
    },
    RenameFolder {
        folder_id: String,
        name: String,
    },
    MoveProjectToFolder {
        project_id: String,
        folder_id: String,
        #[serde(default)]
        position: Option<usize>,
    },
    MoveProjectOutOfFolder {
        project_id: String,
        top_level_index: usize,
    },

    // ── Settings (app-scoped; handled at the remote bridge) ───────────
    /// Return the full current settings as JSON.
    GetSettings,
    /// Return a defaults instance of the settings (de-facto schema: every key
    /// present with its default value).
    GetSettingsSchema,
    /// Deep-merge `patch` into the current settings and apply.
    SetSettings {
        patch: serde_json::Value,
    },

    // ── Theme (app-scoped; handled at the remote bridge) ──────────────
    /// List built-in + custom themes, flagging the active one.
    GetThemes,
    /// Return a theme as an editable custom-theme blob (the active theme when
    /// `id` is None).
    GetTheme {
        #[serde(default)]
        id: Option<String>,
    },
    /// Activate a theme by id: a built-in mode (auto / dark / light /
    /// pastel-dark / high-contrast) or a custom theme id (with or without the
    /// `custom:` prefix).
    SetTheme {
        id: String,
    },
    /// Write a custom theme JSON file (a full `CustomThemeConfig`) and,
    /// when `activate`, switch to it.
    SaveCustomTheme {
        id: String,
        config: serde_json::Value,
        #[serde(default)]
        activate: bool,
    },

    // ── Command palette (app-scoped; handled at the remote bridge) ────
    /// List invokable GUI commands (name, description, category).
    ListActions,
    /// Invoke a named GUI command in a window (the focused window when
    /// `window` is None).
    InvokeAction {
        action_name: String,
        #[serde(default)]
        window: Option<String>,
    },
}

/// Result of processing a remote command on the GPUI thread.
///
/// Lives in `okena-core` (rather than the server crate) so the binary's
/// action-execution layer can produce it without depending on
/// `okena-remote-server`. The server re-exports it as
/// `okena_remote_server::bridge::CommandResult`.
#[derive(Debug)]
pub enum CommandResult {
    /// Success with optional JSON-serializable payload.
    Ok(Option<serde_json::Value>),
    /// Success with raw bytes (e.g., terminal snapshots).
    OkBytes(Vec<u8>),
    /// Error with a human-readable message.
    Err(String),
}

impl ActionRequest {
    /// The window an action explicitly targets ("main" or an extra UUID), if
    /// any. Only the per-window actions carry this; everything else returns
    /// None and lands on the focused window.
    pub fn target_window(&self) -> Option<&str> {
        match self {
            ActionRequest::FocusTerminal { window, .. }
            | ActionRequest::SetProjectShowInOverview { window, .. }
            | ActionRequest::SetFullscreen { window, .. } => window.as_deref(),
            _ => None,
        }
    }
}

fn default_search_mode() -> String { "literal".to_string() }
fn default_max_results() -> usize { 1000 }

/// POST /v1/pair request
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairRequest {
    pub code: String,
}

/// POST /v1/pair response
#[derive(Serialize, Deserialize)]
pub struct PairResponse {
    pub token: String,
    pub expires_in: u64,
}

/// Generic error response
#[derive(Serialize, Deserialize)]
pub struct ErrorResponse {
    pub error: String,
}

// ── Helper methods ──────────────────────────────────────────────────────────

impl ApiLayoutNode {
    /// Collect all terminal IDs from the layout tree
    pub fn collect_terminal_ids(&self) -> Vec<String> {
        let mut ids = Vec::new();
        self.collect_terminal_ids_into(&mut ids);
        ids
    }

    fn collect_terminal_ids_into(&self, ids: &mut Vec<String>) {
        match self {
            ApiLayoutNode::Terminal { terminal_id, .. } => {
                if let Some(id) = terminal_id {
                    ids.push(id.clone());
                }
            }
            ApiLayoutNode::Split { children, .. } | ApiLayoutNode::Tabs { children, .. } => {
                for child in children {
                    child.collect_terminal_ids_into(ids);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_response_round_trip() {
        let resp = StateResponse {
            state_version: 42,
            projects: vec![ApiProject {
                id: "p1".into(),
                name: "Test".into(),
                path: "/tmp".into(),
                show_in_overview: true,
                layout: Some(ApiLayoutNode::Split {
                    direction: SplitDirection::Horizontal,
                    sizes: vec![50.0, 50.0],
                    children: vec![
                        ApiLayoutNode::Terminal {
                            terminal_id: Some("t1".into()),
                            minimized: false,
                            detached: false,
                            cols: None,
                            rows: None,
                        },
                        ApiLayoutNode::Tabs {
                            active_tab: 0,
                            children: vec![ApiLayoutNode::Terminal {
                                terminal_id: Some("t2".into()),
                                minimized: true,
                                detached: true,
                                cols: None,
                                rows: None,
                            }],
                        },
                    ],
                }),
                terminal_names: [("t1".into(), "bash".into())].into_iter().collect(),
                git_status: None,
                folder_color: FolderColor::Blue,
                services: vec![],
                worktree_info: None,
                worktree_ids: vec![],
            }],
            focused_project_id: Some("p1".into()),
            fullscreen_terminal: None,
            project_order: vec!["folder1".into(), "p1".into()],
            folders: vec![ApiFolder {
                id: "folder1".into(),
                name: "My Folder".into(),
                project_ids: vec!["p2".into()],
                folder_color: FolderColor::Red,
            }],
            windows: vec![ApiWindow {
                id: "main".into(),
                kind: "main".into(),
                active: true,
                focused_project_id: Some("p1".into()),
                focused_terminal_id: Some("t1".into()),
                fullscreen: Some(ApiFullscreen {
                    project_id: "p1".into(),
                    terminal_id: "t1".into(),
                }),
                visible_project_ids: vec!["p1".into(), "p2".into()],
                folder_filter: Some("folder1".into()),
                bounds: Some(ApiWindowBounds {
                    x: 10.0,
                    y: 20.0,
                    width: 800.0,
                    height: 600.0,
                }),
                sidebar_open: Some(true),
            }],
        };
        let json = serde_json::to_string(&resp).unwrap();
        let parsed: StateResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.state_version, 42);
        assert_eq!(parsed.projects.len(), 1);
        assert_eq!(parsed.projects[0].id, "p1");
        assert!(matches!(parsed.projects[0].folder_color, FolderColor::Blue));
        assert!(parsed.fullscreen_terminal.is_none());
        assert_eq!(parsed.project_order, vec!["folder1", "p1"]);
        assert_eq!(parsed.folders.len(), 1);
        assert_eq!(parsed.folders[0].id, "folder1");
        assert!(matches!(parsed.folders[0].folder_color, FolderColor::Red));
        assert_eq!(parsed.windows.len(), 1);
        let win = &parsed.windows[0];
        assert_eq!(win.id, "main");
        assert_eq!(win.kind, "main");
        assert!(win.active);
        assert_eq!(win.focused_project_id.as_deref(), Some("p1"));
        assert_eq!(win.focused_terminal_id.as_deref(), Some("t1"));
        assert_eq!(win.fullscreen.as_ref().unwrap().terminal_id, "t1");
        assert_eq!(win.visible_project_ids, vec!["p1", "p2"]);
        assert_eq!(win.folder_filter.as_deref(), Some("folder1"));
        assert_eq!(win.bounds, Some(ApiWindowBounds {
            x: 10.0,
            y: 20.0,
            width: 800.0,
            height: 600.0,
        }));
        assert_eq!(win.sidebar_open, Some(true));
    }

    #[test]
    fn state_response_backward_compat() {
        // Old server response without project_order/folders/folder_color
        let json = r#"{"state_version":1,"projects":[{"id":"p1","name":"Test","path":"/tmp","is_visible":true,"layout":null,"terminal_names":{}}],"focused_project_id":null,"fullscreen_terminal":null}"#;
        let parsed: StateResponse = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.project_order.len(), 0);
        assert_eq!(parsed.folders.len(), 0);
        assert!(parsed.windows.is_empty());
        assert!(matches!(parsed.projects[0].folder_color, FolderColor::Default));
    }

    #[test]
    fn api_git_status_round_trips_pr_and_ci() {
        let status = ApiGitStatus {
            branch: Some("feature/x".into()),
            lines_added: 12,
            lines_removed: 4,
            pr_info: Some(PrInfo {
                url: "https://github.com/o/r/pull/7".into(),
                state: PrState::Open,
                number: 7,
            }),
            ci_checks: Some(CiCheckSummary {
                status: CiStatus::Failure,
                passed: 2,
                failed: 1,
                pending: 0,
                total: 3,
                checks: vec![CiCheck {
                    name: "Lint".into(),
                    workflow: Some("CI".into()),
                    status: CiStatus::Failure,
                    is_skipped: false,
                    link: Some("https://github.com/o/r/runs/1".into()),
                    description: None,
                    elapsed_ms: 65_000,
                }],
            }),
            ahead: Some(3),
            behind: Some(1),
            unpushed: Some(2),
        };
        let json = serde_json::to_string(&status).unwrap();
        let parsed: ApiGitStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.pr_info, status.pr_info);
        assert_eq!(parsed.ci_checks, status.ci_checks);
        assert_eq!(parsed.ahead, Some(3));
        assert_eq!(parsed.behind, Some(1));
        assert_eq!(parsed.unpushed, Some(2));
        assert_eq!(parsed.ci_checks.as_ref().unwrap().checks[0].elapsed_label(), "1m5s");
    }

    #[test]
    fn api_git_status_backward_compat_minimal() {
        // An old host sends only branch + line counts; new optional fields
        // must default to None rather than failing to parse.
        let json = r#"{"branch":"main","lines_added":1,"lines_removed":0}"#;
        let parsed: ApiGitStatus = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.branch.as_deref(), Some("main"));
        assert!(parsed.pr_info.is_none());
        assert!(parsed.ci_checks.is_none());
        assert!(parsed.ahead.is_none());
    }

    #[test]
    fn action_request_round_trip() {
        let actions = vec![
            ActionRequest::SendText {
                terminal_id: "t1".into(),
                text: "hello".into(),
            },
            ActionRequest::RunCommand {
                terminal_id: "t1".into(),
                command: "ls".into(),
            },
            ActionRequest::SendSpecialKey {
                terminal_id: "t1".into(),
                key: SpecialKey::Enter,
            },
            ActionRequest::SplitTerminal {
                project_id: "p1".into(),
                path: vec![0, 1],
                direction: SplitDirection::Vertical,
            },
            ActionRequest::CloseTerminal {
                project_id: "p1".into(),
                terminal_id: "t1".into(),
            },
            ActionRequest::CloseTerminals {
                project_id: "p1".into(),
                terminal_ids: vec!["t1".into(), "t2".into()],
            },
            ActionRequest::FocusTerminal {
                project_id: "p1".into(),
                terminal_id: "t1".into(),
                window: Some("main".into()),
            },
            ActionRequest::ReadContent {
                terminal_id: "t1".into(),
            },
            ActionRequest::Resize {
                terminal_id: "t1".into(),
                cols: 80,
                rows: 24,
            },
            ActionRequest::CreateTerminal {
                project_id: "p1".into(),
            },
            ActionRequest::UpdateSplitSizes {
                project_id: "p1".into(),
                path: vec![0],
                sizes: vec![60.0, 40.0],
            },
            ActionRequest::ToggleMinimized {
                project_id: "p1".into(),
                terminal_id: "t1".into(),
            },
            ActionRequest::SetFullscreen {
                project_id: "p1".into(),
                terminal_id: Some("t1".into()),
                window: Some("main".into()),
            },
            ActionRequest::SetFullscreen {
                project_id: "p1".into(),
                terminal_id: None,
                window: None,
            },
            ActionRequest::RenameTerminal {
                project_id: "p1".into(),
                terminal_id: "t1".into(),
                name: "my-term".into(),
            },
            ActionRequest::AddTab {
                project_id: "p1".into(),
                path: vec![0, 1],
                in_group: true,
            },
            ActionRequest::SetActiveTab {
                project_id: "p1".into(),
                path: vec![0],
                index: 2,
            },
            ActionRequest::MoveTab {
                project_id: "p1".into(),
                path: vec![0],
                from_index: 0,
                to_index: 2,
            },
            ActionRequest::MoveTerminalToTabGroup {
                project_id: "p1".into(),
                terminal_id: "t1".into(),
                target_path: vec![1],
                position: Some(0),
                target_project_id: Some("p2".into()),
            },
            ActionRequest::MovePaneTo {
                project_id: "p1".into(),
                terminal_id: "t1".into(),
                target_project_id: "p1".into(),
                target_terminal_id: "t2".into(),
                zone: "left".into(),
            },
            ActionRequest::GitStatus {
                project_id: "p1".into(),
            },
            ActionRequest::GitDiffSummary {
                project_id: "p1".into(),
            },
            ActionRequest::GitDiff {
                project_id: "p1".into(),
                mode: DiffMode::WorkingTree,
                ignore_whitespace: false,
            },
            ActionRequest::GitBranches {
                project_id: "p1".into(),
            },
            ActionRequest::GitFileContents {
                project_id: "p1".into(),
                file_path: "src/main.rs".into(),
                mode: DiffMode::Staged,
            },
            ActionRequest::GitStageFile {
                project_id: "p1".into(),
                file_path: "src/main.rs".into(),
            },
            ActionRequest::GitUnstageFile {
                project_id: "p1".into(),
                file_path: "src/main.rs".into(),
            },
            ActionRequest::GitDiscardFile {
                project_id: "p1".into(),
                file_path: "src/main.rs".into(),
            },
            ActionRequest::GitBlame {
                project_id: "p1".into(),
                relative_path: "src/main.rs".into(),
            },
            ActionRequest::AddProject {
                name: "My Project".into(),
                path: "/home/user/projects/my-project".into(),
            },
            ActionRequest::ReorderProjectInFolder {
                folder_id: "f1".into(),
                project_id: "p1".into(),
                new_index: 2,
            },
            ActionRequest::SetProjectColor {
                project_id: "p1".into(),
                color: FolderColor::Green,
            },
            ActionRequest::SetFolderColor {
                folder_id: "f1".into(),
                color: FolderColor::Purple,
            },
            ActionRequest::StartService {
                project_id: "p1".into(),
                service_name: "vite".into(),
            },
            ActionRequest::StopService {
                project_id: "p1".into(),
                service_name: "vite".into(),
            },
            ActionRequest::RestartService {
                project_id: "p1".into(),
                service_name: "vite".into(),
            },
            ActionRequest::StartAllServices {
                project_id: "p1".into(),
            },
            ActionRequest::StopAllServices {
                project_id: "p1".into(),
            },
            ActionRequest::ReloadServices {
                project_id: "p1".into(),
            },
            ActionRequest::RenameFile {
                project_id: "p1".into(),
                relative_path: "src/main.rs".into(),
                new_name: "lib.rs".into(),
            },
            ActionRequest::DeleteFile {
                project_id: "p1".into(),
                relative_path: "src/main.rs".into(),
            },
            ActionRequest::CreateFile {
                project_id: "p1".into(),
                relative_path: "src/new.rs".into(),
            },
            ActionRequest::CreateDirectory {
                project_id: "p1".into(),
                relative_path: "src/new_dir".into(),
            },
            ActionRequest::RenameProject {
                project_id: "p1".into(),
                name: "New Name".into(),
            },
            ActionRequest::RenameProjectDirectory {
                project_id: "p1".into(),
                new_name: "new-dir".into(),
            },
            ActionRequest::DeleteProject {
                project_id: "p1".into(),
            },
            ActionRequest::SetProjectShowInOverview {
                project_id: "p1".into(),
                show: false,
                window: None,
            },
            ActionRequest::RemoveWorktreeProject {
                project_id: "p1".into(),
                force: true,
            },
            ActionRequest::CreateFolder {
                name: "My Folder".into(),
            },
            ActionRequest::DeleteFolder {
                folder_id: "f1".into(),
            },
            ActionRequest::RenameFolder {
                folder_id: "f1".into(),
                name: "Renamed".into(),
            },
            ActionRequest::MoveProjectToFolder {
                project_id: "p1".into(),
                folder_id: "f1".into(),
                position: Some(0),
            },
            ActionRequest::MoveProjectOutOfFolder {
                project_id: "p1".into(),
                top_level_index: 0,
            },
        ];
        for action in actions {
            let json = serde_json::to_string(&action).unwrap();
            let _parsed: ActionRequest = serde_json::from_str(&json).unwrap();
        }
    }

    #[test]
    fn api_layout_node_collect_terminal_ids() {
        let layout = ApiLayoutNode::Split {
            direction: SplitDirection::Horizontal,
            sizes: vec![50.0, 50.0],
            children: vec![
                ApiLayoutNode::Terminal {
                    terminal_id: Some("t1".into()),
                    minimized: false,
                    detached: false,
                    cols: None,
                    rows: None,
                },
                ApiLayoutNode::Tabs {
                    active_tab: 0,
                    children: vec![
                        ApiLayoutNode::Terminal {
                            terminal_id: Some("t2".into()),
                            minimized: false,
                            detached: false,
                            cols: None,
                            rows: None,
                        },
                        ApiLayoutNode::Terminal {
                            terminal_id: None,
                            minimized: false,
                            detached: false,
                            cols: None,
                            rows: None,
                        },
                        ApiLayoutNode::Terminal {
                            terminal_id: Some("t3".into()),
                            minimized: false,
                            detached: true,
                            cols: None,
                            rows: None,
                        },
                    ],
                },
            ],
        };
        let ids = layout.collect_terminal_ids();
        assert_eq!(ids, vec!["t1", "t2", "t3"]);
    }

    #[test]
    fn api_service_info_ports_round_trip() {
        let svc = ApiServiceInfo {
            name: "vite".into(),
            status: "running".into(),
            terminal_id: Some("t1".into()),
            ports: vec![3000, 5173],
            exit_code: None,
            kind: "okena".into(),
            is_extra: false,
        };
        let json = serde_json::to_string(&svc).unwrap();
        let parsed: ApiServiceInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.name, "vite");
        assert_eq!(parsed.ports, vec![3000, 5173]);

        // Test that ports defaults to empty when missing
        let json_no_ports = r#"{"name":"api","status":"stopped","terminal_id":null}"#;
        let parsed: ApiServiceInfo = serde_json::from_str(json_no_ports).unwrap();
        assert!(parsed.ports.is_empty());
    }

    #[test]
    fn api_window_round_trip() {
        let win = ApiWindow {
            id: "550e8400-e29b-41d4-a716-446655440000".into(),
            kind: "extra".into(),
            active: false,
            focused_project_id: Some("p1".into()),
            focused_terminal_id: Some("t1".into()),
            fullscreen: Some(ApiFullscreen {
                project_id: "p1".into(),
                terminal_id: "t1".into(),
            }),
            visible_project_ids: vec!["p1".into(), "p2".into(), "p3".into()],
            folder_filter: Some("f1".into()),
            bounds: Some(ApiWindowBounds {
                x: 100.0,
                y: 200.0,
                width: 1280.0,
                height: 720.0,
            }),
            sidebar_open: Some(false),
        };
        let json = serde_json::to_string(&win).unwrap();
        let parsed: ApiWindow = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.id, win.id);
        assert_eq!(parsed.kind, "extra");
        assert!(!parsed.active);
        assert_eq!(parsed.focused_project_id.as_deref(), Some("p1"));
        assert_eq!(parsed.focused_terminal_id.as_deref(), Some("t1"));
        assert_eq!(parsed.fullscreen.as_ref().unwrap().project_id, "p1");
        assert_eq!(parsed.visible_project_ids, vec!["p1", "p2", "p3"]);
        assert_eq!(parsed.folder_filter.as_deref(), Some("f1"));
        assert_eq!(parsed.bounds, win.bounds);
        assert_eq!(parsed.sidebar_open, Some(false));

        // Old-shape ApiWindow JSON missing all optional fields parses with defaults.
        let minimal = r#"{"id":"main","kind":"main","active":true}"#;
        let parsed: ApiWindow = serde_json::from_str(minimal).unwrap();
        assert_eq!(parsed.id, "main");
        assert_eq!(parsed.kind, "main");
        assert!(parsed.active);
        assert!(parsed.focused_project_id.is_none());
        assert!(parsed.focused_terminal_id.is_none());
        assert!(parsed.fullscreen.is_none());
        assert!(parsed.visible_project_ids.is_empty());
        assert!(parsed.folder_filter.is_none());
        assert!(parsed.bounds.is_none());
        assert!(parsed.sidebar_open.is_none());
    }

    #[test]
    fn action_target_window() {
        let focus = ActionRequest::FocusTerminal {
            project_id: "p1".into(),
            terminal_id: "t1".into(),
            window: Some("main".into()),
        };
        assert_eq!(focus.target_window(), Some("main"));

        let create = ActionRequest::CreateTerminal {
            project_id: "p1".into(),
        };
        assert_eq!(create.target_window(), None);

        // A per-window action with no explicit target also returns None.
        let show = ActionRequest::SetProjectShowInOverview {
            project_id: "p1".into(),
            show: true,
            window: None,
        };
        assert_eq!(show.target_window(), None);
    }
}
