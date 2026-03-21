// Re-export everything from the okena-git crate.
// This allows existing `use crate::git::*` imports to keep working.
pub use okena_git::*;
pub use okena_git::branch_names;
pub use okena_git::repository;

// Watcher stays in main app (depends on GPUI + Workspace)
pub mod watcher;

// Theme-dependent color methods as extension traits (kept in main app)
use crate::theme::ThemeColors;

pub trait PrStateColor {
    fn color(&self, t: &ThemeColors) -> u32;
}

impl PrStateColor for PrState {
    fn color(&self, t: &ThemeColors) -> u32 {
        match self {
            PrState::Open => t.term_green,
            PrState::Draft => t.text_muted,
            PrState::Merged => t.term_magenta,
            PrState::Closed => t.term_red,
        }
    }
}

pub trait CiStatusColor {
    fn color(&self, t: &ThemeColors) -> u32;
}

impl CiStatusColor for CiStatus {
    fn color(&self, t: &ThemeColors) -> u32 {
        match self {
            CiStatus::Success => t.term_green,
            CiStatus::Failure => t.term_red,
            CiStatus::Pending => t.term_yellow,
        }
    }
}
