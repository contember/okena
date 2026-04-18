use alacritty_terminal::event::EventListener;
use alacritty_terminal::index::Point;
use alacritty_terminal::term::Term;
use alacritty_terminal::vte::Perform;
use alacritty_terminal::vte::ansi::Processor;
use std::collections::VecDeque;

use super::types::{PromptMark, PromptMarkKind};

/// Ring buffer of recent OSC 133 marks plus a best-effort scroll tracker.
///
/// **Scrollback-cap caveat**: the tracker rebases marks by watching
/// `grid.history_size()` grow — which is exact until the user hits the
/// configured scrollback limit. Past that, the grid starts evicting from
/// the top without changing `history_size`, so mark line values drift by
/// the number of post-cap scrolls. Follow-up work can tighten this by
/// counting linefeeds directly; until then, callers should treat marks
/// older than the scrollback as "approximate" and prefer jumping to the
/// most recent few.
pub(crate) struct PromptTracker {
    marks: VecDeque<PromptMark>,
    /// Oldest-first ring buffer cap. Shells that run thousands of commands
    /// don't need thousands of marks — the UX only looks at the last few.
    capacity: usize,
}

impl PromptTracker {
    pub(super) fn new() -> Self {
        Self { marks: VecDeque::with_capacity(64), capacity: 64 }
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

    /// Shift all stored marks upward by the number of lines that just
    /// scrolled into history. Marks whose new `line` falls off the top of
    /// the grid (below `-history_size`) are dropped.
    pub(super) fn on_history_changed(&mut self, before: usize, after: usize, topmost: i32) {
        let delta = after.saturating_sub(before);
        if delta == 0 {
            return;
        }
        let delta_i32 = delta as i32;
        self.marks.retain_mut(|mark| {
            mark.line -= delta_i32;
            mark.line >= topmost
        });
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
                    if s.is_empty() { None } else { s.parse::<i32>().ok() }
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
            perform: PromptSidecarPerform { pending: None },
        }
    }
}

struct PromptSidecarPerform {
    pending: Option<PromptMarkKind>,
}

impl Perform for PromptSidecarPerform {
    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        // We only care about `OSC 133 ; X [; args...]`. Everything else
        // is handled by the observer sidecar or ignored.
        if params.first().copied() != Some(b"133".as_ref()) {
            return;
        }
        let Some(kind_param) = params.get(1) else { return };
        let Some(&kind_byte) = kind_param.first() else { return };
        if let Some(kind) = parse_osc133_kind(kind_byte, &params[2..]) {
            self.pending = Some(kind);
        }
    }

    fn terminated(&self) -> bool {
        self.pending.is_some()
    }
}

/// Feed `data` to both the main alacritty processor and the prompt sidecar
/// in lockstep. Whenever the sidecar sees an `OSC 133` sequence it flags
/// itself as terminated; we advance the main processor up to the same byte
/// offset (so the cursor is at its post-OSC position, which is unchanged
/// since OSC sequences are zero-width) and then record the mark.
pub(super) fn advance_with_prompt_marks<L: EventListener>(
    term: &mut Term<L>,
    processor: &mut Processor,
    sidecar: &mut PromptSidecar,
    tracker: &mut PromptTracker,
    data: &[u8],
) {
    let mut pos = 0;
    while pos < data.len() {
        let consumed = sidecar
            .parser
            .advance_until_terminated(&mut sidecar.perform, &data[pos..]);
        processor.advance(term, &data[pos..pos + consumed]);
        if let Some(kind) = sidecar.perform.pending.take() {
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
}
