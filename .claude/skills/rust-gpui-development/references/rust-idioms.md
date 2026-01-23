# Rust Idioms

Idiomatic Rust patterns for writing clean, safe, and performant code.

---

## Error Handling

### Use `?` Operator

```rust
// ❌ Verbose
fn load_config(path: &Path) -> Result<Config, Error> {
    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => return Err(e.into()),
    };
    let config = match toml::from_str(&content) {
        Ok(c) => c,
        Err(e) => return Err(e.into()),
    };
    Ok(config)
}

// ✅ Idiomatic
fn load_config(path: &Path) -> Result<Config, Error> {
    let content = fs::read_to_string(path)?;
    let config = toml::from_str(&content)?;
    Ok(config)
}
```

### Define Typed Errors

```rust
// ❌ Stringly-typed
fn parse(s: &str) -> Result<Value, String> {
    Err("invalid format".to_string())
}

// ✅ Typed errors with thiserror
#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("invalid format at position {position}: {message}")]
    InvalidFormat { position: usize, message: String },
    
    #[error("unexpected end of input")]
    UnexpectedEof,
    
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}
```

### Add Context to Errors

```rust
use anyhow::{Context, Result};

fn load_user_settings() -> Result<Settings> {
    let path = settings_path()?;
    let content = fs::read_to_string(&path)
        .with_context(|| format!("failed to read settings from {}", path.display()))?;
    
    toml::from_str(&content)
        .context("failed to parse settings file")
}
```

### Avoid `unwrap()` in Production

```rust
// ❌ Panics on None
let user = users.get(id).unwrap();

// ✅ Return error
let user = users.get(id).ok_or(Error::UserNotFound(id))?;

// ✅ Or handle with default/option
let user = users.get(id);
if let Some(user) = user {
    // use user
}
```

**Acceptable `unwrap()` locations:**
- Tests (`#[cfg(test)]`)
- Examples and documentation
- After explicit checks that guarantee `Some`/`Ok`
- `const` contexts

---

## Ownership & Borrowing

### Prefer Borrowing Over Cloning

```rust
// ❌ Unnecessary clone
fn process(items: &[Item]) {
    for item in items.iter().cloned() {  // clones each item
        handle(item);
    }
}

// ✅ Borrow instead
fn process(items: &[Item]) {
    for item in items {
        handle(item);  // handle takes &Item
    }
}
```

### Use `Cow` for Flexible Ownership

```rust
use std::borrow::Cow;

fn process_name(name: Cow<str>) -> String {
    if name.contains(' ') {
        name.replace(' ', "_")  // Only allocates if needed
    } else {
        name.into_owned()
    }
}

// Can call with borrowed or owned
process_name(Cow::Borrowed("hello"));
process_name(Cow::Owned(computed_string));
```

### Avoid `Rc<RefCell<T>>` Overuse

```rust
// ❌ Interior mutability everywhere
struct App {
    data: Rc<RefCell<Vec<Item>>>,
    processor: Rc<RefCell<Processor>>,
}

// ✅ Clear ownership
struct App {
    data: Vec<Item>,
    processor: Processor,
}

impl App {
    fn process(&mut self) {
        self.processor.process(&mut self.data);
    }
}
```

**When `Rc`/`Arc` IS appropriate:**
- True shared ownership with unclear lifetimes
- Cross-thread sharing (`Arc`)
- Graph structures with cycles (use `Weak` for back-references)

---

## Iterators

### Use Iterator Methods

```rust
// ❌ Manual loop with index
let mut results = Vec::new();
for i in 0..items.len() {
    if items[i].is_valid() {
        results.push(items[i].value * 2);
    }
}

// ✅ Iterator chain
let results: Vec<_> = items.iter()
    .filter(|item| item.is_valid())
    .map(|item| item.value * 2)
    .collect();
```

### Avoid Unnecessary `collect()`

```rust
// ❌ Collect then iterate again
let filtered: Vec<_> = items.iter()
    .filter(|x| x.active)
    .collect();
for item in filtered { ... }

// ✅ Chain without intermediate collection
for item in items.iter().filter(|x| x.active) {
    ...
}
```

### Use `Entry` API for Maps

```rust
// ❌ Double lookup
if !map.contains_key(&key) {
    map.insert(key.clone(), Vec::new());
}
map.get_mut(&key).unwrap().push(value);

// ✅ Single lookup with entry
map.entry(key).or_default().push(value);
```

### Know Your Iterator Adaptors

```rust
// Useful patterns
items.iter().enumerate()           // (index, &item)
items.iter().zip(&other)           // (&item, &other_item)
items.iter().filter_map(|x| x.ok())  // filter + map combined
items.iter().flat_map(|x| &x.children)  // flatten nested
items.iter().take(10)              // first 10
items.iter().skip(5)               // skip first 5
items.iter().peekable()            // peek without consuming
items.iter().chain(&more_items)    // concatenate iterators
```

---

## Type System

### Newtype Pattern

```rust
// ❌ Primitive obsession - easy to mix up parameters
fn transfer(from: u64, to: u64, amount: u64) { ... }

// ✅ Type-safe with newtypes
struct AccountId(u64);
struct Amount(u64);

fn transfer(from: AccountId, to: AccountId, amount: Amount) { ... }

// Compiler prevents: transfer(amount, from, to)
```

### Builder Pattern for Complex Construction

