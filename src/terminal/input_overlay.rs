//! Optimistic input prediction for remote terminals (Mosh-style local echo).
//!
//! Predicted characters render instantly on the client with a visual hint,
//! then get reconciled when the server confirms or invalidates them.
//! Pure logic — no GPUI dependency.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// A single predicted character cell.
#[derive(Clone, Debug)]
pub struct PredictedCell {
    pub col: usize,
    pub row: i32,
    pub character: char,
    pub width: u8,
    pub input_seq: u64,
    pub created_at: Instant,
}

/// State of the prediction engine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PredictionState {
    /// Actively predicting characters.
    Active,
    /// Temporarily paused after a cursor jump (clears after 500ms of stability).
    Tentative,
    /// Disabled (not used currently, reserved for future use).
    Disabled,
}

/// Prediction state machine for a single terminal.
pub struct InputOverlay {
    predictions: VecDeque<PredictedCell>,
    predicted_cursor: Option<(usize, i32)>,
    last_server_cursor: (usize, i32),
    cursor_row_stable_count: u8,
    state: PredictionState,
    next_input_seq: u64,
    last_acked_seq: u64,
    prediction_timeout: Duration,
    tentative_since: Option<Instant>,
    epoch: u64,
    cols: usize,
}

/// Duration after which unacked predictions are garbage-collected.
const DEFAULT_PREDICTION_TIMEOUT: Duration = Duration::from_millis(150);

/// Duration after which Tentative state clears if cursor is stable.
const TENTATIVE_CLEAR_DURATION: Duration = Duration::from_millis(500);

/// Minimum server frames with stable cursor row before predictions are allowed.
const MIN_STABLE_FRAMES: u8 = 3;

impl InputOverlay {
    pub fn new() -> Self {
        Self {
            predictions: VecDeque::new(),
            predicted_cursor: None,
            last_server_cursor: (0, 0),
            cursor_row_stable_count: 0,
            state: PredictionState::Active,
            next_input_seq: 1,
            last_acked_seq: 0,
            prediction_timeout: DEFAULT_PREDICTION_TIMEOUT,
            tentative_since: None,
            epoch: 0,
            cols: 80,
        }
    }

    /// Update the column count (call when terminal is resized).
    pub fn set_cols(&mut self, cols: usize) {
        self.cols = cols;
    }

    /// Check whether a character should be predicted.
    pub fn should_predict(&self, c: char, is_remote: bool) -> bool {
        if !is_remote {
            return false;
        }
        if self.state != PredictionState::Active {
            return false;
        }
        if self.cursor_row_stable_count < MIN_STABLE_FRAMES {
            return false;
        }
        // Only printable ASCII + space + non-control Unicode
        c.is_ascii_graphic() || c == ' ' || (!c.is_control() && !c.is_ascii())
    }

    /// Predict a character at the current predicted cursor position.
    /// Returns the assigned input sequence number, or None if prediction was skipped.
    pub fn predict_char(&mut self, c: char, server_cursor: (usize, i32), cols: usize) -> Option<u64> {
        self.cols = cols;

        if !self.should_predict(c, true) {
            return None;
        }

        let (cursor_col, cursor_row) = self.predicted_cursor.unwrap_or(server_cursor);

        // Determine character width (CJK = 2, others = 1)
        let width = if is_wide_char(c) { 2u8 } else { 1u8 };

        // Don't predict if cursor is at or past the end of line
        if cursor_col >= cols || cursor_col + width as usize > cols {
            return None;
        }

        let seq = self.next_input_seq;
        self.next_input_seq += 1;

        self.predictions.push_back(PredictedCell {
            col: cursor_col,
            row: cursor_row,
            character: c,
            width,
            input_seq: seq,
            created_at: Instant::now(),
        });

        // Advance predicted cursor
        let new_col = cursor_col + width as usize;
        if new_col >= cols {
            // At end of line — stop predicting further by not advancing cursor
            // The predicted_cursor is set to None-equivalent: mark that we're at the edge
            self.predicted_cursor = Some((cols, cursor_row));
        } else {
            self.predicted_cursor = Some((new_col, cursor_row));
        }

        Some(seq)
    }

