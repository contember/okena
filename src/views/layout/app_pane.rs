//! Type-erased wrapper for app pane entities.
//!
//! Any app type can construct an `AppPaneEntity` via `AppPaneEntity::new()`.
//! No match arms needed — methods return stored values directly.

use gpui::*;

/// Type-erased wrapper around a concrete app entity.
///
/// Stores a boxed closure that produces the element, plus cached metadata.
/// This avoids an enum that must be extended for every new app type.
pub struct AppPaneEntity {
    element_fn: Box<dyn Fn() -> AnyElement>,
    focus_handle: FocusHandle,
    app_id: Option<String>,
    display_name: &'static str,
    icon_path: &'static str,
}

impl AppPaneEntity {
    /// Create a new type-erased app pane from any `Render` entity.
    pub fn new<T: Render + 'static>(
        entity: Entity<T>,
        app_id: Option<String>,
        display_name: &'static str,
        icon_path: &'static str,
        focus_handle: FocusHandle,
    ) -> Self {
        let entity_clone = entity.clone();
        Self {
            element_fn: Box::new(move || entity_clone.clone().into_any_element()),
            focus_handle,
            app_id,
            display_name,
            icon_path,
        }
    }

    pub fn into_any_element(&self) -> AnyElement {
        (self.element_fn)()
    }

    #[allow(dead_code)]
    pub fn display_name(&self) -> &str {
        self.display_name
    }

    #[allow(dead_code)]
    pub fn icon_path(&self) -> &str {
        self.icon_path
    }

    pub fn app_id(&self) -> Option<&str> {
        self.app_id.as_deref()
    }

    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus_handle
    }
}
