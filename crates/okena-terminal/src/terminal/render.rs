use super::Terminal;
use super::ansi_snapshot::grid_to_ansi;

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
        let term = self.term.lock();
        let bytes = grid_to_ansi(&term);
        let sequence = self
            .processed_output_sequence
            .load(std::sync::atomic::Ordering::Acquire);
        slow.set_detail(format!("{} bytes", bytes.len()));
        (bytes, sequence)
    }
}
