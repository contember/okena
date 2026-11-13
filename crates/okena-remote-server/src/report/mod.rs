//! Report-back: a child terminal's reports (its command exited, its agent's
//! turn ended or needs the user, or it reported itself) delivered into the
//! input of the terminal that started it, or to that terminal's `okena wait`.
//! Served under `/v1/report/...`; see `docs/reference/report-back.md`.

mod hub;
mod message;
mod state;

pub(crate) use hub::ReportError;
pub use hub::{EXIT_REPORT_MARK, ReportHub};
