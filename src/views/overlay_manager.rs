//! Overlay management utilities.
//!
//! Provides traits and helpers for managing modal overlay components
//! with consistent toggle and close behavior.

use gpui::*;

/// Trait for overlay events that support closing.
///
/// Implement this for your overlay's event enum to enable
/// automatic close handling.
pub trait CloseEvent {
    /// Returns true if this event represents a close action.
    fn is_close(&self) -> bool;
}

/// A slot that manages a single overlay entity with toggle behavior.
///
/// Provides:
/// - Toggle semantics (open if closed, close if open)
/// - Clean entity lifecycle management
pub struct OverlaySlot<T: 'static> {
    entity: Option<Entity<T>>,
}

impl<T: 'static> Default for OverlaySlot<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: 'static> OverlaySlot<T> {
    /// Create a new empty overlay slot.
    pub const fn new() -> Self {
        Self { entity: None }
    }

    /// Check if the overlay is currently open.
    pub fn is_open(&self) -> bool {
        self.entity.is_some()
    }

    /// Get a reference to the entity if open.
    pub fn get(&self) -> Option<&Entity<T>> {
        self.entity.as_ref()
    }

    /// Close the overlay.
    pub fn close(&mut self) {
        self.entity = None;
    }

    /// Set the entity directly.
    pub fn set(&mut self, entity: Entity<T>) {
        self.entity = Some(entity);
    }

    /// Take the entity out of the slot.
    pub fn take(&mut self) -> Option<Entity<T>> {
        self.entity.take()
    }
}

impl<T: 'static + Render> OverlaySlot<T> {
    /// Render the overlay as an optional child element.
    ///
    /// Returns the entity clone if open, None otherwise.
    /// Use with `.when()` and `.child()` in your render method.
    pub fn render(&self) -> Option<Entity<T>> {
        self.entity.clone()
    }

    /// Render helper - returns child if open.
    ///
    /// Usage in render():
    /// ```ignore
    /// .when(self.keybindings_help.is_open(), |d| {
    ///     d.child(self.keybindings_help.render().unwrap())
    /// })
    /// ```
    pub fn child(&self) -> Option<impl IntoElement> {
        self.entity.clone()
    }
}

/// Helper macro for toggling simple overlays.
///
/// Usage:
/// ```ignore
/// toggle_overlay!(self, cx, keybindings_help, KeybindingsHelpEvent, || KeybindingsHelp::new(cx));
/// ```
#[macro_export]
macro_rules! toggle_overlay {
    ($self:ident, $cx:ident, $field:ident, $event_type:ty, $factory:expr) => {
        if $self.$field.is_open() {
            $self.$field.close();
        } else {
            let entity = $cx.new($factory);
            $cx.subscribe(&entity, |this, _, event: &$event_type, cx| {
                if event.is_close() {
                    this.$field.close();
                    cx.notify();
                }
            })
            .detach();
            $self.$field.set(entity);
        }
        $cx.notify();
    };
}

// Re-export the macro at crate level
pub use toggle_overlay;

// Implement CloseEvent for existing overlay events

use crate::views::keybindings_help::KeybindingsHelpEvent;
use crate::views::theme_selector::ThemeSelectorEvent;
use crate::views::command_palette::CommandPaletteEvent;
use crate::views::settings_panel::SettingsPanelEvent;

impl CloseEvent for KeybindingsHelpEvent {
    fn is_close(&self) -> bool {
        matches!(self, KeybindingsHelpEvent::Close)
    }
}

impl CloseEvent for ThemeSelectorEvent {
    fn is_close(&self) -> bool {
        matches!(self, ThemeSelectorEvent::Close)
    }
}

impl CloseEvent for CommandPaletteEvent {
    fn is_close(&self) -> bool {
        matches!(self, CommandPaletteEvent::Close)
    }
}

impl CloseEvent for SettingsPanelEvent {
    fn is_close(&self) -> bool {
        matches!(self, SettingsPanelEvent::Close)
    }
}
