//! Focus Management for Terminal Panes
//!
//! This module provides unified focus management across the application.
//! It maintains a focus stack for restoration after modal dismissal or
//! fullscreen exit, and ensures consistent focus event propagation.
//!
//! FocusManager is the single source of truth for:
//! - Which terminal is focused (current_focus)
//! - Which project is zoomed/focused in the sidebar (focused_project_id)
//! - Whether a terminal is in fullscreen/zoom mode (Fullscreen context + terminal_id)

/// Identifies a focusable terminal in the workspace
#[derive(Clone, Debug, PartialEq)]
pub struct FocusTarget {
    pub project_id: String,
    pub layout_path: Vec<usize>,
    /// Terminal ID (set when entering fullscreen to track which terminal is zoomed)
    pub terminal_id: Option<String>,
}

impl FocusTarget {
    pub fn new(project_id: String, layout_path: Vec<usize>) -> Self {
        Self {
            project_id,
            layout_path,
            terminal_id: None,
        }
    }

    pub fn with_terminal(project_id: String, layout_path: Vec<usize>, terminal_id: String) -> Self {
        Self {
            project_id,
            layout_path,
            terminal_id: Some(terminal_id),
        }
    }
}

/// Focus context type for distinguishing different focus scenarios
#[derive(Clone, Debug, PartialEq)]
pub enum FocusContext {
    /// Normal terminal focus
    Terminal,
    /// Fullscreen mode is active
    Fullscreen,
    /// Modal dialog is open (search, rename, etc.)
    Modal,
}

/// Entry in the focus stack for restoration
#[derive(Clone, Debug)]
struct FocusStackEntry {
    target: Option<FocusTarget>,
    context: FocusContext,
    /// Saved focused_project_id at the time of push
    focused_project_id: Option<String>,
}

/// Manages focus state and focus stack for the application.
///
/// The FocusManager is the single source of truth for:
/// - Current terminal focus target
/// - Project zoom/focus state (focused_project_id)
/// - Fullscreen terminal state
/// - Focus stack for restoration after modal/fullscreen exit
#[derive(Clone, Debug)]
pub struct FocusManager {
    /// Currently focused terminal (if any)
    current_focus: Option<FocusTarget>,
    /// Focus stack for restoration (most recent on top)
    focus_stack: Vec<FocusStackEntry>,
    /// Current focus context
    context: FocusContext,
    /// Which project is "zoomed" in the sidebar (only that project's column is visible)
    focused_project_id: Option<String>,
    /// Maximum stack depth to prevent memory issues
    max_stack_depth: usize,
}

impl Default for FocusManager {
    fn default() -> Self {
        Self::new()
    }
}

impl FocusManager {
    pub fn new() -> Self {
        Self {
            current_focus: None,
            focus_stack: Vec::new(),
            context: FocusContext::Terminal,
            focused_project_id: None,
            max_stack_depth: 10,
        }
    }

    /// Get the current focus as FocusedTerminalState for backward compatibility.
    ///
    /// This is the primary method for checking which terminal is focused.
    /// Returns None if no terminal is focused.
    pub fn focused_terminal_state(&self) -> Option<crate::workspace::state::FocusedTerminalState> {
        self.current_focus.as_ref().map(|target| {
            crate::workspace::state::FocusedTerminalState {
                project_id: target.project_id.clone(),
                layout_path: target.layout_path.clone(),
            }
        })
    }

    /// Get the current focus context
    #[allow(dead_code)]
    pub fn context(&self) -> &FocusContext {
        &self.context
    }

    /// Check if a specific terminal is currently focused
    #[allow(dead_code)]
    pub fn is_focused(&self, project_id: &str, layout_path: &[usize]) -> bool {
        self.current_focus.as_ref().map_or(false, |f| {
            f.project_id == project_id && f.layout_path == layout_path
        })
    }

    // --- Focused project ID (project zoom) ---

    /// Get the currently focused/zoomed project ID
    pub fn focused_project_id(&self) -> Option<&String> {
        self.focused_project_id.as_ref()
    }

    /// Set the focused/zoomed project ID
    pub fn set_focused_project_id(&mut self, id: Option<String>) {
        self.focused_project_id = id;
    }

    // --- Fullscreen state queries ---

    /// Get fullscreen state as (project_id, terminal_id) if in fullscreen
    pub fn fullscreen_state(&self) -> Option<(&str, &str)> {
        if self.context != FocusContext::Fullscreen {
            return None;
        }
        self.current_focus.as_ref().and_then(|f| {
            f.terminal_id.as_deref().map(|tid| (f.project_id.as_str(), tid))
        })
    }

    /// Check if a specific terminal is currently fullscreened
    pub fn is_terminal_fullscreened(&self, project_id: &str, terminal_id: &str) -> bool {
        self.fullscreen_state()
            .map_or(false, |(pid, tid)| pid == project_id && tid == terminal_id)
    }

    /// Check if any terminal is in fullscreen mode
    pub fn has_fullscreen(&self) -> bool {
        self.context == FocusContext::Fullscreen
            && self.current_focus.as_ref().map_or(false, |f| f.terminal_id.is_some())
    }

