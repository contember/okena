# Project Architecture

Patterns for organizing Rust GPUI projects for maintainability and testability.

---

## Recommended Structure

### Single Crate (Small-Medium Projects)

```
my-app/
├── Cargo.toml
├── src/
│   ├── main.rs           # Entry point only
│   ├── app.rs            # App initialization
│   │
│   ├── core/             # Domain logic (NO gpui dependency)
│   │   ├── mod.rs
│   │   ├── terminal.rs
│   │   ├── buffer.rs
│   │   └── parser.rs
│   │
│   ├── ui/               # GPUI views
│   │   ├── mod.rs
│   │   ├── terminal_view.rs
│   │   ├── tab_bar.rs
│   │   └── components/
│   │       ├── mod.rs
│   │       └── button.rs
│   │
│   ├── state/            # Application state
│   │   ├── mod.rs
│   │   └── app_state.rs
│   │
│   ├── actions/          # GPUI actions
│   │   ├── mod.rs
│   │   └── terminal.rs
│   │
│   └── settings/         # Configuration
│       ├── mod.rs
│       └── schema.rs
│
└── tests/                # Integration tests
```

### Workspace (Large Projects)

```
my-app/
├── Cargo.toml            # Workspace definition
│
├── crates/
│   ├── app/              # Binary crate
│   │   ├── Cargo.toml
│   │   └── src/main.rs
│   │
│   ├── core/             # Domain logic (no UI deps)
│   │   ├── Cargo.toml
│   │   └── src/lib.rs
│   │
│   ├── ui/               # GPUI components
│   │   ├── Cargo.toml
│   │   └── src/lib.rs
│   │
│   └── settings/         # Configuration
│       ├── Cargo.toml
│       └── src/lib.rs
│
└── tests/
```

---

## Dependency Direction

### The Rule

Dependencies flow inward - outer layers depend on inner:

```
┌────────────────────────────────────────────┐
│              UI Layer                       │
│    (views, components, rendering)           │
│              depends on ↓                   │
├────────────────────────────────────────────┤
│           Application Layer                 │
│    (state, actions, coordination)           │
│              depends on ↓                   │
├────────────────────────────────────────────┤
│             Domain Layer                    │
│    (core logic, NO framework deps)          │
│              depends on ↓                   │
├────────────────────────────────────────────┤
│          Infrastructure Layer               │
│    (platform, IO, external services)        │
└────────────────────────────────────────────┘
```

### Implementation

```rust
// ✅ core/ has NO gpui dependency
// crates/core/src/terminal.rs
pub struct Terminal {
    buffer: Buffer,
    cursor: Cursor,
}

impl Terminal {
    pub fn write(&mut self, data: &[u8]) -> Vec<TerminalEvent> {
        // Pure logic, returns events
    }
}

// ✅ ui/ depends on core
// crates/ui/src/terminal_view.rs
use core::Terminal;
use gpui::*;

pub struct TerminalView {
    terminal: Entity<Terminal>,
}
```

---

## Module Exports

### Minimal Public API

```rust
// src/core/mod.rs
mod buffer;      // Private
mod parser;      // Private
mod terminal;    // Private

// Public API - only what external code needs
pub use terminal::{Terminal, TerminalConfig, TerminalEvent};

// Crate-internal
pub(crate) use buffer::Buffer;
```

### Re-exports for Convenience

```rust
// src/lib.rs
pub mod core;
pub mod ui;
pub mod state;

// Prelude for common imports
pub mod prelude {
    pub use crate::core::{Terminal, TerminalConfig};
    pub use crate::state::AppState;
    pub use crate::error::{Error, Result};
}

// Usage in other modules
use crate::prelude::*;
```

---

## Testability

### Trait-Based Dependencies

```rust
// Define trait for external dependency
pub trait PtyBackend: Send + Sync {
    fn spawn(&self, cmd: &str) -> Result<PtyHandle>;
    fn read(&self, handle: &PtyHandle) -> Result<Vec<u8>>;
    fn write(&self, handle: &PtyHandle, data: &[u8]) -> Result<()>;
}

// Production implementation
pub struct NativePty;

impl PtyBackend for NativePty {
    fn spawn(&self, cmd: &str) -> Result<PtyHandle> {
        // Real implementation
    }
    // ...
}

// Test implementation
#[cfg(test)]
pub struct MockPty {
    pub output: Vec<u8>,
    pub written: std::sync::Mutex<Vec<u8>>,
}

#[cfg(test)]
impl PtyBackend for MockPty {
    fn spawn(&self, _cmd: &str) -> Result<PtyHandle> {
        Ok(PtyHandle::mock())
    }
    
    fn read(&self, _: &PtyHandle) -> Result<Vec<u8>> {
        Ok(self.output.clone())
    }
    
    fn write(&self, _: &PtyHandle, data: &[u8]) -> Result<()> {
        self.written.lock().unwrap().extend_from_slice(data);
        Ok(())
    }
}

// Terminal uses trait
pub struct Terminal<P: PtyBackend> {
    pty: P,
    buffer: Buffer,
}
```

### Test Layers

```
Integration tests (tests/)
    - Full app scenarios
    - GPUI TestAppContext
    
Component tests (ui/*/tests)
    - Individual view behavior
    - Mock dependencies

Unit tests (core/*/tests)
    - Pure logic
    - No framework dependencies
```

---

## Platform Abstraction

```rust
// src/platform/mod.rs
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "windows")]
mod windows;

pub trait Platform: Send + Sync {
    fn default_shell(&self) -> PathBuf;
    fn home_directory(&self) -> PathBuf;
    fn open_url(&self, url: &str) -> Result<()>;
}

pub fn current() -> Box<dyn Platform> {
    #[cfg(target_os = "macos")]
    return Box::new(macos::MacOS);
    
    #[cfg(target_os = "linux")]
    return Box::new(linux::Linux);
    
    #[cfg(target_os = "windows")]
    return Box::new(windows::Windows);
}
```

---

## Configuration

### Settings Schema

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub terminal: TerminalSettings,
    pub appearance: AppearanceSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TerminalSettings {
    pub shell: Option<PathBuf>,
    pub scrollback_lines: usize,
    pub font_family: String,
    pub font_size: f32,
}

impl Default for TerminalSettings {
    fn default() -> Self {
        Self {
            shell: None,  // Platform default
            scrollback_lines: 10_000,
            font_family: "JetBrains Mono".into(),
            font_size: 14.0,
        }
    }
}
```

### Loading/Saving

```rust
impl Settings {
    pub fn load() -> Result<Self> {
        let path = Self::path()?;
        if path.exists() {
            let content = fs::read_to_string(&path)?;
            Ok(toml::from_str(&content)?)
        } else {
            Ok(Self::default())
        }
    }
    
    pub fn save(&self) -> Result<()> {
        let path = Self::path()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, toml::to_string_pretty(self)?)?;
        Ok(())
    }
    
    fn path() -> Result<PathBuf> {
        dirs::config_dir()
            .map(|p| p.join("my-app/settings.toml"))
            .ok_or(Error::NoConfigDir)
    }
}
```

---

## Checklist

- [ ] Is `main.rs` minimal (just initialization)?
- [ ] Is domain logic in `core/` without UI dependencies?
- [ ] Do dependencies flow inward?
- [ ] Is public API minimal (`pub` only where needed)?
- [ ] Are external dependencies behind traits (for testing)?
- [ ] Is platform-specific code isolated?
- [ ] Can each layer be tested independently?
