use alacritty_terminal::event::EventListener;
use alacritty_terminal::index::Point;
use alacritty_terminal::term::{Term, TermMode};
use alacritty_terminal::vte::Perform;
use alacritty_terminal::vte::ansi::Processor;
use std::collections::VecDeque;

use super::scrollback::Scrollback;
use super::types::{PromptMark, PromptMarkKind};

/// Ring buffer of recent OSC 133 marks plus a scroll tracker. Mark lines use
/// the history-wide numbering (see `scrollback.rs`), so a mark keeps its line
/// when its row moves from the grid into the compressed archive.
///
/// **History-limit caveat**: the tracker counts the rows that entered the
/// history by watching its size grow. Rows alacritty drops at its own limit
/// inside one parser step do not count, so marks drift by that many lines:
/// with `scrollback_lines` at or below the live rows (the grid is the whole
/// history and stays full), or when one step adds more rows than the grid has
/// room for above the live rows (a large synchronized update). A resize that
/// reflows rows moves them without moving the marks. Treat marks older than
/// such an event as approximate.
pub(crate) struct PromptTracker {
    marks: VecDeque<PromptMark>,
    /// Oldest-first ring buffer cap. Shells that run thousands of commands
    /// don't need thousands of marks — the UX only looks at the last few.
    capacity: usize,
}

impl PromptTracker {
    pub(super) fn new() -> Self {
        Self {
            marks: VecDeque::with_capacity(64),
            capacity: 64,
        }
    }

    /// Record a new mark at the given grid point. Evicts the oldest mark
    /// when the ring buffer is full.
    fn record(&mut self, kind: PromptMarkKind, point: Point) {
        if self.marks.len() == self.capacity {
            self.marks.pop_front();
        }
        self.marks.push_back(PromptMark {
            kind,
            line: point.line.0,
            column: point.column.0,
        });
    }

    /// Shift all stored marks upward by the `rows` that just scrolled into
    /// history. Marks that fall above `topmost` are dropped.
    pub(super) fn on_rows_entered(&mut self, rows: i64, topmost: i32) {
        if rows <= 0 {
            return;
        }
        let rows = rows.min(i64::from(i32::MAX)) as i32;
        self.marks.retain_mut(|mark| {
            mark.line = mark.line.saturating_sub(rows);
            mark.line >= topmost
        });
    }

    /// Drop marks that no longer have a row: above `topmost` after the
    /// scrollback shrank or was erased.
    pub(super) fn drop_marks_above(&mut self, topmost: i32) {
        self.marks.retain(|mark| mark.line >= topmost);
    }

    pub(super) fn snapshot(&self) -> Vec<PromptMark> {
        self.marks.iter().copied().collect()
    }
}

/// Parse the body of an `OSC 133 ; <kind> [; <args...>]` sequence into a
/// [`PromptMarkKind`]. Returns `None` for unrecognized kind bytes; extra
/// key=value parameters (e.g. `aid=...`, `cl=...` used by some shells)
/// are ignored.
pub(super) fn parse_osc133_kind(kind: u8, rest: &[&[u8]]) -> Option<PromptMarkKind> {
    match kind {
        b'A' => Some(PromptMarkKind::PromptStart),
        b'B' => Some(PromptMarkKind::CommandStart),
        b'C' => Some(PromptMarkKind::CommandExecuted),
        b'D' => {
            // First sub-param, when present and purely numeric, is the
            // exit code. Anything else (key=value metadata, junk) means
            // "unknown exit".
            let exit_code = rest
                .first()
                .and_then(|p| std::str::from_utf8(p).ok())
                .and_then(|s| {
                    let s = s.trim();
                    if s.is_empty() {
                        None
                    } else {
                        s.parse::<i32>().ok()
                    }
                });
            Some(PromptMarkKind::CommandFinished { exit_code })
        }
        _ => None,
    }
}

/// Byte-splitting sidecar for OSC 133. Unlike the observer-only
/// [`super::osc_sidecar::OscSidecar`], this one uses `advance_until_terminated`
/// so the caller can snapshot the main processor's cursor position at the
/// exact byte where the mark arrived.
pub(crate) struct PromptSidecar {
    parser: alacritty_terminal::vte::Parser,
    perform: PromptSidecarPerform,
}

impl PromptSidecar {
    pub(super) fn new() -> Self {
        Self {
            parser: alacritty_terminal::vte::Parser::new(),
            perform: PromptSidecarPerform {
                pending: None,
                history_erase: None,
            },
        }
    }
}

