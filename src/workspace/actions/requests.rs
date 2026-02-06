//! UI request workspace actions
//!
//! Actions for managing UI dialogs and menus (context menu, shell selector, etc.)

use crate::workspace::state::{OverlayRequest, SidebarRequest, Workspace};
use gpui::*;

impl Workspace {
    /// Push an overlay request onto the queue and notify.
    pub fn push_overlay_request(&mut self, request: OverlayRequest, cx: &mut Context<Self>) {
        self.overlay_requests.push_back(request);
        cx.notify();
    }

    /// Push a sidebar request onto the queue and notify.
    pub fn push_sidebar_request(&mut self, request: SidebarRequest, cx: &mut Context<Self>) {
        self.sidebar_requests.push_back(request);
        cx.notify();
    }
}
