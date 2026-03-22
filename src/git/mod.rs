// Re-export everything from the okena-git crate.
// This allows existing `use crate::git::*` imports to keep working.
pub use okena_git::*;
pub use okena_git::branch_names;
pub use okena_git::repository;

// Watcher stays in main app (depends on GPUI + Workspace)
pub mod watcher;

// Re-export color extension traits from the views-git crate so existing
// `use crate::git::{PrStateColor, CiStatusColor}` imports keep working.
pub use okena_views_git::project_header::{PrStateColor, CiStatusColor};
