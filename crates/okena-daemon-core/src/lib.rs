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
//! This crate also provides the self-contained, gpui-free async tasks the
//! daemon runs on its reactor:
//!
//! - the observer tasks (see [`observers`]),
//! - the PTY event loop ([`pty_loop::run_pty_loop`]),
//! - the git-status poller ([`git_poll::run_git_poll`]),
//! - the remote command loop ([`command_loop::daemon_command_loop`]), and
//! - the gpui-free settings/theme handlers ([`daemon_config`]).
//!
//! Each takes its dependencies as parameters; the (later) `DaemonCore::new`
//! wires them onto the tokio `LocalSet` / multi-thread runtime.

pub mod command_loop;
pub mod daemon_config;
pub mod git_poll;
pub mod observers;
pub mod pty_loop;
pub mod reactor;
pub mod service_cx;
pub mod workspace_cx;
