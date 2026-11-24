use super::Terminal;
use super::ansi_snapshot::grid_to_ansi;
use super::scrollback::HistoryView;

impl Terminal {
    /// Render the terminal's visible content as ANSI escape sequences.
    ///
    /// Produces a byte stream that, when fed to another terminal emulator,
    /// reproduces the current screen state including colors and attributes.
    pub fn render_snapshot(&self) -> Vec<u8> {
        self.render_snapshot_with_sequence().0
    }

    /// Render a snapshot with the last PTY event incorporated into that grid.
    pub fn render_snapshot_with_sequence(&self) -> (Vec<u8>, u64) {
        let mut slow = okena_core::timing::SlowGuard::new("Terminal::render_snapshot");
        self.drain_pending_output();
        let mut term = self.term.lock();
        let mut scrollback = self.scrollback.lock();
        scrollback.settle(&mut term, true);
        let mut bytes = grid_to_ansi(&HistoryView::new(&term, &scrollback));
        drop(scrollback);
        // A newly attached/reconnected client may never see another title OSC
        // from an idle process. Capture it under the same grid lock/watermark.
        // Empty OSC 2 also clears a stale client title when the source has none.
        bytes.extend_from_slice(b"\x1b]2;");
        if let Some(title) = self.title() {
            bytes.extend_from_slice(title.as_bytes());
        }
        bytes.extend_from_slice(b"\x1b\\");
        let sequence = self
            .processed_output_sequence
            .load(std::sync::atomic::Ordering::Acquire);
        slow.set_detail(format!("{} bytes", bytes.len()));
        (bytes, sequence)
    }
}
