//! GPUI-free daemon core for Okena.
//!
//! The desktop app drives the workspace/service logic crates through GPUI's
//! `Context`/`AsyncApp` reactor; this crate provides the second, headless
//! implementer backed by a plain tokio reactor and `Arc<parking_lot::Mutex>`
//! shared state. It exists so a headless daemon can run the exact same
//! `okena-workspace` / `okena-services` code paths with no GPUI in scope.
//!
//! This is the scaffold step: it stands up the shared [`reactor::DaemonReactor`]
//! state and the tokio-backed implementations of the two reactor trait families
//!
//! - [`okena_workspace::context::WorkspaceCx`] (see [`workspace_cx`])
//! - [`okena_services::manager`]'s `ServiceCx` / `ServiceHandle` /
//!   `ServiceAsyncCx` (see [`service_cx`])
//!
//! The observer tasks, PTY event loop, git polling, and command loop are *not*
//! wired here — they are later steps. This step only proves the trait impls
//! compile and link zero gpui.

pub mod reactor;
pub mod service_cx;
pub mod workspace_cx;
