//! Generic overlay management utilities.
//!
//! Provides traits and helpers for modal overlay components with consistent
//! toggle and close behavior. Framework for managing overlay lifecycle.

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

    /// Close the overlay.
    pub fn close(&mut self) {
        self.entity = None;
    }

    /// Set the entity directly.
    pub fn set(&mut self, entity: Entity<T>) {
        self.entity = Some(entity);
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
}

/// Helper macro for toggling modal overlays via the single active_modal slot.
///
/// Usage:
/// ```ignore
/// toggle_overlay!(self, cx, KeybindingsHelp, KeybindingsHelpEvent, |cx| KeybindingsHelp::new(cx));
/// ```
#[macro_export]
macro_rules! toggle_overlay {
    ($self:ident, $cx:ident, $type:ty, $event_type:ty, $factory:expr) => {
        if $self.is_modal::<$type>() {
            $self.close_modal($cx);
        } else {
            let entity = $cx.new($factory);
            $cx.subscribe(&entity, |this, _, event: &$event_type, cx| {
                if event.is_close() {
                    this.close_modal(cx);
                }
            })
            .detach();
            $self.open_modal(entity, $cx);
        }
        $cx.notify();
    };
}
