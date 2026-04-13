//! Re-exports the hook execution surface from `okena-hooks` so that existing
//! `crate::hooks::*` callers (and `okena_workspace::hooks::*` from outside)
//! keep working after the split.
pub use okena_hooks::hooks::*;
