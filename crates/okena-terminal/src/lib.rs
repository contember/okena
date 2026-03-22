pub mod backend;
pub mod input;
pub mod process;
pub mod pty_manager;
pub mod session_backend;
pub mod shell_config;
pub mod terminal;

use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;

/// Shared terminals registry for PTY event routing.
/// Maps terminal ID → Terminal instance.
pub type TerminalsRegistry = Arc<Mutex<HashMap<String, Arc<terminal::Terminal>>>>;
