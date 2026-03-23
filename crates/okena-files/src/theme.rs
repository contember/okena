//! Theme colors provider for okena-files.
//!
//! Re-exports `GlobalThemeProvider` and `theme()` from okena-ui so that
//! existing imports (`okena_files::theme::theme`) keep working.

pub use okena_ui::theme::{GlobalThemeProvider, theme};