    /// Get the project ID of the fullscreened terminal (if any)
    pub fn fullscreen_project_id(&self) -> Option<&str> {
        self.fullscreen_state().map(|(pid, _)| pid)
    }

    // --- Focus actions ---

    /// Focus a terminal pane.
    ///
    /// This is the primary method for focusing a terminal. It:
    /// - Updates the current focus target
    /// - Does NOT push to stack (direct user action)
    pub fn focus_terminal(&mut self, project_id: String, layout_path: Vec<usize>) {
        self.current_focus = Some(FocusTarget::new(project_id, layout_path));
        self.context = FocusContext::Terminal;
    }

    /// Enter fullscreen mode, saving current focus for restoration.
    ///
    /// When entering fullscreen, the current focus and focused_project_id are
    /// pushed to the stack so they can be restored when fullscreen exits.
    pub fn enter_fullscreen(&mut self, project_id: String, layout_path: Vec<usize>, terminal_id: String) {
        // Save current state to stack (target may be None if nothing was focused)
        self.push_focus(self.current_focus.clone(), self.context.clone(), self.focused_project_id.clone());

        // Set fullscreen as current focus
        self.current_focus = Some(FocusTarget::with_terminal(project_id.clone(), layout_path, terminal_id));
        self.context = FocusContext::Fullscreen;

        // Also zoom to the project
        self.focused_project_id = Some(project_id);
    }

    /// Exit fullscreen mode, restoring previous focus and project zoom.
    ///
    /// Returns the target that should be focused after exiting fullscreen.
    pub fn exit_fullscreen(&mut self) -> Option<FocusTarget> {
        if self.context != FocusContext::Fullscreen {
            return None;
        }

        // Pop and restore previous focus + focused_project_id
        if let Some(entry) = self.pop_focus() {
            self.current_focus = entry.target.clone();
            self.context = entry.context;
            self.focused_project_id = entry.focused_project_id;
            entry.target
        } else {
            // No saved focus, clear current
            self.current_focus = None;
            self.context = FocusContext::Terminal;
            self.focused_project_id = None;
            None
        }
    }

    /// Clear fullscreen without restoring the saved focused_project_id.
    ///
    /// Used by set_focused_project() which overrides the project zoom itself.
    /// This avoids exit_fullscreen() restoring an old project_id that would
    /// immediately be overwritten.
    pub fn clear_fullscreen_without_restore(&mut self) {
        if self.context != FocusContext::Fullscreen {
            return;
        }

        // Pop the stack entry but discard it (don't restore focused_project_id)
        let _ = self.pop_focus();
        self.context = FocusContext::Terminal;
        // Don't clear current_focus - the caller (set_focused_project) will set new focus
    }

    /// Enter modal context (search, rename, etc.), saving current focus.
    ///
    /// Modal contexts temporarily take focus away from terminals.
    /// The previous focus is saved for restoration when the modal closes.
    pub fn enter_modal(&mut self) {
        // Save current focus to stack (including focused_project_id)
        self.push_focus(self.current_focus.clone(), self.context.clone(), self.focused_project_id.clone());

        // Don't clear current_focus - we just change context
        // This allows the visual indicator to remain while modal is open
        self.context = FocusContext::Modal;
    }

    /// Exit modal context, restoring previous focus.
    ///
    /// Returns the target that should be focused after exiting the modal.
    pub fn exit_modal(&mut self) -> Option<FocusTarget> {
        if self.context != FocusContext::Modal {
            return self.current_focus.clone();
        }

        // Pop and restore previous focus + focused_project_id
        if let Some(entry) = self.pop_focus() {
            self.current_focus = entry.target.clone();
            self.context = entry.context;
            self.focused_project_id = entry.focused_project_id;
            entry.target
        } else {
            // No saved focus - restore to terminal context but keep current
            self.context = FocusContext::Terminal;
            self.current_focus.clone()
        }
    }

    /// Clear current focus without affecting the stack.
    ///
    /// Used when focus should be removed but not restored later
    /// (e.g., terminal closed).
    pub fn clear_focus(&mut self) {
        self.current_focus = None;
        self.context = FocusContext::Terminal;
    }

    /// Clear all focus state: current focus, focused_project_id, and stack.
    ///
    /// Used when switching workspaces to reset everything.
    pub fn clear_all(&mut self) {
        self.current_focus = None;
        self.context = FocusContext::Terminal;
        self.focused_project_id = None;
        self.focus_stack.clear();
    }

    /// Push a focus entry onto the stack.
    fn push_focus(&mut self, target: Option<FocusTarget>, context: FocusContext, focused_project_id: Option<String>) {
        // Enforce max stack depth
        while self.focus_stack.len() >= self.max_stack_depth {
            self.focus_stack.remove(0);
        }

        self.focus_stack.push(FocusStackEntry { target, context, focused_project_id });
    }

    /// Pop the most recent focus entry from the stack.
    fn pop_focus(&mut self) -> Option<FocusStackEntry> {
        self.focus_stack.pop()
    }

    /// Check if we're in modal context
    pub fn is_modal(&self) -> bool {
        self.context == FocusContext::Modal
    }

}