    /// Called when a server frame arrives with the latest acked sequence and cursor position.
    pub fn on_server_frame(&mut self, acked_seq: u64, server_cursor: (usize, i32)) {
        // Track cursor row stability
        if server_cursor.1 != self.last_server_cursor.1 {
            self.cursor_row_stable_count = 0;
        } else {
            self.cursor_row_stable_count = self.cursor_row_stable_count.saturating_add(1);
        }
        self.last_server_cursor = server_cursor;

        // Ack predictions up to acked_seq
        if acked_seq > self.last_acked_seq {
            self.last_acked_seq = acked_seq;
            while let Some(front) = self.predictions.front() {
                if front.input_seq <= acked_seq {
                    self.predictions.pop_front();
                } else {
                    break;
                }
            }
        }

        // Detect cursor row mismatch (server jumped to different row than predicted)
        if !self.predictions.is_empty() {
            if let Some((_, predicted_row)) = self.predicted_cursor {
                if server_cursor.1 != predicted_row {
                    self.discard_all();
                    self.state = PredictionState::Tentative;
                    self.tentative_since = Some(Instant::now());
                    return;
                }
            }
        }

        // If no predictions remain, re-sync predicted cursor with server
        if self.predictions.is_empty() {
            self.predicted_cursor = None;
        }

        // Handle tentative state clearing
        if self.state == PredictionState::Tentative {
            if let Some(since) = self.tentative_since {
                if since.elapsed() >= TENTATIVE_CLEAR_DURATION
                    && self.cursor_row_stable_count >= MIN_STABLE_FRAMES
                {
                    self.state = PredictionState::Active;
                    self.tentative_since = None;
                }
            }
        }
    }

    /// Discard all predictions and increment epoch.
    pub fn discard_all(&mut self) {
        self.predictions.clear();
        self.predicted_cursor = None;
        self.epoch += 1;
    }

    /// Remove predictions that have exceeded the timeout.
    pub fn gc_expired(&mut self) {
        let now = Instant::now();
        let timeout = self.prediction_timeout;
        while let Some(front) = self.predictions.front() {
            if now.duration_since(front.created_at) >= timeout {
                self.predictions.pop_front();
            } else {
                break;
            }
        }
        if self.predictions.is_empty() {
            self.predicted_cursor = None;
        }
    }

    /// Get the current prediction cells for rendering.
    pub fn cells(&self) -> &VecDeque<PredictedCell> {
        &self.predictions
    }

    /// Get the predicted cursor position (if predictions are active).
    pub fn predicted_cursor(&self) -> Option<(usize, i32)> {
        self.predicted_cursor
    }

    /// Current epoch (incremented on each discard_all).
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// Current prediction state.
    pub fn state(&self) -> PredictionState {
        self.state
    }

    /// The next sequence number that will be assigned.
    pub fn next_input_seq(&self) -> u64 {
        self.next_input_seq
    }
}

