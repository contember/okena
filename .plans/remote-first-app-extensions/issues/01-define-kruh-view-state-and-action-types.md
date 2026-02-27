# Issue 01: Define KruhViewState and KruhAction types

**Priority:** high
**Files:** `src/views/layout/kruh_pane/types.rs`, `crates/okena-core/src/app_state.rs` (new), `crates/okena-core/src/lib.rs`

## Description

Add serializable view-state and action types that describe KruhPane's full UI state as pure data — no GPUI handles, no `Instant`, no `Task<>`. These types are the foundation for remote rendering.

## Implementation

### 1. `crates/okena-core/src/app_state.rs` (new file)

Create marker traits that all app extensions implement:

```rust
/// Marker trait for app view states — must be serializable to JSON
pub trait AppViewState: serde::Serialize + serde::de::DeserializeOwned + Send + 'static {}

/// Marker trait for app actions — must be serializable to JSON
pub trait AppAction: serde::Serialize + serde::de::DeserializeOwned + Send + 'static {}
```

### 2. `crates/okena-core/src/lib.rs`

Add `pub mod app_state;`

### 3. `src/views/layout/kruh_pane/types.rs`

Add the following types (alongside existing types, do not remove anything):

```rust
use serde::{Deserialize, Serialize};

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
```

Implement `AppViewState` for `KruhViewState` and `AppAction` for `KruhAction`.

## Acceptance Criteria

- All new types compile and derive `Serialize`/`Deserialize`
- `KruhScreen` uses `#[serde(tag = "screen")]` for clean JSON
- `KruhAction` uses `#[serde(tag = "action")]` for clean JSON
- `OutputLineView` has no `Instant` field
- Existing types (`KruhState`, `LoopInstance`, `OutputLine`, etc.) are NOT modified
- `cargo build` succeeds
