use alacritty_terminal::term::test::TermSize;
use std::sync::atomic::Ordering;

use super::Terminal;
use super::resize_authority::{
    claim_resize_authority_local, claim_resize_authority_remote, is_resize_authority_local,
};
use super::types::TerminalSize;

impl Terminal {
    /// Resize the terminal with debounced transport resize.
    ///
    /// The terminal grid is always resized immediately (optimistic update) for
    /// smooth rendering. Transport resize signals (PTY/remote) are debounced to
    /// avoid flooding the shell or network.
    ///
    /// Debounce interval is transport-dependent: 16ms for local PTY (~60fps),
    /// 150ms for remote connections. A trailing-edge timer ensures the final
    /// resize is always sent even when resize events stop mid-debounce.
    pub fn resize(&self, new_size: TerminalSize) {
        let debounce_ms = self.transport.resize_debounce_ms();

        // Always update local size immediately (optimistic UI)
        {
            let mut rs = self.resize_state.lock();
            rs.size = new_size;
            rs.last_local_resize = std::time::Instant::now();
        }

        // Resize terminal grid immediately (independent mutex)
        let mut term = self.term.lock();
        let term_size = TermSize::new(new_size.cols as usize, new_size.rows as usize);
        term.resize(term_size);
        drop(term);

        self.content_generation.fetch_add(1, Ordering::Relaxed);

        // Debounce transport resize
        let now = std::time::Instant::now();
        let mut rs = self.resize_state.lock();
        let elapsed = now.duration_since(rs.last_pty_resize);

        if elapsed.as_millis() >= debounce_ms as u128 {
            // Enough time has passed — send resize immediately
            rs.pending_pty_resize = None;
            rs.last_pty_resize = now;
            drop(rs);
            self.transport.resize(&self.terminal_id, new_size.cols, new_size.rows);
        } else {
            // Store pending resize
            rs.pending_pty_resize = Some((new_size.cols, new_size.rows));

            // Schedule a trailing-edge flush timer if not already active.
            // This ensures the final resize is always sent even when events stop.
            if !rs.flush_timer_active {
                rs.flush_timer_active = true;
                let transport = self.transport.clone();
                let terminal_id = self.terminal_id.clone();
                let resize_state = self.resize_state.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(debounce_ms));
                    let mut rs = resize_state.lock();
                    rs.flush_timer_active = false;
                    if let Some((cols, rows)) = rs.pending_pty_resize.take() {
                        rs.last_pty_resize = std::time::Instant::now();
                        drop(rs);
                        transport.resize(&terminal_id, cols, rows);
                    }
                });
            }
        }
    }

    /// Resize only the local alacritty grid, without sending resize to PTY/transport.
    /// Used by remote clients to pre-resize the grid to match server dimensions before snapshot.
    ///
    /// Skips the resize if the client recently performed a local resize (within 200ms)
    /// to avoid redundant grid reflows from server echo during active resize drag.
    pub fn resize_grid_only(&self, cols: u16, rows: u16) {
        let rs = self.resize_state.lock();
        // Skip if we recently resized locally — the server is echoing back our own resize
        if rs.last_local_resize.elapsed().as_millis() < 200 {
            // Still accept if the size actually differs (e.g. server-initiated resize)
            if rs.size.cols == cols && rs.size.rows == rows {
                return;
            }
        }
        let size = TerminalSize {
            cols,
            rows,
            cell_width: rs.size.cell_width,
            cell_height: rs.size.cell_height,
        };
        drop(rs);
        self.resize_state.lock().size = size;
        let mut term = self.term.lock();
        let term_size = TermSize::new(cols as usize, rows as usize);
        term.resize(term_size);
        self.content_generation.fetch_add(1, Ordering::Relaxed);
    }

    /// Mark the local side (origin) as resize authority. Process-global:
    /// any local keyboard/mouse input in any terminal claims authority for all
    /// terminals.
    pub fn claim_resize_local(&self) {
        claim_resize_authority_local();
    }

    /// Mark the remote side as resize authority. Called on the server when a
    /// remote client sends input to any terminal.
    pub fn claim_resize_remote(&self) {
        claim_resize_authority_remote();
    }

    /// Returns true if the local (origin) side currently has resize authority.
    /// The server's UI uses this to decide whether to push resize events to
    /// the PTY.
    pub fn is_resize_owner_local(&self) -> bool {
        is_resize_authority_local()
    }

    /// Flush any pending PTY resize (call this when resize operations complete)
    pub fn flush_pending_resize(&self) {
        let mut rs = self.resize_state.lock();
        if let Some((cols, rows)) = rs.pending_pty_resize.take() {
            rs.last_pty_resize = std::time::Instant::now();
            drop(rs);
            self.transport.resize(&self.terminal_id, cols, rows);
        }
    }

    /// Get cell dimensions (width, height) for coordinate conversion
    pub fn cell_dimensions(&self) -> (f32, f32) {
        let rs = self.resize_state.lock();
        (rs.size.cell_width, rs.size.cell_height)
    }
}
