//! Shared active-harness-view state.
//!
//! The sidebar renders the HARNESS nav and the window renders the view, but
//! they live in different crates and hold no handle to each other. Both can see
//! `okena-workspace`, so the selection lives here as an [`Entity`] behind a
//! global — the same shape as the project-hover state — so the sidebar can
//! `cx.observe` it and re-highlight when the window changes views.

use gpui::*;
use okena_core::harness::HarnessSection;
use okena_state::WindowId;
use std::collections::HashMap;

/// Which harness view is showing, per window.
///
/// Keyed by window because each window shows its own view: switching to Agents
/// in one window must not yank another window off its terminals.
pub struct HarnessState {
    active: HashMap<WindowId, HarnessSection>,
}

impl HarnessState {
    pub fn new() -> Self {
        Self {
            active: HashMap::new(),
        }
    }

    pub fn active(&self, window: WindowId) -> Option<HarnessSection> {
        self.active.get(&window).copied()
    }

    /// Whether setting `section` would actually change anything. Split out so
    /// the no-churn rule is testable without a GPUI context.
    pub fn is_change(&self, window: WindowId, section: Option<HarnessSection>) -> bool {
        self.active(window) != section
    }

    /// Replace the active view for `window`, notifying observers only on a real
    /// change so re-selecting the same entry doesn't churn every observer.
    pub fn set(
        &mut self,
        window: WindowId,
        section: Option<HarnessSection>,
        cx: &mut Context<Self>,
    ) {
        if !self.is_change(window, section) {
            return;
        }
        match section {
            Some(s) => {
                self.active.insert(window, s);
            }
            None => {
                self.active.remove(&window);
            }
        }
        cx.notify();
    }
}

impl Default for HarnessState {
    fn default() -> Self {
        Self::new()
    }
}

/// Global handle to the shared [`HarnessState`].
pub struct GlobalHarnessState(pub Entity<HarnessState>);

impl Global for GlobalHarnessState {}

/// The active harness view for `window`, or `None` when the terminal workspace
/// is showing.
pub fn active_harness(window: WindowId, cx: &App) -> Option<HarnessSection> {
    cx.try_global::<GlobalHarnessState>()
        .and_then(|g| g.0.read(cx).active(window))
}

/// Handle to the shared state, when it has been registered.
pub fn harness_state_entity(cx: &App) -> Option<Entity<HarnessState>> {
    cx.try_global::<GlobalHarnessState>().map(|g| g.0.clone())
}

/// Publish the active harness view. A no-op when the global has not been
/// registered (headless contexts).
pub fn set_active_harness(window: WindowId, section: Option<HarnessSection>, cx: &mut App) {
    let Some(entity) = harness_state_entity(cx) else {
        return;
    };
    entity.update(cx, |state, cx| state.set(window, section, cx));
}

#[cfg(test)]
mod tests {
    // Deliberately NOT `use super::*`: that re-exports the `gpui::*` glob from
    // this module, which brings gpui's own `test` attribute macro into scope.
    // A bare `#[test]` would then resolve to `gpui::test`, which expands to
    // `#[test]` again — infinite macro recursion, reported as "recursion limit
    // reached". Import only what the tests need.
    use super::HarnessState;
    use okena_core::harness::HarnessSection;
    use okena_state::WindowId;

    fn with(section: HarnessSection) -> HarnessState {
        let mut state = HarnessState::new();
        state.active.insert(WindowId::Main, section);
        state
    }

    #[test]
    fn reselecting_the_same_view_is_not_a_change() {
        // Clicking the active nav entry again must be a no-op, not a toggle —
        // this is the rule that keeps the view from disappearing under the user.
        let state = with(HarnessSection::Tasks);
        assert!(!state.is_change(WindowId::Main, Some(HarnessSection::Tasks)));
    }

    #[test]
    fn switching_views_is_a_change() {
        let state = with(HarnessSection::Tasks);
        assert!(state.is_change(WindowId::Main, Some(HarnessSection::Agents)));
    }

    #[test]
    fn clearing_from_active_is_a_change() {
        let state = with(HarnessSection::Agents);
        assert!(state.is_change(WindowId::Main, None));
        assert!(!HarnessState::new().is_change(WindowId::Main, None));
    }

    #[test]
    fn windows_are_independent() {
        // A view selected in one window must not appear active in another.
        let state = with(HarnessSection::Tasks);
        let other = WindowId::Extra(uuid::Uuid::nil());
        assert_eq!(state.active(WindowId::Main), Some(HarnessSection::Tasks));
        assert_eq!(state.active(other), None);
    }
}
