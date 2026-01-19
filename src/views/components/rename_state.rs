//! Reusable rename state management.
//!
//! Provides a generic helper for inline rename functionality with SimpleInput.

use super::simple_input::SimpleInputState;
use gpui::*;

/// State for an active rename operation.
///
/// Generic over the ID type that identifies what is being renamed.
pub struct RenameState<Id> {
    /// The ID of the item being renamed
    pub target: Id,
    /// The input entity for editing the name
    pub input: Entity<SimpleInputState>,
}

impl<Id> RenameState<Id> {
    /// Create a new rename state.
    pub fn new(target: Id, input: Entity<SimpleInputState>) -> Self {
        Self { target, input }
    }

    /// Get the current input value.
    pub fn value(&self, cx: &App) -> String {
        self.input.read(cx).value().to_string()
    }
}

/// Start a rename operation.
///
/// Creates a new `RenameState` with a configured `SimpleInputState`.
/// The input will be focused and a blur handler will be set up to call `on_blur`.
///
/// # Arguments
/// * `target` - The ID of the item being renamed
/// * `current_name` - The current name to pre-fill
/// * `placeholder` - Placeholder text for the input
/// * `window` - Window reference for focus management
/// * `cx` - Context for the parent view
///
/// # Example
/// ```rust
/// self.rename_state = Some(start_rename(
///     terminal_id.clone(),
///     &current_name,
///     "Terminal name...",
///     window,
///     cx,
/// ));
/// ```
pub fn start_rename<Id: Clone + 'static, V: 'static>(
    target: Id,
    current_name: &str,
    placeholder: &str,
    window: &mut Window,
    cx: &mut Context<V>,
) -> RenameState<Id> {
    let input = cx.new(|cx| {
        SimpleInputState::new(cx)
            .placeholder(placeholder)
            .default_value(current_name)
    });

    // Focus the input
    let focus_handle = input.read(cx).focus_handle(cx);
    window.focus(&focus_handle, cx);

    RenameState::new(target, input)
}

/// Start a rename operation with a blur handler.
///
/// Same as `start_rename` but also sets up a blur handler that will be called
/// when the input loses focus.
///
/// # Arguments
/// * `target` - The ID of the item being renamed
/// * `current_name` - The current name to pre-fill
/// * `placeholder` - Placeholder text for the input
/// * `on_blur` - Callback to invoke when input loses focus
/// * `window` - Window reference for focus management
/// * `cx` - Context for the parent view
pub fn start_rename_with_blur<Id, V, F>(
    target: Id,
    current_name: &str,
    placeholder: &str,
    on_blur: F,
    window: &mut Window,
    cx: &mut Context<V>,
) -> RenameState<Id>
where
    Id: Clone + 'static,
    V: 'static,
    F: Fn(&mut V, &mut Window, &mut Context<V>) + 'static,
{
    let input = cx.new(|cx| {
        SimpleInputState::new(cx)
            .placeholder(placeholder)
            .default_value(current_name)
    });

    // Set up blur handler
    let focus_handle = input.read(cx).focus_handle(cx);
    let _ = cx.on_blur(&focus_handle, window, on_blur);

    // Focus will be set by on_blur registration
    window.focus(&focus_handle, cx);

    RenameState::new(target, input)
}

/// Finish a rename operation and get the result.
///
/// Returns `Some((target, new_name))` if the rename was active and the name is not empty.
/// Returns `None` if the state was `None` or the input was empty.
///
/// This function clears the rename state.
///
/// # Example
/// ```rust
/// if let Some((terminal_id, new_name)) = finish_rename(&mut self.rename_state, cx) {
///     workspace.rename_terminal(&terminal_id, new_name, cx);
/// }
/// ```
pub fn finish_rename<Id>(
    state: &mut Option<RenameState<Id>>,
    cx: &App,
) -> Option<(Id, String)> {
    let rename_state = state.take()?;
    let new_name = rename_state.value(cx);

    if new_name.is_empty() {
        None
    } else {
        Some((rename_state.target, new_name))
    }
}

/// Cancel a rename operation without applying changes.
///
/// Simply clears the rename state.
pub fn cancel_rename<Id>(state: &mut Option<RenameState<Id>>) {
    *state = None;
}

/// Check if a rename is active for a specific target.
pub fn is_renaming<Id: PartialEq>(state: &Option<RenameState<Id>>, target: &Id) -> bool {
    state.as_ref().map_or(false, |s| &s.target == target)
}

/// Get the input entity from an active rename state.
pub fn rename_input<Id>(state: &Option<RenameState<Id>>) -> Option<&Entity<SimpleInputState>> {
    state.as_ref().map(|s| &s.input)
}
