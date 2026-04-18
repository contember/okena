use alacritty_terminal::term::test::TermSize;
use alacritty_terminal::term::{Config as TermConfig, Term};
use alacritty_terminal::vte::ansi::{CursorShape as VteCursorShape, CursorStyle as VteCursorStyle, Processor};
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::Arc;
use std::time::Instant;

mod ansi_snapshot;
mod app_version;
mod child_processes;
mod event_listener;
mod idle;
mod io;
mod links;
mod meta;
mod mouse;
mod modes;
mod osc_sidecar;
mod prompt_jump;
mod prompt_marks;
mod render;
mod resize;
mod resize_authority;
mod scroll;
mod search;
mod selection;
mod transport;
mod types;
mod url_detect;

#[cfg(test)]
mod tests;

pub use app_version::set_app_version;
pub use child_processes::has_child_processes;
pub use resize_authority::{
    claim_resize_authority_local, claim_resize_authority_remote, is_resize_authority_local,
};
pub use transport::TerminalTransport;
pub use types::{
    AppCursorShape, DetectedLink, PromptMark, PromptMarkKind, ResizeState, SelectionState,
    TerminalSize,
};

use event_listener::ZedEventListener;
use osc_sidecar::OscSidecar;
use prompt_marks::{PromptSidecar, PromptTracker};

/// A terminal instance wrapping alacritty_terminal
/// Terminal emulator state.
///
/// # Threading model
///
/// `Terminal` is always stored behind `Arc` (in `TerminalsRegistry`) and all
/// methods take `&self`, using interior mutability for mutation. Three
/// execution contexts access the struct:
///
/// 1. **GPUI thread** — the main UI thread. Runs `process_output` (via the
///    batched PTY event loop in `Okena`), all rendering (`with_content`),
///    user-input methods, resize, selection, scroll, and idle-detection reads.
///    This is where the vast majority of field access happens.
///
/// 2. **Tokio reader task** (remote connections only) — calls `enqueue_output`
///    to buffer incoming data without holding `term.lock()`. Only touches
///    `pending_output`, `dirty`, and `last_output_time`.
///
/// 3. **Resize debounce timer** — a short-lived `std::thread::spawn` that
///    flushes a trailing-edge resize after the debounce window. Only touches
///    `resize_state` and `transport`.
///
/// The PTY reader OS thread does **not** touch `Terminal` directly — it sends
/// `PtyEvent::Data` through an `async_channel`, which the GPUI thread drains.
///
/// # Synchronization primitives
///
/// - **`Arc<Mutex<T>>`** — the `Arc` is needed when the value is shared with a
///   sub-struct (`ZedEventListener`, `OscSidecar`) or handed to a background
///   thread (`resize_state`). The `Mutex` (from `parking_lot`) provides
///   interior mutability.
///
/// - **`Mutex<T>`** — interior mutability for fields that don't need to be
///   shared outside the `Terminal` struct. All current `Mutex`-only fields are
///   accessed exclusively from the GPUI thread; the `Mutex` is required
///   because `&self` methods need interior mutability, not because multiple
///   threads contend.
///
/// - **`AtomicBool` / `AtomicU64`** — lock-free signaling between the GPUI
///   thread and the tokio reader task (for `dirty`), or between the GPUI
///   thread's output path and its render path (for `content_generation`,
///   `waiting_for_input`, `had_user_input`) to avoid mutex overhead on every
///   frame.
pub struct Terminal {
    // ── Immutable after construction ─────────────────────────────────

    /// Unique identifier for this terminal instance. Immutable after
    /// construction; read freely from any thread.
    pub terminal_id: String,

    /// I/O transport (local PTY or remote WebSocket). Immutable ref after
    /// construction. `Arc` for sharing with `ZedEventListener`, `OscSidecar`,
    /// and the resize debounce timer.
    pub(super) transport: Arc<dyn TerminalTransport>,

    /// Initial working directory passed at creation time. Immutable.
    /// Used as fallback when the shell has not yet reported its cwd via OSC 7.
    pub(super) initial_cwd: String,

    // ── GPUI-thread only ─────────────────────────────────────────────
    // All fields below are accessed exclusively from the GPUI thread.
    // `Mutex` provides interior mutability for `&self` methods, not
    // cross-thread safety.

    /// ANSI parser state (alacritty_terminal `Term`). Locked by
    /// `process_output`, `with_content`, `resize`, `scroll`, and selection
    /// methods — all on the GPUI thread. The `Arc` is structural: it doesn't
    /// get cloned, but `Terminal` requires `Send + Sync` and `Term` is
    /// mutated through `&self`.
    pub(super) term: Arc<Mutex<Term<ZedEventListener>>>,

    /// VTE byte processor. Locked together with `term` in `process_output`
    /// and `drain_pending_output`. GPUI thread only.
    pub(super) processor: Mutex<Processor>,