/// Heuristic for CJK wide characters.
fn is_wide_char(c: char) -> bool {
    let cp = c as u32;
    // CJK Unified Ideographs, CJK Compatibility Ideographs, Hangul Syllables, etc.
    matches!(cp,
        0x1100..=0x115F |
        0x2E80..=0x303E |
        0x3041..=0x33BF |
        0x3400..=0x4DBF |
        0x4E00..=0x9FFF |
        0xA000..=0xA4CF |
        0xAC00..=0xD7AF |
        0xF900..=0xFAFF |
        0xFE30..=0xFE6F |
        0xFF01..=0xFF60 |
        0xFFE0..=0xFFE6 |
        0x20000..=0x2FFFF |
        0x30000..=0x3FFFF
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_overlay() -> InputOverlay {
        let mut overlay = InputOverlay::new();
        // Pre-warm stability count
        for _ in 0..MIN_STABLE_FRAMES {
            overlay.on_server_frame(0, (0, 0));
        }
        overlay
    }

    #[test]
    fn predict_char_advances_cursor() {
        let mut overlay = make_overlay();
        let seq = overlay.predict_char('a', (0, 0), 80);
        assert_eq!(seq, Some(1));
        assert_eq!(overlay.predicted_cursor(), Some((1, 0)));

        let seq = overlay.predict_char('b', (0, 0), 80);
        assert_eq!(seq, Some(2));
        assert_eq!(overlay.predicted_cursor(), Some((2, 0)));
    }

    #[test]
    fn on_server_frame_acks_predictions() {
        let mut overlay = make_overlay();
        overlay.predict_char('a', (0, 0), 80);
        overlay.predict_char('b', (0, 0), 80);
        assert_eq!(overlay.cells().len(), 2);

        // Ack first prediction
        overlay.on_server_frame(1, (1, 0));
        assert_eq!(overlay.cells().len(), 1);
        assert_eq!(overlay.cells()[0].character, 'b');

        // Ack second prediction
        overlay.on_server_frame(2, (2, 0));
        assert!(overlay.cells().is_empty());
    }

    #[test]
    fn cursor_row_mismatch_discards_and_goes_tentative() {
        let mut overlay = make_overlay();
        overlay.predict_char('a', (0, 0), 80);
        assert_eq!(overlay.state(), PredictionState::Active);

        // Server cursor jumps to a different row
        overlay.on_server_frame(0, (0, 5));
        assert!(overlay.cells().is_empty());
        assert_eq!(overlay.state(), PredictionState::Tentative);
    }

    #[test]
    fn gc_expired_removes_old_predictions() {
        let mut overlay = make_overlay();
        overlay.prediction_timeout = Duration::from_millis(10);
        overlay.predict_char('a', (0, 0), 80);

        // Wait for expiry
        std::thread::sleep(Duration::from_millis(15));
        overlay.gc_expired();
        assert!(overlay.cells().is_empty());
    }

    #[test]
    fn gc_expired_keeps_fresh() {
        let mut overlay = make_overlay();
        overlay.prediction_timeout = Duration::from_secs(10);
        overlay.predict_char('a', (0, 0), 80);

        overlay.gc_expired();
        assert_eq!(overlay.cells().len(), 1);
    }

    #[test]
    fn should_predict_rejects_control_chars() {
        let overlay = make_overlay();
        assert!(!overlay.should_predict('\x03', true)); // Ctrl-C
        assert!(!overlay.should_predict('\x1b', true)); // Escape
        assert!(!overlay.should_predict('\n', true));
        assert!(!overlay.should_predict('\r', true));
    }

    #[test]
    fn should_predict_accepts_printable() {
        let overlay = make_overlay();
        assert!(overlay.should_predict('a', true));
        assert!(overlay.should_predict(' ', true));
        assert!(overlay.should_predict('Z', true));
        assert!(overlay.should_predict('1', true));
    }

    #[test]
    fn should_predict_rejects_non_remote() {
        let overlay = make_overlay();
        assert!(!overlay.should_predict('a', false));
    }

    #[test]
    fn should_predict_rejects_tentative_state() {
        let mut overlay = make_overlay();
        overlay.predict_char('a', (0, 0), 80);
        // Force tentative
        overlay.on_server_frame(0, (0, 5));
        assert!(!overlay.should_predict('b', true));
    }

    #[test]
    fn tentative_clears_after_stability() {
        let mut overlay = InputOverlay::new();
        overlay.state = PredictionState::Tentative;
        overlay.tentative_since = Some(Instant::now() - Duration::from_millis(600));

        // Pump stable frames
        for _ in 0..MIN_STABLE_FRAMES + 1 {
            overlay.on_server_frame(0, (0, 0));
        }
        assert_eq!(overlay.state(), PredictionState::Active);
    }

    #[test]
    fn wide_char_prediction_advances_by_two() {
        let mut overlay = make_overlay();
        // CJK character
        let seq = overlay.predict_char('\u{4E00}', (0, 0), 80);
        assert!(seq.is_some());
        assert_eq!(overlay.predicted_cursor(), Some((2, 0)));
        assert_eq!(overlay.cells()[0].width, 2);
    }

    #[test]
    fn prediction_at_end_of_line_stops() {
        let mut overlay = make_overlay();
        // Position cursor near end (col 79 in 80-col terminal)
        let seq = overlay.predict_char('a', (79, 0), 80);
        assert!(seq.is_some());
        // Cursor should advance past the end (col 80, which is >= cols)
        assert_eq!(overlay.predicted_cursor(), Some((80, 0)));

        // Next char should fail because cursor is at cols (80 >= 80)
        let seq = overlay.predict_char('b', (79, 0), 80);
        assert!(seq.is_none());
    }

    #[test]
    fn discard_all_increments_epoch() {
        let mut overlay = make_overlay();
        let e0 = overlay.epoch();
        overlay.discard_all();
        assert_eq!(overlay.epoch(), e0 + 1);
    }

    #[test]
    fn should_predict_requires_stable_frames() {
        let mut overlay = InputOverlay::new();
        // No frames yet — stability count is 0
        assert!(!overlay.should_predict('a', true));

        // Only 2 stable frames (need 3)
        overlay.on_server_frame(0, (0, 0));
        overlay.on_server_frame(0, (0, 0));
        assert!(!overlay.should_predict('a', true));

        // Third stable frame
        overlay.on_server_frame(0, (0, 0));
        assert!(overlay.should_predict('a', true));
    }
}
