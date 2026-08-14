use crate::requests::{OverlayRequest, SidebarRequest};
use gpui::*;
use okena_core::send_payload::SendPayload;
use std::collections::VecDeque;

/// Dedicated entity for transient UI request routing.
///
/// Decouples overlay/sidebar request queues from Workspace so that
/// observers only fire when actual requests are enqueued, not on every
/// workspace state change.
pub struct RequestBroker {
    overlay_requests: VecDeque<OverlayRequest>,
    sidebar_requests: VecDeque<SidebarRequest>,
    send_to_terminal: VecDeque<(SendPayload, Option<String>)>,
}

impl Default for RequestBroker {
    fn default() -> Self {
        Self::new()
    }
}

impl RequestBroker {
    pub fn new() -> Self {
        Self {
            overlay_requests: VecDeque::new(),
            sidebar_requests: VecDeque::new(),
            send_to_terminal: VecDeque::new(),
        }
    }

    pub fn push_overlay_request(&mut self, request: OverlayRequest, cx: &mut Context<Self>) {
        self.overlay_requests.push_back(request);
        cx.notify();
    }

    pub fn push_sidebar_request(&mut self, request: SidebarRequest, cx: &mut Context<Self>) {
        self.sidebar_requests.push_back(request);
        cx.notify();
    }

    /// Queue a "Send to Terminal" payload for the focused terminal. The host
    /// drains this on observation, resolves that terminal's CWD, and formats +
    /// pastes the result.
    pub fn push_send_to_terminal(&mut self, payload: SendPayload, cx: &mut Context<Self>) {
        self.send_to_terminal.push_back((payload, None));
        cx.notify();
    }

    /// Queue a payload for one specific terminal, regardless of focus.
    ///
    /// Annotating a terminal's own output sends it back to that same terminal,
    /// and by then the composer overlay holds focus — so the target can't be
    /// inferred the way `push_send_to_terminal` does it.
    pub fn push_send_to_terminal_targeted(
        &mut self,
        payload: SendPayload,
        terminal_id: String,
        cx: &mut Context<Self>,
    ) {
        self.send_to_terminal
            .push_back((payload, Some(terminal_id)));
        cx.notify();
    }

    pub fn drain_overlay_requests(&mut self) -> Vec<OverlayRequest> {
        self.overlay_requests.drain(..).collect()
    }

    pub fn drain_sidebar_requests(&mut self) -> Vec<SidebarRequest> {
        self.sidebar_requests.drain(..).collect()
    }

    pub fn drain_send_to_terminal(&mut self) -> Vec<(SendPayload, Option<String>)> {
        self.send_to_terminal.drain(..).collect()
    }

    pub fn has_overlay_requests(&self) -> bool {
        !self.overlay_requests.is_empty()
    }

    pub fn has_sidebar_requests(&self) -> bool {
        !self.sidebar_requests.is_empty()
    }

    pub fn has_send_to_terminal(&self) -> bool {
        !self.send_to_terminal.is_empty()
    }
}