    /// Mouse/keyboard selection state. GPUI thread only (selection start,
    /// update, finish, cancel — all driven by UI events).
    pub(super) selection_state: Mutex<SelectionState>,

    /// Cumulative scroll delta in the scrollback buffer. GPUI thread only
    /// (scroll, scroll_page). The `Mutex` is for interior mutability; no
    /// cross-thread contention.
    pub(super) scroll_offset: Mutex<i32>,

    /// Terminal title set by OSC 0/1/2 sequences. `Arc` shared with
    /// `ZedEventListener` (which lives inside `Term`): the listener writes
    /// on title-change events during `process_output`, and the GPUI render
    /// path reads via `get_title`. Both happen on the GPUI thread.
    pub(super) title: Arc<Mutex<Option<String>>>,

    /// Bell notification flag. `Arc` shared with `ZedEventListener`: set on
    /// BEL during `process_output`, cleared by the render path on focus.
    /// GPUI thread only.
    pub(super) has_bell: Arc<Mutex<bool>>,

    /// Pending OSC 52 clipboard writes requested by the running app. `Arc`
    /// shared with `ZedEventListener`: pushed during `process_output`,
    /// drained by the GPUI render path via `drain_clipboard_writes`.
    /// GPUI thread only.
    pub(super) pending_clipboard: Arc<Mutex<Vec<String>>>,

    /// Theme palette used to answer OSC 10/11/12/4 color queries from
    /// terminal apps. `Arc` shared with `ZedEventListener`: the render path
    /// pushes the current theme via `push_palette`, and the listener reads
    /// it when composing color-query responses. GPUI thread only.
    pub(super) palette: Arc<Mutex<Option<okena_core::theme::ThemeColors>>>,

    /// Working directory most recently reported by the shell via OSC 7.
    /// `None` until the shell sends its first `ESC ] 7 ; file://...`
    /// sequence. `Arc` shared with `OscSidecar` (the sidecar writes on
    /// parse, GPUI reads via `reported_cwd`). GPUI thread only.
    pub(super) reported_cwd: Arc<Mutex<Option<String>>>,

    /// Pending iTerm2-style `OSC 9` notifications. `Arc` shared with
    /// `OscSidecar`: pushed during `process_output`, drained by the GPUI
    /// render path via `drain_notifications`. GPUI thread only.
    pub(super) pending_notifications: Arc<Mutex<Vec<String>>>,

    /// VTE sidecar parser for OSC/CSI sequences (OSC 7 cwd, OSC 9
    /// notifications, XTVERSION) that alacritty_terminal either ignores or
    /// answers differently than Okena wants. GPUI thread only
    /// (`process_output` and `drain_pending_output`).
    pub(super) osc_sidecar: Mutex<OscSidecar>,

    /// Byte-splitting sidecar for OSC 133 shell-integration marks. Runs
    /// in lockstep with the main `processor` so cursor positions can be
    /// snapshotted at the exact byte each mark arrives. GPUI thread only.
    pub(super) prompt_sidecar: Mutex<PromptSidecar>,

    /// Ring buffer of captured OSC 133 prompt marks. Written during
    /// `process_output`, read by `prompt_marks` and `jump_to_prompt_*`.
    /// GPUI thread only.
    pub(super) prompt_tracker: Mutex<PromptTracker>,

    /// Reverse index into the current list of `PromptStart` marks (0 =
    /// newest). `Some` while the user is walking through prompts with
    /// `jump_to_prompt_above/below`; reset to `None` on any output or
    /// scroll so the next walk starts from the most recent prompt again.
    /// GPUI thread only.
    pub(super) prompt_jump_index: Mutex<Option<usize>>,

    /// Shell process PID. Set by `set_shell_pid` (called from GPUI thread
    /// after PTY spawn), read by `shell_pid` and `has_running_child`.
    /// GPUI thread only.
    pub(super) shell_pid: Mutex<Option<u32>>,

    /// Timestamp of when the user last viewed this terminal (set on blur
    /// via `mark_as_viewed`). Compared against `last_output_time` to
    /// determine `has_unseen_output`. GPUI thread only.
    ///
    /// The `Arc` is historical — the value is never cloned; a plain `Mutex`
    /// would suffice.
    pub(super) last_viewed_time: Arc<Mutex<Instant>>,

    // ── GPUI + resize debounce timer ─────────────────────────────────

    /// Terminal size, debounce state, and pending PTY resize. `Arc` is
    /// required: a clone is handed to the short-lived debounce timer thread
    /// (`std::thread::spawn` in `resize`) which flushes the trailing-edge
    /// resize after the debounce window.
    pub resize_state: Arc<Mutex<ResizeState>>,

    // ── Cross-thread (GPUI + tokio reader task) ──────────────────────
    // These fields are touched by the remote-connection tokio reader task
    // via `enqueue_output`. The tokio task buffers data and sets flags;
    // the GPUI thread drains and clears them.

