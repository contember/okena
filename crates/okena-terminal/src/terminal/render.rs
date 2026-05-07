use super::Terminal;
use super::ansi_snapshot::grid_to_ansi;

impl Terminal {
    /// Render the terminal's visible content as ANSI escape sequences.
    ///
    /// Produces a byte stream that, when fed to another terminal emulator,
    /// reproduces the current screen state including colors and attributes.
    pub fn render_snapshot(&self) -> Vec<u8> {
        self.with_content(|term| grid_to_ansi(term))
    }
}
