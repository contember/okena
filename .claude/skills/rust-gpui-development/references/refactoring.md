# Refactoring Guide

Systematic approach to analyzing and improving existing GPUI codebases.

---

## Analysis Workflow

### Phase 1: Discovery

```bash
# Project overview
find . -name "*.rs" | wc -l
wc -l src/**/*.rs | sort -n | tail -20
cat Cargo.toml

# GPUI usage
grep -rn "impl Render" src/
grep -rn "Entity<" src/ | wc -l
grep -rn "cx\.new(" src/
```

### Phase 2: Issue Detection

```bash
# Critical: potential panics
grep -rn "\.unwrap()" src/ | grep -v test | wc -l
grep -rn "\.expect(" src/ | grep -v test
grep -rn "panic!" src/ | grep -v test
grep -rn "\[.*\]" src/ | grep -v "impl\|struct\|enum"  # unchecked indexing

# Code quality
grep -rn "\.clone()" src/ | wc -l
grep -rn "Rc<RefCell" src/
grep -rn "unsafe" src/

# GPUI patterns
grep -rn "observe\|subscribe" src/ | grep -v "detach()"  # missing detach
grep -rn "cx\.notify()" src/ | wc -l

# Clippy
cargo clippy -- -D warnings 2>&1 | head -50
```

### Phase 3: Prioritization

```
P0 (Critical) - Fix immediately
    - Panics in user-facing code
    - Memory safety issues
    - Data loss risks

P1 (High) - Fix soon
    - Architectural problems
    - Testability blockers
    - Performance issues

P2 (Medium) - Improve when touched
    - Code quality
    - Missing error handling
    - Documentation

P3 (Low) - Nice to have
    - Style improvements
    - Minor optimizations
```

---

## Common Refactorings

### Extract Business Logic from View

**Before:**
```rust
struct TerminalView {
    buffer: Vec<Line>,
    cursor: Position,
    // View handles all logic
}

impl TerminalView {
    fn process_input(&mut self, data: &[u8], cx: &mut Context<Self>) {
        // Complex parsing logic mixed with view
        self.parse_ansi(data);
        self.update_cursor();
        cx.notify();
    }
}
```

**After:**
```rust
// Model: pure logic
struct Terminal {
    buffer: Vec<Line>,
    cursor: Position,
}

impl Terminal {
    fn process_input(&mut self, data: &[u8], cx: &mut Context<Self>) {
        self.parse_ansi(data);
        self.update_cursor();
        cx.notify();
    }
}

// View: UI only
struct TerminalView {
    terminal: Entity<Terminal>,
}

impl TerminalView {
    fn new(terminal: Entity<Terminal>, cx: &mut Context<Self>) -> Self {
        cx.observe(&terminal, |_, _, cx| cx.notify()).detach();
        Self { terminal }
    }
}
```

### Extract Module

**Before:**
```rust
// src/terminal.rs - 2000 lines
pub struct Terminal { ... }
pub struct TerminalConfig { ... }
pub struct TerminalBuffer { ... }
fn parse_ansi(...) { ... }
fn render_cell(...) { ... }
```

**After:**
```
src/terminal/
├── mod.rs          # pub use exports
├── terminal.rs     # Terminal struct
├── config.rs       # TerminalConfig
├── buffer.rs       # TerminalBuffer
├── parser.rs       # ANSI parsing
└── renderer.rs     # Rendering helpers
```

```rust
// src/terminal/mod.rs
mod buffer;
mod config;
mod parser;
mod renderer;
mod terminal;

pub use config::TerminalConfig;
pub use terminal::{Terminal, TerminalEvent};
```

### Replace Clone with Borrowing

**Before:**
```rust
fn process_items(items: &[Item]) {
    for item in items {
        let owned = item.clone();  // Unnecessary
        validate(owned);
    }
}
```

**After:**
```rust
fn process_items(items: &[Item]) {
    for item in items {
        validate(item);  // Pass reference
    }
}
```

### Convert Panic to Result

**Before:**
```rust
fn get_item(&self, id: usize) -> &Item {
    &self.items[id]  // Panics if out of bounds
}

fn parse_config(s: &str) -> Config {
    toml::from_str(s).unwrap()  // Panics on invalid input
}
```

**After:**
```rust
fn get_item(&self, id: usize) -> Option<&Item> {
    self.items.get(id)
}

fn parse_config(s: &str) -> Result<Config, ConfigError> {
    toml::from_str(s).map_err(ConfigError::Parse)
}
```

### Decompose Large View

**Before:**
```rust
struct EditorView {
    // 25 fields
    buffer: Buffer,
    cursor: Cursor,
    selection: Option<Selection>,
    scroll_offset: f32,
    gutter_width: f32,
    // ... 20 more
}

impl Render for EditorView {
    fn render(&mut self, ...) {
        // 400 lines
    }
}
```

**After:**
```rust
struct EditorView {
    buffer: Entity<Buffer>,
    gutter: Entity<GutterView>,
    content: Entity<ContentView>,
    scrollbar: Entity<ScrollbarView>,
}

impl Render for EditorView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .size_full()
            .child(self.gutter.clone())
            .child(self.content.clone())
            .child(self.scrollbar.clone())
    }
}
```

---

## Safe Refactoring Process

1. **Verify current behavior**
   ```bash
   cargo test
   cargo clippy
   ```

2. **Make one focused change**
   - Extract one module, OR
   - Fix one pattern, OR
   - Add one test

3. **Verify again**
   ```bash
   cargo test
   cargo clippy
   ```

4. **Commit**
   ```bash
   git commit -m "refactor: extract buffer module from terminal"
   ```

5. **Repeat**

---

## Analysis Report Template

```markdown
# Refactoring Analysis: [Project Name]

## Summary
[2-3 sentences on overall health]

## Metrics
| Metric | Value | Status |
|--------|-------|--------|
| Files | X | |
| Lines | X | |
| unwrap() outside tests | X | ⚠️ if >10 |
| clone() calls | X | ⚠️ if >50 |
| Tests | X | ⚠️ if 0 |
| Clippy warnings | X | ⚠️ if >0 |

## P0 Issues
1. [Issue] at [location] - [risk] - [fix]

## P1 Issues  
1. [Issue] - [impact] - [recommendation]

## Suggested Order
1. [First change] - [why first]
2. [Second change] - [depends on first]

## Target Structure
```
src/
├── ...
```
```

---

## Checklist Before/After

### Before Starting
- [ ] Tests pass
- [ ] Clippy clean
- [ ] Git clean (committed)

### After Each Change
- [ ] `cargo build` succeeds
- [ ] `cargo test` passes
- [ ] `cargo clippy` clean
- [ ] Behavior unchanged (or intentionally improved)
- [ ] Committed with clear message

### After Refactoring Complete
- [ ] No new `unwrap()` in production code
- [ ] Dependencies flow correctly
- [ ] Public API minimal
- [ ] Key logic has tests
- [ ] Documentation updated
