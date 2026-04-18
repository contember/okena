#![cfg_attr(not(test), warn(clippy::unwrap_used, clippy::expect_used))]

pub mod config;
pub mod docker_compose;
pub mod error;
pub mod manager;
pub mod port_detect;

pub use error::{ServiceError, ServiceResult};
