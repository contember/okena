// Re-export action submodules from the workspace crate.
// These contain `impl Workspace` blocks (no standalone types to re-export),
// but re-exporting the modules makes them accessible as `crate::workspace::actions::*`.
#[allow(unused_imports)]
pub use okena_workspace::actions::{focus, folder, layout, project, terminal};

mod app;
pub mod execute;
