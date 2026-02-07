---
name: rust-gpui-development
description: Development skill for Rust GUI applications using GPUI framework (Zed's UI framework). Use when working on GPUI-based desktop applications - creating views, managing state, handling actions, or reviewing/refactoring existing code. Triggers on Rust GUI development, GPUI views, Entity/Context usage, terminal emulators, code editors, or any desktop app built with GPUI.
---

# Rust GPUI Development

Patterns and API reference for building Rust applications with the GPUI framework.

## Reference Files

Load based on current task:

| Task | Reference | Content |
|------|-----------|---------|
| Working with GPUI | [gpui-patterns.md](references/gpui-patterns.md) | App architecture, context types, entities, actions, rendering, modals, drag & drop, menus, tooltips, testing |
| Designing state flow | [state-management.md](references/state-management.md) | Global state, view-model separation, state machines, derived state, unidirectional flow |

## Quick Reference

### GPUI Context Types

| Context | Scope | When Used |
|---------|-------|-----------|
| `&mut App` | Global | App init, global operations |
| `&mut Window` | Window | Rendering, events |
| `&mut Context<T>` | Entity | Entity methods, `render()` |

### View Structure

```rust
pub struct MyView {
    focus_handle: FocusHandle,
}

impl Render for MyView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .key_context("MyView")
            .on_action(cx.listener(Self::handle_action))
            .child(/* ... */)
    }
}

impl MyView {
    fn handle_action(&mut self, _: &MyAction, window: &mut Window, cx: &mut Context<Self>) {
        cx.notify();
    }
}
```

### Actions

```rust
// Simple actions
actions!(my_namespace, [DoSomething, Cancel]);

// Parameterized actions
#[derive(Clone, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = my_namespace)]
pub struct SelectItem { pub index: usize }

// Register in render() via cx.listener()
div().on_action(cx.listener(Self::handle_action))
```

### Global State

```rust
impl Global for AppSettings {}
cx.set_global(AppSettings::new());
let settings = cx.global::<AppSettings>();
cx.observe_global::<AppSettings>(|this, cx| { cx.notify(); }).detach();
```

### Critical Rules

1. **Never store `cx`** - always pass through method parameters
2. **Call `.detach()`** on subscriptions and observations
3. **Call `cx.notify()`** after state changes that affect rendering
4. **Use `cx.listener()`** for action/click handlers in `render()` - wraps entity access
5. **`window: &mut Window`** is a separate parameter in all entity callbacks
