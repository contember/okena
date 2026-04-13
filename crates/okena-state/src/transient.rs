//! Transient (non-persisted) state types.

/// Which edge zone the user dropped onto during pane drag-and-drop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropZone {
    Top,
    Bottom,
    Left,
    Right,
    Center,
}

/// State for focused terminal (for visual indicator)
#[derive(Clone, Debug, PartialEq)]
pub struct FocusedTerminalState {
    pub project_id: String,
    pub layout_path: Vec<usize>,
}

/// Pending worktree close operation waiting for a hook to complete.
#[derive(Clone, Debug)]
pub struct PendingWorktreeClose {
    pub project_id: String,
    pub hook_terminal_id: String,
    /// Data needed for the worktree_removed hook after removal
    pub branch: String,
    pub main_repo_path: String,
}
