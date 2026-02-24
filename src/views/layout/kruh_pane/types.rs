use std::time::Instant;

use super::config::KruhPlanOverrides;

#[derive(Clone, Debug, Default, PartialEq)]
pub enum KruhState {
    #[default]
    Scanning,
    PlanPicker,
    TaskBrowser,
    Editing,
    Settings,
    Running,
    Paused,
    WaitingForStep,
    Completed,
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
