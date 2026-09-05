#![cfg_attr(not(test), warn(clippy::unwrap_used, clippy::expect_used))]

//! okena-tasks — task-manager integration for the engineering harness.
//!
//! GPUI-free by design: the daemon owns task state (ADR-0001), so this crate
//! must be usable from `okena-daemon`. Nothing here may depend on gpui.
//!
//! Layering mirrors `okena-core` / `okena-transport`: the provider-neutral
//! types live in [`okena_core::tasks`] (so `okena-state` can persist a
//! `TaskRef` without inheriting this crate's networking dependencies), while
//! this crate owns the client side — auth and the per-platform providers.
//!
//! - [`provider`] — the `TaskProvider` trait every platform implements.
//! - [`providers`] — concrete implementations (Linear today).
//! - [`store`] — per-profile credential persistence.

pub mod provider;
pub mod providers;
pub mod store;

pub use okena_core::tasks::{Task, TaskId, TaskRef, TaskState};
pub use provider::{AuthStatus, Credential, TaskError, TaskProvider};
pub use providers::LinearProvider;

/// Build the provider for `provider_id` using whatever credential is stored for
/// it, or `None` when the id is unknown.
///
/// A provider is constructed per call rather than cached: the credential can
/// change under us (connect / disconnect / refresh) and these are cheap structs
/// wrapping a token, so a stale cached instance would be the only real hazard.
pub fn provider_for(provider_id: &str) -> Option<Box<dyn TaskProvider>> {
    match provider_id {
        "linear" => Some(Box::new(LinearProvider::new(store::load("linear")))),
        _ => None,
    }
}

/// Provider ids this build knows about, in display order.
pub const KNOWN_PROVIDERS: &[&str] = &["linear"];
