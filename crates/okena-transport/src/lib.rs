//! Networking/transport layer over the wire schema defined in `okena-core`.
//!
//! Holds the heavy, optional-dependency code (tokio / reqwest / tungstenite /
//! rustls) that used to live in `okena-core` and forced the async stack onto
//! every crate in the workspace. The message schema (`okena_core::api`) and the
//! WS message types (`okena_core::ws`) stay in `okena-core`; this crate is the
//! client engine + blocking HTTP that talk over them.

#![cfg_attr(not(test), warn(clippy::unwrap_used, clippy::expect_used))]

mod connection_config;
#[cfg(any(feature = "client", feature = "blocking-http"))]
pub mod tls;

#[cfg(feature = "client")]
pub mod client;
#[cfg(feature = "blocking-http")]
pub mod http;
#[cfg(any(feature = "client", feature = "blocking-http"))]
pub mod remote_action;
#[cfg(any(feature = "client", feature = "blocking-http"))]
pub mod remote_http;

pub use connection_config::{LocalEndpoint, RemoteConnectionConfig, LOCAL_DAEMON_CONNECTION_ID};
