//! App-global hover state shared by the Switch Project overlay.
//!
//! When the user hovers a project row in the project switcher, that project's
//! id is published here. Every window observes this entity and ring-highlights
//! the matching project panel if the project is visible in that window — so a
//! hover also reveals where the project lives across multiple windows.
//!
//! It lives as an [`Entity`] behind a [`GlobalProjectHover`] (rather than a
//! plain `Global` value) so each `WindowView` can `cx.observe` it and re-render
//! when the hovered project changes, regardless of which window's switcher
//! produced the change.

use gpui::*;

/// The currently hovered project id (or `None` when nothing is hovered).
pub struct ProjectHoverState {
    hovered: Option<String>,
}

impl ProjectHoverState {
    pub fn new() -> Self {
        Self { hovered: None }
    }

    pub fn hovered(&self) -> Option<&str> {
        self.hovered.as_deref()
    }

    /// Replace the hovered id, notifying observers only on an actual change so
    /// repeated mouse-enter events on the same row don't churn every window.
    fn set(&mut self, id: Option<String>, cx: &mut Context<Self>) {
        if self.hovered.as_deref() != id.as_deref() {
            self.hovered = id;
            cx.notify();
        }
    }
}

impl Default for ProjectHoverState {
    fn default() -> Self {
        Self::new()
    }
}

/// Global handle to the shared [`ProjectHoverState`].
pub struct GlobalProjectHover(pub Entity<ProjectHoverState>);

impl Global for GlobalProjectHover {}

/// Read the hovered project id from the global, if it has been registered.
pub fn hovered_project(cx: &App) -> Option<String> {
    cx.try_global::<GlobalProjectHover>()
        .and_then(|g| g.0.read(cx).hovered().map(str::to_string))
}

/// Publish (or clear) the hovered project id on the shared global. A no-op when
/// the global has not been registered (e.g. headless contexts).
pub fn set_hovered_project(id: Option<String>, cx: &mut App) {
    let Some(entity) = cx.try_global::<GlobalProjectHover>().map(|g| g.0.clone()) else {
        return;
    };
    entity.update(cx, |state, cx| state.set(id, cx));
}