struct PromptSidecarPerform {
    pending: Option<PromptMarkKind>,
    /// An erase of the saved lines (`CSI 3 J`) or a full reset (`ESC c`)
    /// arrived: the compressed archive has to go the way alacritty's grid
    /// history does.
    history_erase: Option<HistoryErase>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum HistoryErase {
    /// `CSI 3 J`: alacritty erases the history of the active screen only.
    SavedLines,
    /// `ESC c`: both screens are reset.
    Reset,
}

impl Perform for PromptSidecarPerform {
    fn csi_dispatch(
        &mut self,
        params: &alacritty_terminal::vte::Params,
        intermediates: &[u8],
        ignore: bool,
        action: char,
    ) {
        // vte's own reading: the first subparameter of the first parameter.
        if action == 'J'
            && !ignore
            && intermediates.is_empty()
            && matches!(params.iter().next(), Some(&[3, ..]))
        {
            self.history_erase = Some(HistoryErase::SavedLines);
        }
    }

    fn esc_dispatch(&mut self, intermediates: &[u8], _ignore: bool, byte: u8) {
        if byte == b'c' && intermediates.is_empty() {
            self.history_erase = Some(HistoryErase::Reset);
        }
    }

    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        // We only care about `OSC 133 ; X [; args...]`. Everything else
        // is handled by the observer sidecar or ignored.
        if params.first().copied() != Some(b"133".as_ref()) {
            return;
        }
        let Some(kind_param) = params.get(1) else {
            return;
        };
        let Some(&kind_byte) = kind_param.first() else {
            return;
        };
        if let Some(kind) = parse_osc133_kind(kind_byte, &params[2..]) {
            self.pending = Some(kind);
        }
    }

    fn terminated(&self) -> bool {
        self.pending.is_some() || self.history_erase.is_some()
    }
}

/// Most bytes the main processor parses before history past the live rows is
/// archived. alacritty's grid may hold the whole scrollback, so this bounds how
/// many uncompressed rows one step can allocate, not what is kept.
const ADVANCE_STEP_BYTES: usize = 4096;

/// Feed `data` to both the main alacritty processor and the prompt sidecar
/// in lockstep. Whenever the sidecar sees an `OSC 133` sequence it flags
/// itself as terminated; we advance the main processor up to the same byte
/// offset (so the cursor is at its post-OSC position, which is unchanged
/// since OSC sequences are zero-width) and then record the mark. The sidecar
/// also stops at an erase of the saved lines, so the archive is erased with
/// the grid's history and rows archived after it survive.
///
/// After every step, history past the live rows moves to the archive, and
/// marks and the selection follow the rows that scrolled into history.
///
/// Returns `true` if at least one `CommandFinished` (OSC 133 ;D) mark was
/// recorded in this chunk, so the caller can raise the per-terminal
/// command-finished activity edge exactly once per drain.
pub(super) fn advance_with_prompt_marks<L: EventListener>(
    term: &mut Term<L>,
    processor: &mut Processor,
    sidecar: &mut PromptSidecar,
    tracker: &mut PromptTracker,
    scrollback: &mut Scrollback,
    data: &[u8],
) -> bool {
    let mut command_finished = false;
    let mut pos = 0;
    while pos < data.len() {
        let consumed = sidecar
            .parser
            .advance_until_terminated(&mut sidecar.perform, &data[pos..]);
        for step in data[pos..pos + consumed].chunks(ADVANCE_STEP_BYTES) {
            let before = scrollback.history_mark(term);
            processor.advance(term, step);
            // An erase of the saved lines shrinks the history without moving
            // the screen.
            let rows = scrollback.rows_entered(term, before).max(0);
            scrollback.follow_output(term, rows);
            scrollback.drain(term);
            let topmost = -(scrollback.history_size(term) as i32);
            tracker.on_rows_entered(rows, topmost);
        }
        if let Some(erase) = sidecar.perform.history_erase.take() {
            let alt_screen = term.mode().contains(TermMode::ALT_SCREEN);
            if erase == HistoryErase::Reset || !alt_screen {
                scrollback.clear_archive(term);
                let topmost = -(scrollback.history_size(term) as i32);
                tracker.drop_marks_above(topmost);
            }
        }
        if let Some(kind) = sidecar.perform.pending.take() {
            if matches!(kind, PromptMarkKind::CommandFinished { .. }) {
                command_finished = true;
            }
            let point = term.grid().cursor.point;
            tracker.record(kind, point);
        }
        if consumed == 0 {
            // Safety net: `advance_until_terminated` is expected to make
            // progress on every call (at least one byte per inner loop
            // iteration) as long as `terminated()` was false on entry.
            // If the parser ever stalls anyway, bail rather than spin.
            break;
        }
        pos += consumed;
    }
    command_finished
}
