//! Blame loading lifecycle for `FileViewer`. Toggle, async load via provider,
//! invalidation on file changes.

use super::{BlameLoadState, FileViewer};
use gpui::{Context, WeakEntity};
use std::sync::Arc;

impl FileViewer {
    /// Toggle whether the blame gutter is shown. When turning on, kicks off
    /// an async load for the active tab if it has no blame data yet.
    pub fn toggle_blame(&mut self, cx: &mut Context<Self>) {
        self.blame_visible = !self.blame_visible;
        if self.blame_visible {
            self.spawn_blame_load_for_active(cx);
        }
        cx.notify();
    }

    /// Public read accessor for hosts persisting the setting.
    pub fn blame_visible(&self) -> bool {
        self.blame_visible
    }

    /// Override blame visibility without triggering a reload; used by the
    /// host when applying persisted settings on construction.
    pub fn set_blame_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.blame_visible == visible {
            return;
        }
        self.blame_visible = visible;
        if visible {
            self.spawn_blame_load_for_active(cx);
        }
        cx.notify();
    }

    /// If the active tab has no blame yet and a provider is configured, spawn
    /// a background task to fetch it. Idempotent — re-entering while
    /// [`BlameLoadState::Loading`] is a no-op.
    pub(super) fn spawn_blame_load_for_active(&mut self, cx: &mut Context<Self>) {
        let Some(provider) = self.blame_provider.clone() else {
            return;
        };
        let Some(tab) = self.tabs.get_mut(self.active_tab) else {
            return;
        };
        if tab.relative_path.is_empty() {
            return;
        }
        if matches!(tab.blame, BlameLoadState::Loading | BlameLoadState::Loaded(_)) {
            return;
        }
        tab.blame = BlameLoadState::Loading;
        let rel = tab.relative_path.clone();
        cx.spawn(async move |entity: WeakEntity<Self>, cx| {
            let result = cx
                .background_executor()
                .spawn({
                    let rel = rel.clone();
                    async move { provider.get_blame(&rel) }
                })
                .await;
            let _ = entity.update(cx, |this, cx| {
                if let Some(tab) = this.tabs.iter_mut().find(|t| t.relative_path == rel) {
                    tab.blame = match result {
                        Ok(lines) => BlameLoadState::Loaded(Arc::new(lines)),
                        Err(e) => BlameLoadState::Error(e),
                    };
                    cx.notify();
                }
            });
        })
        .detach();
    }
}
