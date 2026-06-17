//! Headless app-logic layer: global observable settings and the
//! action-execution glue over the workspace. Decoupled from the UI views and
//! the app coordinator (which still live in the `okena` binary for now).

pub mod settings;
pub mod workspace;
