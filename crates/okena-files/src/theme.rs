//! Theme colors provider for okena-files.
//!
//! The main app must register a `GlobalThemeProvider` at startup so that
//! crate-internal views can read the current theme colors without depending
//! on the full `AppTheme` entity.

use okena_core::theme::ThemeColors;
use gpui::*;

/// Global theme provider -- a function pointer that reads the current theme colors.
/// The host app registers this at startup; okena-files views call `theme()` to read colors.
pub struct GlobalThemeProvider(pub fn(&App) -> ThemeColors);

impl Global for GlobalThemeProvider {}

/// Get current theme colors.
/// Panics if `GlobalThemeProvider` has not been set by the host app.
pub fn theme(cx: &App) -> ThemeColors {
    (cx.global::<GlobalThemeProvider>().0)(cx)
}
