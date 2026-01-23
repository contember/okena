---
name: rust-gpui-development
description: Development skill for Rust GUI applications using GPUI framework (Zed's UI framework). Use when working on GPUI-based desktop applications - creating views, managing state, handling actions, writing idiomatic Rust code, structuring projects, or reviewing/refactoring existing code. Triggers on Rust GUI development, GPUI views, Entity/Context usage, terminal emulators, code editors, or any desktop app built with GPUI.
---

# Rust GPUI Development

Best practices for building and maintaining Rust applications with GPUI framework.

## When to Use This Skill

- **New development**: Creating views, entities, actions, state management
- **Code review**: Checking for anti-patterns and architectural issues  
- **Refactoring**: Improving existing codebase structure
- **Debugging**: Understanding GPUI behavior and common pitfalls
- **Architecture**: Designing module structure and data flow

## Reference Files

Load based on current task:

| Task | Reference | Content |
|------|-----------|---------|
| Writing any Rust code | [rust-idioms.md](references/rust-idioms.md) | Idiomatic patterns, error handling, ownership |
| Working with GPUI | [gpui-guide.md](references/gpui-guide.md) | Entity, View, Context, Actions, rendering |
| Designing state flow | [state-management.md](references/state-management.md) | Centralized state, events, state machines |
| Structuring project | [architecture.md](references/architecture.md) | Modules, crates, dependencies, testing |
| Reviewing/refactoring | [refactoring.md](references/refactoring.md) | Analysis workflow, checklists, recipes |

## Quick Reference

### GPUI Core Concepts

```
Application (App)
    └── Window
        └── View (Entity<T> where T: Render)
            └── Model (Entity<T>) - non-visual state
                └── Context (cx) - access to GPUI services
```

### Essential Patterns

**Creating entities:**
```rust
let model = cx.new(|_| MyModel::new());
let view = cx.new(|cx| MyView::new(model, cx));
```

**View structure:**
```rust
pub struct MyView {
    model: Entity<MyModel>,
}

impl Render for MyView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(/* ... */)
    }
}
```

**Observing changes:**
```rust
cx.observe(&model, |this, _model, cx| {
    cx.notify();  // Request re-render
}).detach();
```

**Emitting events:**
```rust
cx.emit(MyEvent::Updated(data));
```

**Handling actions:**
```rust
cx.on_action(|this, _: &MyAction, cx| {
    this.handle_action(cx);
});
```

### Critical Rules

1. **Never store `cx`** - always pass through method parameters
2. **Call `.detach()`** on subscriptions and observations
3. **Call `cx.notify()`** after state changes that affect rendering
4. **Avoid `unwrap()`** in production code - use `?` or proper error handling
5. **Minimize `clone()`** - prefer borrowing where possible

### Project Layout

```
src/
├── main.rs          # Entry point (minimal)
├── app.rs           # App initialization
├── core/            # Domain logic (no GPUI deps)
├── ui/              # GPUI views and components
├── state/           # Application state
├── actions/         # Action definitions
└── settings/        # Configuration
```

## Development Workflow

### Starting New Feature

1. Read [gpui-guide.md](references/gpui-guide.md) for GPUI patterns
2. Design state structure (what data, where it lives)
3. Create model entity for business logic
4. Create view entity for UI
5. Wire up observations and actions
6. Test with `#[gpui::test]`

### Reviewing Code

1. Run quick checks:
   ```bash
   cargo clippy -- -D warnings
   grep -rn "\.unwrap()" src/ | grep -v test
   ```
2. Read [refactoring.md](references/refactoring.md) for full checklist
3. Check architecture against [architecture.md](references/architecture.md)

### Common Tasks

| Task | Reference Section |
|------|-------------------|
| Add new view | gpui-guide.md → View Patterns |
| Handle keyboard shortcut | gpui-guide.md → Action System |
| Share state between views | state-management.md → Centralized State |
| Define custom error type | rust-idioms.md → Error Handling |
| Extract module | architecture.md → Module Organization |
| Improve performance | gpui-guide.md → Performance Patterns |

## Validation Commands

```bash
# Must pass before committing
cargo build
cargo clippy -- -D warnings
cargo test

# Optional deeper checks
cargo clippy -- -W clippy::pedantic
cargo fmt --check
```
