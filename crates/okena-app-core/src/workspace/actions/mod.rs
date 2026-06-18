// Re-export action submodules from the workspace crate.
// These mostly contain `impl Workspace` blocks; `soft_close` also exports the
// `PendingDecision` type. Re-exporting the modules makes them accessible as
// `crate::workspace::actions::*`.
#[allow(unused_imports)]
pub use okena_workspace::actions::{focus, folder, layout, project, soft_close, terminal};

pub mod execute;