    /// Buffer for remote-connection output. Written by the tokio reader
    /// task (`enqueue_output`), drained by the GPUI thread
    /// (`drain_pending_output` inside `with_content`). Decouples the tokio
    /// task from `term.lock()`, preventing lock contention that would
    /// freeze the UI.
    pub(super) pending_output: Mutex<Vec<u8>>,

    /// Content-changed flag. Set by `process_output` (GPUI) and
    /// `enqueue_output` (tokio). Cleared by `take_dirty` (GPUI render).
    /// `AtomicBool` for lock-free cross-thread signaling.
    pub(super) dirty: AtomicBool,

    /// Timestamp of last terminal output. Written by `process_output`
    /// (GPUI), `enqueue_output` (tokio), and `clear_waiting` (GPUI). Read
    /// by idle-detection methods on the GPUI thread.
    ///
    /// The `Arc` is historical — the value is never cloned; a plain `Mutex`
    /// would suffice since `Terminal` is already behind `Arc`.
    pub(super) last_output_time: Arc<Mutex<Instant>>,

    // ── Atomics (lock-free render reads) ─────────────────────────────
    // These use atomics so the GPUI render path can read them without
    // taking a mutex on every frame.

    /// Monotonically-increasing counter bumped on every `process_output`,
    /// `drain_pending_output`, resize, scroll, and selection change. Used
    /// by `UrlDetector` and `SearchBar` to skip redundant work when
    /// content hasn't changed. GPUI thread only (despite being atomic —
    /// the atomic avoids locking, not cross-thread access).
    pub(super) content_generation: AtomicU64,

    /// Cached "waiting for input" state. Written by the GPUI idle-check
    /// loop (`set_waiting_for_input`), read lock-free by renderers
    /// (`is_waiting_for_input`). Atomic avoids mutex overhead in the
    /// render hot path.
    pub(super) waiting_for_input: AtomicBool,

    /// Whether the user has ever typed into this terminal. Set on
    /// `send_input` / `send_paste` / `send_raw_input` (GPUI thread), read
    /// lock-free by the idle-detection loop. Prevents flagging fresh
    /// terminals as idle before the user has interacted.
    pub(super) had_user_input: AtomicBool,
}

impl Terminal {
    /// Create a new terminal
    pub fn new(
        terminal_id: String,
        size: TerminalSize,
        transport: Arc<dyn TerminalTransport>,
        initial_cwd: String,
    ) -> Self {
        // Use HollowBlock as a sentinel for "app has not set a cursor shape
        // via DECSCUSR" — no real DECSCUSR code maps to HollowBlock, so if
        // `cursor_style()` returns it we know to fall back to the user
        // setting instead of honoring an app override.
        let config = TermConfig {
            default_cursor_style: VteCursorStyle {
                shape: VteCursorShape::HollowBlock,
                blinking: false,
            },
            ..TermConfig::default()
        };
        let term_size = TermSize::new(size.cols as usize, size.rows as usize);

        // Create shared storage for OSC sequence handling and bell
        let title = Arc::new(Mutex::new(None));
        let has_bell = Arc::new(Mutex::new(false));
        let pending_clipboard = Arc::new(Mutex::new(Vec::new()));
        let palette = Arc::new(Mutex::new(None));
        let event_listener = ZedEventListener::new(
            title.clone(),
            has_bell.clone(),
            pending_clipboard.clone(),
            palette.clone(),
            transport.clone(),
            terminal_id.clone(),
        );
        let term = Term::new(config, &term_size, event_listener);

        let reported_cwd = Arc::new(Mutex::new(None));
        let pending_notifications = Arc::new(Mutex::new(Vec::new()));
        let osc_sidecar = Mutex::new(OscSidecar::new(
            reported_cwd.clone(),
            pending_notifications.clone(),
            transport.clone(),
            terminal_id.clone(),
        ));

        Self {
            term: Arc::new(Mutex::new(term)),
            processor: Mutex::new(Processor::new()),
            terminal_id,
            resize_state: Arc::new(Mutex::new(ResizeState::new(size))),
            transport,
            selection_state: Mutex::new(SelectionState::default()),
            scroll_offset: Mutex::new(0),
            title,
            has_bell,
            pending_clipboard,
            palette,
            pending_output: Mutex::new(Vec::new()),
            dirty: AtomicBool::new(false),
            content_generation: AtomicU64::new(0),
            initial_cwd,
            reported_cwd,
            pending_notifications,
            osc_sidecar,
            prompt_sidecar: Mutex::new(PromptSidecar::new()),
            prompt_tracker: Mutex::new(PromptTracker::new()),
            prompt_jump_index: Mutex::new(None),
            last_output_time: Arc::new(Mutex::new(Instant::now())),
            shell_pid: Mutex::new(None),
            waiting_for_input: AtomicBool::new(false),
            had_user_input: AtomicBool::new(false),
            last_viewed_time: Arc::new(Mutex::new(Instant::now())),
        }
    }
}