```rust
pub struct TerminalConfig {
    shell: PathBuf,
    working_dir: PathBuf,
    env: HashMap<String, String>,
    scrollback: usize,
}

impl TerminalConfig {
    pub fn builder() -> TerminalConfigBuilder {
        TerminalConfigBuilder::default()
    }
}

#[derive(Default)]
pub struct TerminalConfigBuilder {
    shell: Option<PathBuf>,
    working_dir: Option<PathBuf>,
    env: HashMap<String, String>,
    scrollback: Option<usize>,
}

impl TerminalConfigBuilder {
    pub fn shell(mut self, shell: impl Into<PathBuf>) -> Self {
        self.shell = Some(shell.into());
        self
    }
    
    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.insert(key.into(), value.into());
        self
    }
    
    pub fn build(self) -> Result<TerminalConfig, ConfigError> {
        Ok(TerminalConfig {
            shell: self.shell.ok_or(ConfigError::MissingShell)?,
            working_dir: self.working_dir.unwrap_or_else(default_working_dir),
            env: self.env,
            scrollback: self.scrollback.unwrap_or(10_000),
        })
    }
}

// Usage
let config = TerminalConfig::builder()
    .shell("/bin/zsh")
    .env("TERM", "xterm-256color")
    .build()?;
```

### Use Enums for States

```rust
// ❌ Boolean flags
struct Connection {
    is_connected: bool,
    is_authenticated: bool,
    error_message: Option<String>,
}

// ✅ Enum encodes valid states
enum ConnectionState {
    Disconnected,
    Connecting { attempt: u32 },
    Connected { session_id: String },
    Authenticated { session_id: String, user: User },
    Error { message: String },
}

struct Connection {
    state: ConnectionState,
}
```

---

## Structs & Traits

### Implement Common Traits

```rust
// Derive what makes sense for your type
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct UserId(u64);

#[derive(Debug, Clone, Default)]
pub struct Config {
    // ...
}

// Implement Display for user-facing output
impl std::fmt::Display for UserId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "user:{}", self.0)
    }
}
```

### Use `impl Trait` for Return Types

```rust
// ❌ Exposes implementation details
fn get_items(&self) -> std::slice::Iter<'_, Item> { ... }

// ✅ Hides implementation
fn get_items(&self) -> impl Iterator<Item = &Item> {
    self.items.iter()
}
```

### Trait Objects vs Generics

```rust
// Generics: zero-cost, monomorphized
fn process<T: Processor>(processor: T, data: &Data) {
    processor.process(data);
}

// Trait objects: dynamic dispatch, single code path
fn process(processor: &dyn Processor, data: &Data) {
    processor.process(data);
}

// Use generics for performance-critical paths
// Use trait objects for plugin systems, heterogeneous collections
```

---

## Code Organization

### Module Exports

```rust
// src/terminal/mod.rs
mod buffer;      // private implementation
mod parser;      // private implementation
mod session;     // private implementation

// Public API only
pub use session::{Terminal, TerminalConfig, TerminalEvent};

// Crate-internal use
pub(crate) use buffer::TerminalBuffer;
```

### Prelude Pattern

```rust
// src/prelude.rs - common imports for internal use
pub use crate::error::{Error, Result};
pub use crate::types::{TerminalId, SessionId};
pub use crate::config::Config;

// In other modules:
use crate::prelude::*;
```

### Keep Functions Small

```rust
// ❌ Long function doing many things
fn process_input(input: &str) -> Result<Output> {
    // 100 lines of parsing
    // 50 lines of validation
    // 80 lines of transformation
    // 30 lines of formatting
}

// ✅ Composed small functions
fn process_input(input: &str) -> Result<Output> {
    let parsed = parse(input)?;
    let validated = validate(parsed)?;
    let transformed = transform(validated)?;
    Ok(format_output(transformed))
}
```

---

## Common Pitfalls

### Indexing Without Bounds Check

```rust
// ❌ Can panic
let first = items[0];

// ✅ Safe alternatives
let first = items.first().ok_or(Error::Empty)?;
let first = items.get(0);

// ✅ Pattern matching
if let [first, rest @ ..] = items.as_slice() {
    // use first
}
```

### Forgetting `#[must_use]`

```rust
// Mark functions where ignoring return value is likely a bug
#[must_use]
pub fn validate(&self) -> Result<(), ValidationError> {
    // ...
}

// Compiler warns if result is ignored
```

### String Building

```rust
// ❌ Repeated allocation
let mut s = String::new();
for item in items {
    s = s + &item.to_string() + ", ";
}

// ✅ Pre-allocate and push
let mut s = String::with_capacity(items.len() * 10);
for item in items {
    write!(&mut s, "{}, ", item).unwrap();
}

// ✅ Or use join
let s = items.iter()
    .map(|i| i.to_string())
    .collect::<Vec<_>>()
    .join(", ");
```

---

## Quick Checks

```bash
# Find unwrap usage outside tests
grep -rn "\.unwrap()" src/ --include="*.rs" | grep -v "_test.rs" | grep -v "#\[test\]"

# Find clone hotspots
grep -rn "\.clone()" src/ --include="*.rs" | cut -d: -f1 | sort | uniq -c | sort -nr

# Run Clippy with pedantic lints
cargo clippy -- -W clippy::pedantic -W clippy::nursery

# Check for TODO/FIXME
grep -rn "TODO\|FIXME" src/ --include="*.rs"
```
