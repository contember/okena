#![cfg_attr(not(test), warn(clippy::unwrap_used, clippy::expect_used))]

pub mod api;
pub mod client;
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod frb_generated;
