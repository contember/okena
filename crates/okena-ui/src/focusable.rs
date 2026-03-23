/// Implement the `Focusable` trait for a type that has a `focus_handle` field.
///
/// This macro generates a standard `Focusable` implementation that returns
/// a clone of the `focus_handle` field.
///
/// # Example
///
/// ```rust,ignore
/// pub struct MyView {
///     focus_handle: FocusHandle,
/// }
///
/// okena_ui::impl_focusable!(MyView);
/// ```
#[macro_export]
macro_rules! impl_focusable {
    ($type:ty) => {
        impl gpui::Focusable for $type {
            fn focus_handle(&self, _cx: &gpui::App) -> gpui::FocusHandle {
                self.focus_handle.clone()
            }
        }
    };
}
