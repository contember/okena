use std::fmt;
use std::time::Instant;

use serde::{Deserialize, Serialize};

use gpui::ScrollHandle;

use super::config::{KruhConfig, KruhPlanOverrides};

#[derive(Clone, Debug, Default, PartialEq)]
pub enum KruhState {
    #[default]
    Scanning,
    PlanPicker,
    TaskBrowser,
    Editing,
    Settings,
    LoopOverview,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum LoopPhase {
    #[default]
    Idle,
    ParsingStatus,
    BuildingPrompt,
    SpawningAgent,
    AgentRunning,
    WaitingForExit,
    CheckingResults,
    Sleeping(u64),
}

impl fmt::Display for LoopPhase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoopPhase::Idle => write!(f, "Idle"),
            LoopPhase::ParsingStatus => write!(f, "Parsing status..."),
            LoopPhase::BuildingPrompt => write!(f, "Building prompt..."),
            LoopPhase::SpawningAgent => write!(f, "Spawning agent..."),
            LoopPhase::AgentRunning => write!(f, "Agent running..."),
            LoopPhase::WaitingForExit => write!(f, "Waiting for exit..."),
            LoopPhase::CheckingResults => write!(f, "Checking results..."),
            LoopPhase::Sleeping(secs) => write!(f, "Sleeping ({}s)...", secs),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum EditTarget {
    Status,
    Instructions,
    Issue,
}

#[derive(Clone, Debug)]
pub struct OutputLine {
    pub text: String,
    #[allow(dead_code)]
    pub timestamp: Instant,
    pub is_error: bool,
}

#[allow(dead_code)]
pub enum KruhPaneEvent {
    Close,
}

#[derive(Clone, Debug, Default)]
pub struct StatusProgress {
    pub pending: usize,
    pub done: usize,
    pub total: usize,
    pub pending_issues: Vec<String>,
    pub done_issues: Vec<String>,
    pub pending_refs: Vec<IssueRef>,
    pub done_refs: Vec<IssueRef>,
}

#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct IssueRef {
    pub number: String,
    pub name: String,
}

#[derive(Clone, Debug)]
pub struct PlanInfo {
    pub name: String,
    pub dir: String,
    pub pending: usize,
    pub done: usize,
    pub total: usize,
}

#[derive(Clone, Debug)]
pub struct IssueDetail {
    pub ref_info: IssueRef,
    #[allow(dead_code)]
    pub raw_name: String,
    pub done: bool,
    #[allow(dead_code)]
    pub preview: Option<String>,
    pub overrides: KruhPlanOverrides,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum LoopState {
    #[default]
    Running,
    Paused,
    WaitingForStep,
    Completed,
}

/// Per-loop instance holding all state for a single running plan loop.
pub struct LoopInstance {
    pub id: usize,
    pub plan: PlanInfo,
    pub config: KruhConfig,
    pub state: LoopState,
    pub iteration: usize,
    pub pass_count: usize,
    pub fail_count: usize,
    pub start_time: Option<Instant>,
    pub loop_phase: LoopPhase,
    pub current_issue_name: Option<String>,
    pub iteration_start_time: Option<Instant>,
    pub output_lines: Vec<OutputLine>,
    pub output_scroll: ScrollHandle,
    pub diff_stat: Option<String>,
    pub progress: StatusProgress,
    pub paused: bool,
    pub step_mode: bool,
    pub skip_requested: bool,
    pub quit_requested: bool,
    pub step_advance_requested: bool,
}

impl LoopInstance {
    pub fn new(id: usize, plan: PlanInfo, config: KruhConfig) -> Self {
        Self {
            id,
            progress: StatusProgress {
                pending: plan.pending,
                done: plan.done,
                total: plan.total,
                ..Default::default()
            },
            plan,
            config,
            state: LoopState::Running,
            iteration: 0,
            pass_count: 0,
            fail_count: 0,
            start_time: Some(Instant::now()),
            loop_phase: LoopPhase::default(),
            current_issue_name: None,
            iteration_start_time: None,
            output_lines: Vec::new(),
            output_scroll: ScrollHandle::new(),
            diff_stat: None,
            paused: false,
            step_mode: false,
            skip_requested: false,
            quit_requested: false,
            step_advance_requested: false,
        }
    }

    pub fn add_output(&mut self, text: &str, is_error: bool) {
        self.output_lines.push(OutputLine {
            text: text.to_string(),
            timestamp: Instant::now(),
            is_error,
        });
        let len = self.output_lines.len();
        if len > 0 {
            self.output_scroll.scroll_to_item(len - 1);
        }
    }
}

// ── Remote-rendering types ────────────────────────────────────────────────────

/// Serializable snapshot of KruhPane — everything a remote client needs to render
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KruhViewState {
    pub app_id: Option<String>,
    pub screen: KruhScreen,
}

/// Tagged enum for each UI screen
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "screen")]
pub enum KruhScreen {
    Scanning,
    PlanPicker {
        plans: Vec<PlanViewInfo>,
        selected_index: usize,
    },
    TaskBrowser {
        plan_name: String,
        issues: Vec<IssueViewInfo>,
    },
    Editing {
        file_path: String,
        content: String,
        is_new: bool,
    },
    Settings {
        model: String,
        max_iterations: usize,
        auto_start: bool,
    },
    LoopOverview {
        loops: Vec<LoopViewInfo>,
        focused_index: usize,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanViewInfo {
    pub name: String,
    pub path: String,
    pub issue_count: usize,
    pub completed_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IssueViewInfo {
    pub number: String,
    pub title: String,
    pub status: String, // "pending", "in_progress", "completed"
    pub priority: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopViewInfo {
    pub loop_id: usize,
    pub plan_name: String,
    pub phase: String, // serialized LoopPhase display name
    pub state: String, // "running", "paused", "waiting", "completed"
    pub current_issue: Option<String>,
    pub progress: ProgressViewInfo,
    pub output_lines: Vec<OutputLineView>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgressViewInfo {
    pub completed: usize,
    pub total: usize,
}

/// OutputLine without Instant (not serializable) — capped at 200 lines in view state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputLineView {
    pub text: String,
    pub is_error: bool,
}

/// All user interactions that can be performed on KruhPane
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action")]
pub enum KruhAction {
    // Navigation
    StartScan,
    SelectPlan { index: usize },
    OpenPlan { name: String },
    BackToPlans,

    // Loop control
    StartLoop { plan_name: String },
    StartAllLoops,
    PauseLoop { loop_id: usize },
    ResumeLoop { loop_id: usize },
    StopLoop { loop_id: usize },
    CloseLoops,
    FocusLoop { index: usize },

    // Editor
    OpenEditor { file_path: String },
    SaveEditor { content: String },
    CloseEditor,

    // Settings
    OpenSettings,
    UpdateSettings { model: String, max_iterations: usize, auto_start: bool },
    CloseSettings,

    // Task browser
    BrowseTasks { plan_name: String },
}

impl okena_core::app_state::AppViewState for KruhViewState {}
impl okena_core::app_state::AppAction for KruhAction {}
