//! Review workspace UI — see `docs/review-workspace-ui-spec.md`.
//!
//! `state` / `model` / `ranking` / `labels` / `actions` are the shared surface;
//! the remaining modules render one screen region each.

pub(crate) mod state;

pub(crate) mod model;

pub(crate) mod ranking;

pub(crate) mod labels;

pub(crate) mod actions;

pub(crate) mod shell;

pub(crate) mod diff_state;

#[cfg(test)]
pub(crate) mod fixtures;

pub(crate) mod status;

pub(crate) mod navigator;

pub(crate) mod overview;

pub(crate) mod file_view;

pub(crate) mod keys;

pub(crate) use shell::DiffPaneArgs;
pub(crate) use state::ContentView;
