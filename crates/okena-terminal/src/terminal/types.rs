/// Terminal size in cells and pixels
#[derive(Clone, Copy, Debug)]
pub struct TerminalSize {
    pub cols: u16,
    pub rows: u16,
    pub cell_width: f32,
    pub cell_height: f32,
}

impl Default for TerminalSize {
    fn default() -> Self {
        Self {
            cols: 80,
            rows: 24,
            cell_width: 8.0,
            cell_height: 16.0,
        }
    }
}

#[derive(Default)]
pub(super) struct FocusReportState {
    pub(super) viewers: std::collections::HashMap<u64, bool>,
    pub(super) last_reported: Option<bool>,
}

/// Which way `jump_to_prompt` looks relative to the currently visible top.
#[derive(Clone, Copy, Debug)]
pub(super) enum JumpDirection {
    Above,
    Below,
}

/// Kind of an OSC 133 shell-integration mark.
///
/// Shells that implement OSC 133 emit one of these four sequences at
/// well-defined points in the prompt/command lifecycle. Each kind carries
/// no positional data itself — the row/column are captured by Okena when
/// the sidecar sees the sequence arrive on the byte stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptMarkKind {
    /// `OSC 133 ; A` — beginning of a new prompt (before the user can type).
    PromptStart,
    /// `OSC 133 ; B` — end of prompt, the shell is ready for input.
    CommandStart,
    /// `OSC 133 ; C` — user hit Enter, the command is now executing.
    CommandExecuted,
    /// `OSC 133 ; D [; <exit_code>]` — the command finished. The exit code
    /// is optional because some shells elide it on interactive prompts.
    CommandFinished { exit_code: Option<i32> },
}

/// A shell-integration mark with its captured grid position.
///
/// `line` is in alacritty's `Line` coordinate system: `0..screen_lines-1`
/// is the viewport and negative values live in scrollback. The tracker
/// rebases `line` as content scrolls so the value stays valid across
/// command output — up to the scrollback cap.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PromptMark {
    pub kind: PromptMarkKind,
    pub line: i32,
    pub column: usize,
}

/// Cursor shape requested by the terminal application via DECSCUSR.
///
/// Maps onto the three shapes Okena's renderer knows how to paint.
/// Alacritty's `HollowBlock` is used internally as a sentinel for "app has
/// not set any shape" and never reaches callers as this type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppCursorShape {
    Block,
    Bar,
    Underline,
}

/// Selection state for the terminal
#[derive(Clone, Debug)]
pub struct SelectionState {
    pub start: Option<(usize, usize)>,
    pub end: Option<(usize, usize)>,
    pub is_selecting: bool,
}

impl Default for SelectionState {
    fn default() -> Self {
        Self {
            start: None,
            end: None,
            is_selecting: false,
        }
    }
}

/// A detected link in terminal content (URL or file path)
#[derive(Clone, Debug)]
pub struct DetectedLink {
    pub line: i32,
    pub col: usize,
    pub len: usize,
    pub text: String,
    pub file_line: Option<u32>,
    pub file_col: Option<u32>,
    pub is_url: bool,
    /// Segments of the same wrapped URL share the same wrap_group.
    /// Different occurrences of the same URL get different wrap_groups.
    pub wrap_group: usize,
}

/// Consolidated resize-related state, protected by a single mutex
pub struct ResizeState {
    pub size: TerminalSize,
    pub(super) last_pty_resize: std::time::Instant,
    pub(super) pending_pty_resize: Option<(u16, u16)>,
    /// True when a background flush timer is scheduled to send the pending resize.
    pub(super) flush_timer_active: bool,
    /// Timestamp of the last local resize (from TerminalElement::paint).
    /// Used to suppress redundant server resize echoes in remote mode.
    pub last_local_resize: std::time::Instant,
}

impl ResizeState {
    pub(super) fn new(size: TerminalSize) -> Self {
        Self {
            size,
            // Use a time in the past so the first resize from paint() always
            // passes the debounce check and sends SIGWINCH to the PTY immediately
            last_pty_resize: std::time::Instant::now() - std::time::Duration::from_secs(1),
            flush_timer_active: false,
            pending_pty_resize: None,
            last_local_resize: std::time::Instant::now() - std::time::Duration::from_secs(1),
        }
    }
}
