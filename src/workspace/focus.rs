//! Focus Management for Terminal Panes
//!
//! This module provides unified focus management across the application.
//! It maintains a focus stack for restoration after modal dismissal or
//! fullscreen exit, and ensures consistent focus event propagation.

/// Identifies a focusable terminal in the workspace
#[derive(Clone, Debug, PartialEq)]
pub struct FocusTarget {
    pub project_id: String,
    pub layout_path: Vec<usize>,
}

impl FocusTarget {
    pub fn new(project_id: String, layout_path: Vec<usize>) -> Self {
        Self {
            project_id,
            layout_path,
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
    target: FocusTarget,
    context: FocusContext,
}

/// Manages focus state and focus stack for the application.
///
/// The FocusManager provides:
/// - Single source of truth for current focus target
/// - Focus stack for restoration after modal/fullscreen exit
/// - Consistent focus event handling
#[derive(Clone, Debug)]
pub struct FocusManager {
    /// Currently focused terminal (if any)
    current_focus: Option<FocusTarget>,
    /// Focus stack for restoration (most recent on top)
    focus_stack: Vec<FocusStackEntry>,
    /// Current focus context
    context: FocusContext,
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
            max_stack_depth: 10,
        }
    }

    /// Get the currently focused terminal target
    #[allow(dead_code)]
    pub fn current(&self) -> Option<&FocusTarget> {
        self.current_focus.as_ref()
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
    /// When entering fullscreen, the current focus is pushed to the stack
    /// so it can be restored when fullscreen exits.
    pub fn enter_fullscreen(&mut self, project_id: String, layout_path: Vec<usize>) {
        // Save current focus to stack if we have one
        if let Some(ref current) = self.current_focus {
            self.push_focus(current.clone(), self.context.clone());
        }

        // Set fullscreen as current focus
        self.current_focus = Some(FocusTarget::new(project_id, layout_path));
        self.context = FocusContext::Fullscreen;
    }

    /// Exit fullscreen mode, restoring previous focus.
    ///
    /// Returns the target that should be focused after exiting fullscreen.
    pub fn exit_fullscreen(&mut self) -> Option<FocusTarget> {
        if self.context != FocusContext::Fullscreen {
            return None;
        }

        // Pop and restore previous focus
        if let Some(entry) = self.pop_focus() {
            self.current_focus = Some(entry.target.clone());
            self.context = entry.context;
            Some(entry.target)
        } else {
            // No saved focus, clear current
            self.current_focus = None;
            self.context = FocusContext::Terminal;
            None
        }
    }

    /// Enter modal context (search, rename, etc.), saving current focus.
    ///
    /// Modal contexts temporarily take focus away from terminals.
    /// The previous focus is saved for restoration when the modal closes.
    pub fn enter_modal(&mut self) {
        // Save current focus to stack if we have one
        if let Some(ref current) = self.current_focus {
            self.push_focus(current.clone(), self.context.clone());
        }

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

        // Pop and restore previous focus
        if let Some(entry) = self.pop_focus() {
            self.current_focus = Some(entry.target.clone());
            self.context = entry.context;
            Some(entry.target)
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
    #[allow(dead_code)]
    pub fn clear_focus(&mut self) {
        self.current_focus = None;
        self.context = FocusContext::Terminal;
    }

    /// Clear the entire focus stack.
    ///
    /// Used when the application state changes significantly
    /// (e.g., workspace reload).
    #[allow(dead_code)]
    pub fn clear_stack(&mut self) {
        self.focus_stack.clear();
    }

    /// Push a focus target onto the stack.
    fn push_focus(&mut self, target: FocusTarget, context: FocusContext) {
        // Enforce max stack depth
        while self.focus_stack.len() >= self.max_stack_depth {
            self.focus_stack.remove(0);
        }

        self.focus_stack.push(FocusStackEntry { target, context });
    }

    /// Pop the most recent focus target from the stack.
    fn pop_focus(&mut self) -> Option<FocusStackEntry> {
        self.focus_stack.pop()
    }

    /// Check if we're in fullscreen context
    #[allow(dead_code)]
    pub fn is_fullscreen(&self) -> bool {
        self.context == FocusContext::Fullscreen
    }

    /// Check if we're in modal context
    #[allow(dead_code)]
    pub fn is_modal(&self) -> bool {
        self.context == FocusContext::Modal
    }

    /// Get the stack depth (for debugging)
    #[allow(dead_code)]
    pub fn stack_depth(&self) -> usize {
        self.focus_stack.len()
    }
}
