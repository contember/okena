//! Sidebar state controller.
//!
//! Manages sidebar visibility and auto-hide behavior.

use crate::settings::{AppSettings, MAX_SIDEBAR_WIDTH, MIN_SIDEBAR_WIDTH};

/// Controller for sidebar state and behavior.
///
/// Encapsulates:
/// - Open/closed state
/// - Auto-hide mode
/// - Hover state for auto-hide
/// - Configurable width
pub struct SidebarController {
    /// Whether the sidebar is logically open (user toggled)
    open: bool,
    /// Whether auto-hide mode is enabled
    auto_hide: bool,
    /// Whether sidebar is temporarily shown in auto-hide mode (mouse hover)
    hover_shown: bool,
    /// Configured sidebar width in pixels
    width: f32,
}

impl SidebarController {
    /// Create a new sidebar controller from app settings.
    pub fn new(settings: &AppSettings) -> Self {
        let open = settings.sidebar.is_open;
        let width = settings
            .sidebar
            .width
            .clamp(MIN_SIDEBAR_WIDTH, MAX_SIDEBAR_WIDTH);
        Self {
            open,
            auto_hide: settings.sidebar.auto_hide,
            hover_shown: false,
            width,
        }
    }

    /// Get the configured sidebar width.
    pub fn width(&self) -> f32 {
        self.width
    }

    /// Set the sidebar width (clamped to min/max bounds).
    ///
    /// Note: Caller is responsible for persisting via SettingsState.
    pub fn set_width(&mut self, width: f32) {
        self.width = width.clamp(MIN_SIDEBAR_WIDTH, MAX_SIDEBAR_WIDTH);
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

    /// Get current rendered width in pixels.
    pub fn current_width(&self) -> f32 {
        if self.should_render() {
            self.width
        } else {
            0.0
        }
    }

    /// Check if sidebar content should be rendered.
    pub fn should_render(&self) -> bool {
        self.open || (self.auto_hide && self.hover_shown)
    }

    /// Toggle sidebar visibility.
    pub fn toggle(&mut self) {
        self.open = !self.open;
        self.hover_shown = false;
    }

    /// Toggle auto-hide mode.
    ///
    /// If auto-hide is enabled and sidebar is open, it will close.
    pub fn toggle_auto_hide(&mut self) {
        self.auto_hide = !self.auto_hide;

        if self.auto_hide && self.open {
            // Close sidebar when enabling auto-hide
            self.open = false;
        }
    }

    /// Show sidebar on hover (in auto-hide mode).
    pub fn show_on_hover(&mut self) -> bool {
        if self.auto_hide && !self.open && !self.hover_shown {
            self.hover_shown = true;
            true
        } else {
            false
        }
    }

    /// Hide sidebar when mouse leaves (in auto-hide mode).
    pub fn hide_on_leave(&mut self) -> bool {
        if self.auto_hide && self.hover_shown {
            self.hover_shown = false;
            true
        } else {
            false
        }
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
        let settings = test_settings();
        let mut ctrl = SidebarController::new(&settings);

        assert!(!ctrl.is_open());

        ctrl.toggle();
        assert!(ctrl.is_open());
        assert!(ctrl.should_render());

        ctrl.toggle();
        assert!(!ctrl.is_open());
        assert!(!ctrl.should_render());
    }

    #[test]
    fn test_auto_hide() {
        let mut settings = test_settings();
        settings.sidebar.is_open = true;
        let mut ctrl = SidebarController::new(&settings);

        // Enable auto-hide while open should close
        ctrl.toggle_auto_hide();
        assert!(ctrl.is_auto_hide());
        assert!(!ctrl.is_open());
        assert!(!ctrl.should_render());
    }

    #[test]
    fn test_hover_show_hide() {
        let mut settings = test_settings();
        settings.sidebar.auto_hide = true;
        let mut ctrl = SidebarController::new(&settings);

        assert!(ctrl.show_on_hover());
        assert!(ctrl.is_hover_shown());
        assert!(ctrl.should_render());
        assert_eq!(ctrl.current_width(), ctrl.width());

        assert!(ctrl.hide_on_leave());
        assert!(!ctrl.is_hover_shown());
        assert!(!ctrl.should_render());
        assert_eq!(ctrl.current_width(), 0.0);
    }

    #[test]
    fn test_width_clamping() {
        let settings = test_settings();
        let mut ctrl = SidebarController::new(&settings);

        ctrl.set_width(10.0); // below MIN
        assert_eq!(ctrl.width(), MIN_SIDEBAR_WIDTH);

        ctrl.set_width(9999.0); // above MAX
        assert_eq!(ctrl.width(), MAX_SIDEBAR_WIDTH);

        ctrl.set_width(300.0); // within range
        assert_eq!(ctrl.width(), 300.0);
    }

    #[test]
    fn test_current_width_tracks_visibility() {
        let settings = test_settings();
        let mut ctrl = SidebarController::new(&settings);
        assert!(!ctrl.should_render());
        assert_eq!(ctrl.current_width(), 0.0);

        ctrl.toggle();
        assert!(ctrl.should_render());
        assert_eq!(ctrl.current_width(), ctrl.width());
    }
}
