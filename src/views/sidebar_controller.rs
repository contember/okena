//! Sidebar state and animation controller.
//!
//! Manages sidebar visibility, auto-hide behavior, and animation state.
//! The actual animation spawning is handled by the parent view.

use crate::workspace::persistence::{save_settings, AppSettings};

/// Default sidebar width in pixels.
pub const SIDEBAR_WIDTH: f32 = 250.0;

/// Animation duration in milliseconds.
pub const ANIMATION_DURATION_MS: u64 = 150;

/// Frame time for ~60fps animation.
pub const FRAME_TIME_MS: u64 = 16;

/// Result of a sidebar state change that may require animation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AnimationTarget {
    /// No animation needed
    None,
    /// Animate to fully open (1.0)
    Open,
    /// Animate to fully closed (0.0)
    Close,
}

impl AnimationTarget {
    /// Get the target value for animation.
    pub fn value(self) -> Option<f32> {
        match self {
            AnimationTarget::None => None,
            AnimationTarget::Open => Some(1.0),
            AnimationTarget::Close => Some(0.0),
        }
    }
}

/// Controller for sidebar state and behavior.
///
/// Encapsulates:
/// - Open/closed state
/// - Animation progress (0.0 = closed, 1.0 = open)
/// - Auto-hide mode
/// - Hover state for auto-hide
///
/// The parent view is responsible for:
/// - Spawning animations when `AnimationTarget` is returned
/// - Rendering the sidebar based on `current_width()` and `should_render()`
pub struct SidebarController {
    /// Whether the sidebar is logically open (user toggled)
    open: bool,
    /// Animation progress (0.0 = collapsed, 1.0 = fully open)
    animation: f32,
    /// Whether auto-hide mode is enabled
    auto_hide: bool,
    /// Whether sidebar is temporarily shown in auto-hide mode (mouse hover)
    hover_shown: bool,
}

impl SidebarController {
    /// Create a new sidebar controller from app settings.
    pub fn new(settings: &AppSettings) -> Self {
        let open = settings.sidebar.is_open;
        Self {
            open,
            animation: if open { 1.0 } else { 0.0 },
            auto_hide: settings.sidebar.auto_hide,
            hover_shown: false,
        }
    }

    /// Check if sidebar is logically open.
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Check if auto-hide mode is enabled.
    pub fn is_auto_hide(&self) -> bool {
        self.auto_hide
    }

    /// Check if sidebar is temporarily shown via hover.
    pub fn is_hover_shown(&self) -> bool {
        self.hover_shown
    }

    /// Get current animation progress (0.0 to 1.0).
    pub fn animation(&self) -> f32 {
        self.animation
    }

    /// Set animation progress (called during animation updates).
    pub fn set_animation(&mut self, value: f32) {
        self.animation = value.clamp(0.0, 1.0);
    }

    /// Get current rendered width in pixels.
    pub fn current_width(&self) -> f32 {
        self.animation * SIDEBAR_WIDTH
    }

    /// Check if sidebar content should be rendered (animation > threshold).
    pub fn should_render(&self) -> bool {
        self.animation > 0.01
    }

    /// Toggle sidebar visibility.
    ///
    /// Returns the animation target and updates settings.
    pub fn toggle(&mut self, settings: &mut AppSettings) -> AnimationTarget {
        self.open = !self.open;
        self.hover_shown = false;

        // Persist state
        settings.sidebar.is_open = self.open;
        let _ = save_settings(settings);

        if self.open {
            AnimationTarget::Open
        } else {
            AnimationTarget::Close
        }
    }

    /// Toggle auto-hide mode.
    ///
    /// If auto-hide is enabled and sidebar is open, it will close.
    /// Returns the animation target and updates settings.
    pub fn toggle_auto_hide(&mut self, settings: &mut AppSettings) -> AnimationTarget {
        self.auto_hide = !self.auto_hide;

        let target = if self.auto_hide && self.open {
            // Close sidebar when enabling auto-hide
            self.open = false;
            AnimationTarget::Close
        } else {
            AnimationTarget::None
        };

        // Persist state
        settings.sidebar.auto_hide = self.auto_hide;
        settings.sidebar.is_open = self.open;
        let _ = save_settings(settings);

        target
    }

    /// Show sidebar on hover (in auto-hide mode).
    ///
    /// Returns animation target if sidebar should animate open.
    pub fn show_on_hover(&mut self) -> AnimationTarget {
        if self.auto_hide && !self.open && !self.hover_shown {
            self.hover_shown = true;
            AnimationTarget::Open
        } else {
            AnimationTarget::None
        }
    }

    /// Hide sidebar when mouse leaves (in auto-hide mode).
    ///
    /// Returns animation target if sidebar should animate closed.
    pub fn hide_on_leave(&mut self) -> AnimationTarget {
        if self.auto_hide && self.hover_shown {
            self.hover_shown = false;
            AnimationTarget::Close
        } else {
            AnimationTarget::None
        }
    }

    /// Calculate eased animation progress.
    ///
    /// Uses ease-out cubic for smooth deceleration.
    pub fn ease_progress(current: f32, target: f32, step: usize, total_steps: usize) -> f32 {
        let t = step as f32 / total_steps as f32;
        let eased = 1.0 - (1.0 - t).powi(3); // ease-out cubic
        current + (target - current) * eased
    }

    /// Get animation step count based on duration and frame time.
    pub fn animation_steps() -> usize {
        (ANIMATION_DURATION_MS / FRAME_TIME_MS) as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_settings() -> AppSettings {
        AppSettings::default()
    }

    #[test]
    fn test_toggle() {
        let mut settings = test_settings();
        let mut ctrl = SidebarController::new(&settings);

        assert!(!ctrl.is_open());

        let target = ctrl.toggle(&mut settings);
        assert!(ctrl.is_open());
        assert_eq!(target, AnimationTarget::Open);

        let target = ctrl.toggle(&mut settings);
        assert!(!ctrl.is_open());
        assert_eq!(target, AnimationTarget::Close);
    }

    #[test]
    fn test_auto_hide() {
        let mut settings = test_settings();
        settings.sidebar.is_open = true;
        let mut ctrl = SidebarController::new(&settings);
        ctrl.animation = 1.0;

        // Enable auto-hide while open should close
        let target = ctrl.toggle_auto_hide(&mut settings);
        assert!(ctrl.is_auto_hide());
        assert!(!ctrl.is_open());
        assert_eq!(target, AnimationTarget::Close);
    }

    #[test]
    fn test_hover_show_hide() {
        let mut settings = test_settings();
        settings.sidebar.auto_hide = true;
        let mut ctrl = SidebarController::new(&settings);

        let target = ctrl.show_on_hover();
        assert!(ctrl.is_hover_shown());
        assert_eq!(target, AnimationTarget::Open);

        let target = ctrl.hide_on_leave();
        assert!(!ctrl.is_hover_shown());
        assert_eq!(target, AnimationTarget::Close);
    }
}
