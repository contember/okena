//! Okena's UI/app layer: the GPUI views, the app coordinator, keybindings,
//! action dispatch, and the thin re-export shim modules over the lower-level
//! crates. Extracted out of the `okena` binary so the binary stays a thin
//! entry point and this layer compiles as its own crate.

// The remote-control server lives in its own crate (`okena-remote-server`).
// Re-exported as `crate::remote` so the moved code's `crate::remote::...`
// references keep working unchanged.
pub use okena_remote_server as remote;

// The headless app-logic layer (global settings + workspace action glue) lives
// in `okena-app-core`. Re-exported as `crate::settings` / `crate::workspace` so
// the moved code's `crate::settings::...` / `crate::workspace::...` references
// keep working unchanged.
pub use okena_app_core::{settings, workspace};

// `macros.rs` declares `#[macro_export] macro_rules! impl_focusable`, which must
// stay exported at this crate's root so `impl_focusable!` resolves in the moved
// code (and as `okena_app::impl_focusable!` from the binary).
#[macro_use]
mod macros;

pub mod action_dispatch;
pub mod app;
pub mod elements;
pub mod git;
pub mod keybindings;
pub mod logging;
pub mod remote_client;
pub mod services;
#[cfg(target_os = "linux")]
pub mod simple_root;
pub mod soft_close;
pub mod terminal;
pub mod theme;
pub mod ui;
pub mod views;
