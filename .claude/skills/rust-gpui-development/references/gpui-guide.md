# GPUI Guide

Complete guide to building applications with GPUI, Zed's GPU-accelerated UI framework.

---

## Core Architecture

```
┌─────────────────────────────────────────────────────────────┐
│ Application (App)                                           │
│   - Owns all entities                                       │
│   - Manages event loop                                      │
│   - Global services (quit, open_url, etc.)                 │
├─────────────────────────────────────────────────────────────┤
│ Window                                                      │
│   - Platform window wrapper                                 │
│   - Has one root View                                       │
│   - Handles input events                                    │
├─────────────────────────────────────────────────────────────┤
│ View (Entity<T> where T: Render)                           │
│   - Visual component                                        │
│   - Implements Render trait                                 │
│   - Rebuilt each frame                                      │
├─────────────────────────────────────────────────────────────┤
│ Model (Entity<T>)                                          │
│   - Non-visual state                                        │
│   - Business logic                                          │
│   - Can be shared between views                            │
├─────────────────────────────────────────────────────────────┤
│ Context (cx)                                               │
│   - Gateway to all GPUI services                           │
│   - Never store, always pass as parameter                  │
└─────────────────────────────────────────────────────────────┘
```

---

## Entities

Entities are GPUI's fundamental building blocks. All state lives in entities, owned by the `App`.

### Creating Entities

```rust
// Simple model
let counter = cx.new(|_| Counter { value: 0 });

// Model with initialization
let terminal = cx.new(|cx| {
    let config = TerminalConfig::load();
    Terminal::new(config, cx)
});

// View (also an entity)
let view = cx.new(|cx| {
    let model = cx.new(|_| MyModel::default());
    MyView::new(model, cx)
});
```

### Reading & Updating Entities

```rust
// Read entity state (immutable borrow)
let value = counter.read(cx).value;

// Update entity state (mutable borrow)
counter.update(cx, |counter, cx| {
    counter.value += 1;
    cx.notify();  // Important: trigger re-render
});
```

### Entity References

```rust
struct MyView {
    // Strong reference - keeps entity alive
    model: Entity<MyModel>,
    
    // Weak reference - doesn't prevent cleanup
    optional_ref: WeakEntity<OtherModel>,
}

impl MyView {
    fn use_optional(&self, cx: &App) {
        // Must check if still valid
        if let Some(other) = self.optional_ref.upgrade() {
            let data = other.read(cx);
            // ...
        }
    }
}
```

---

## Views

Views are entities that implement `Render` - they produce UI.

### Basic View Structure

```rust
pub struct MyView {
    // Entity references
    model: Entity<MyModel>,
    
    // View-specific state
    scroll_offset: f32,
    is_focused: bool,
    
    // Child views (optional)
    header: Entity<HeaderView>,
}

impl MyView {
    pub fn new(model: Entity<MyModel>, cx: &mut Context<Self>) -> Self {
        // Set up observations
        cx.observe(&model, |this, _model, cx| {
            cx.notify();  // Re-render when model changes
        }).detach();
        
        // Set up action handlers
        cx.on_action(|this, _: &Save, cx| {
            this.save(cx);
        });
        
        let header = cx.new(|_| HeaderView::new());
        
        Self {
            model,
            scroll_offset: 0.0,
            is_focused: false,
            header,
        }
    }
}

impl Render for MyView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let model = self.model.read(cx);
        
        div()
            .size_full()
            .bg(cx.theme().background)
            .child(self.header.clone())
            .child(self.render_content(model, cx))
    }
}
```

### View Composition

Break large views into smaller, focused components:

```rust
// ❌ Monolithic view
struct EditorView {
    // 30 fields...
}

impl Render for EditorView {
    fn render(...) {
        // 500 lines...
    }
}

// ✅ Composed views
struct EditorView {
    toolbar: Entity<Toolbar>,
    gutter: Entity<Gutter>,
    content: Entity<EditorContent>,
    minimap: Entity<Minimap>,
    status_bar: Entity<StatusBar>,
}

impl Render for EditorView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .child(self.toolbar.clone())
            .child(
                h_flex()
                    .flex_1()
                    .child(self.gutter.clone())
                    .child(self.content.clone())
                    .child(self.minimap.clone())
            )
            .child(self.status_bar.clone())
    }
}
```

### Conditional Rendering

```rust
impl Render for MyView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            // Conditional child
            .when(self.show_sidebar, |div| {
                div.child(self.sidebar.clone())
            })
            // Conditional with else
            .map(|div| {
                if self.is_loading {
                    div.child(Spinner::new())
                } else {
                    div.child(self.content.clone())
                }
            })
    }
}
```

---

## Context Types

Different contexts provide different capabilities:

### `App` - Global Context

```rust
fn init_app(cx: &mut App) {
    // Application-level operations
    cx.open_window(options, |cx| RootView::new(cx));
    cx.quit();
    cx.open_url("https://example.com");
    
    // Global action handlers
    cx.on_action(|_: &Quit, cx| cx.quit());
}
```

### `Context<T>` - Entity Context

```rust
impl MyView {
    fn some_method(&mut self, cx: &mut Context<Self>) {
        // Entity operations
        cx.notify();                           // Request re-render
        cx.emit(MyEvent::Changed);             // Emit event
        cx.observe(&other, |this, _, cx| {});  // Observe entity
        cx.subscribe(&other, |this, _, e, cx| {});  // Subscribe to events
        cx.on_action(|this, _: &Action, cx| {});    // Handle action
        
        // Spawn async work
        cx.spawn(async move |this, mut cx| {
            let result = fetch_data().await;
            this.update(&mut cx, |this, cx| {
                this.data = result;
                cx.notify();
            })
        }).detach();
    }
}
```

### `Window` - Window Context

```rust
impl Render for MyView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let bounds = window.bounds();
        let scale = window.scale_factor();
        // ...
    }
}
```

### Critical Rules

```rust
// ❌ NEVER store context
struct BadView {
    cx: Context<Self>,  // Will not compile, but conceptually wrong
}

// ✅ Always pass through parameters
impl MyView {
    fn helper(&mut self, cx: &mut Context<Self>) {
        // Use cx here
    }
}
```

---

## Events & Observations

### Observing Entities

Trigger callback when entity is updated:

```rust
impl MyView {
    fn new(model: Entity<Model>, cx: &mut Context<Self>) -> Self {
        // Observe any mutation
        cx.observe(&model, |this, _model, cx| {
            // Called after any model.update()
            cx.notify();
        }).detach();  // Important!
        
        Self { model }
    }
}
```

### Subscribing to Events

Listen for specific typed events:

```rust
// Define event type
#[derive(Clone)]
pub enum TerminalEvent {
    Output(String),
    TitleChanged(String),
    Exited(i32),
}

// Emit events
impl Terminal {
    fn receive_data(&mut self, data: &[u8], cx: &mut Context<Self>) {
        self.buffer.append(data);
        cx.emit(TerminalEvent::Output(String::from_utf8_lossy(data).into()));
        cx.notify();
    }
}

// Subscribe to events
impl TerminalView {
    fn new(terminal: Entity<Terminal>, cx: &mut Context<Self>) -> Self {
        cx.subscribe(&terminal, |this, _terminal, event, cx| {
            match event {
                TerminalEvent::Output(_) => this.scroll_to_bottom(cx),
                TerminalEvent::TitleChanged(title) => this.set_title(title, cx),
                TerminalEvent::Exited(code) => this.handle_exit(*code, cx),
            }
        }).detach();
        
        Self { terminal }
    }
}
```

### Subscription Cleanup

Always call `.detach()` or store the handle:

```rust
// Option 1: Detach immediately (most common)
cx.observe(&model, |_, _, _| {}).detach();

// Option 2: Store for manual cleanup
struct MyView {
    _subscription: Subscription,
}

let sub = cx.observe(&model, |_, _, _| {});
Self { _subscription: sub }
```

---

## Actions

Actions connect user input to application behavior.

### Defining Actions

```rust
// Simple actions (no data)
actions!(terminal, [Copy, Paste, Clear, NewTab, CloseTab]);

// Actions with data
#[derive(Clone, PartialEq, Deserialize)]
pub struct OpenFile {
    pub path: PathBuf,
}
impl_actions!(editor, [OpenFile]);

// With serde for keymap parsing
#[derive(Clone, PartialEq, Deserialize)]
pub struct Scroll {
    pub delta: f32,
}
impl_actions!(editor, [Scroll]);
```

### Handling Actions

```rust
impl TerminalView {
    fn new(cx: &mut Context<Self>) -> Self {
        // View-level handlers
        cx.on_action(|this, _: &Copy, cx| {
            if let Some(text) = this.selection_text() {
                cx.write_to_clipboard(text);
            }
        });
        
        cx.on_action(|this, _: &Paste, cx| {
            if let Some(text) = cx.read_from_clipboard() {
                this.write_input(&text, cx);
            }
        });
        
        Self { ... }
    }
}

// Global handlers at app level
fn init(cx: &mut App) {
    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.on_action(|action: &OpenFile, cx| {
        // ...
    });
}
```

### Key Bindings

```rust
// Programmatic binding
cx.bind_keys([
    KeyBinding::new("cmd-c", Copy, Some("TerminalView")),
    KeyBinding::new("cmd-v", Paste, Some("TerminalView")),
    KeyBinding::new("cmd-q", Quit, None),  // Global
]);

// Or load from keymap file (JSON/TOML)
let keymap = load_keymap("keymap.json")?;
cx.bind_keys(keymap);
```

---

## Performance

### Efficient Rendering

```rust
// ❌ Expensive computation in render
impl Render for ListView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let sorted = self.items.iter()
            .sorted_by_key(|i| &i.name)  // Every frame!
            .collect::<Vec<_>>();
        // ...
    }
}

// ✅ Cache computed values
struct ListView {
    items: Vec<Item>,
    sorted_indices: Vec<usize>,  // Cached
}

impl ListView {
    fn set_items(&mut self, items: Vec<Item>, cx: &mut Context<Self>) {
        self.items = items;
        self.sorted_indices = (0..self.items.len())
            .sorted_by_key(|&i| &self.items[i].name)
            .collect();
        cx.notify();
    }
}
```

### Minimize Notifications

```rust
// ❌ Notify for each item
fn add_items(&mut self, items: Vec<Item>, cx: &mut Context<Self>) {
    for item in items {
        self.items.push(item);
        cx.notify();  // Triggers render for each!
    }
}

// ✅ Batch updates
fn add_items(&mut self, items: Vec<Item>, cx: &mut Context<Self>) {
    self.items.extend(items);
    cx.notify();  // Single render
}
```

### Virtual Lists

For large lists, render only visible items:

```rust
impl Render for ListView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        uniform_list(
            cx.entity().clone(),
            "items",
            self.items.len(),
            |this, range, cx| {
                range.map(|i| this.render_item(i, cx)).collect()
            }
        )
    }
}
```

---

## Async Operations

### Spawning Tasks

```rust
impl MyView {
    fn load_data(&mut self, cx: &mut Context<Self>) {
        self.is_loading = true;
        cx.notify();
        
        cx.spawn(async move |this, mut cx| {
            // Async work (runs off main thread)
            let result = fetch_data().await;
            
            // Update back on main thread
            this.update(&mut cx, |this, cx| {
                this.data = Some(result);
                this.is_loading = false;
                cx.notify();
            })?;
            
            Ok(())
        }).detach();
    }
}
```

### Cancellation

```rust
struct MyView {
    current_task: Option<Task<()>>,
}

impl MyView {
    fn start_search(&mut self, query: String, cx: &mut Context<Self>) {
        // Cancel previous search
        self.current_task.take();
        
        // Start new search
        let task = cx.spawn(async move |this, mut cx| {
            let results = search(&query).await;
            this.update(&mut cx, |this, cx| {
                this.results = results;
                cx.notify();
            })
        });
        
        self.current_task = Some(task);
    }
}
```

---

## Testing

### Basic Test Setup

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;
    
    #[gpui::test]
    async fn test_counter(cx: &mut TestAppContext) {
        let counter = cx.new(|_| Counter { value: 0 });
        
        cx.update(|cx| {
            counter.update(cx, |c, _| c.value += 1);
        });
        
        cx.update(|cx| {
            assert_eq!(counter.read(cx).value, 1);
        });
    }
}
```

### Testing Views

```rust
#[gpui::test]
async fn test_view_interaction(cx: &mut TestAppContext) {
    let view = cx.add_window(|cx| MyView::new(cx));
    
    // Simulate user input
    cx.simulate_keystrokes("cmd-n");
    
    // Check result
    cx.update(|cx| {
        let view = view.read(cx);
        assert!(view.has_new_item());
    });
}
```

### Testing Actions

```rust
#[gpui::test]
async fn test_action_handling(cx: &mut TestAppContext) {
    let view = cx.add_window(|cx| {
        let v = MyView::new(cx);
        cx.on_action(|this, _: &Increment, cx| {
            this.count += 1;
            cx.notify();
        });
        v
    });
    
    cx.dispatch_action(Increment);
    
    cx.update(|cx| {
        assert_eq!(view.read(cx).count, 1);
    });
}
```

---

## Common Mistakes

| Mistake | Problem | Solution |
|---------|---------|----------|
| Missing `.detach()` | Memory leak, callbacks don't fire | Always call `.detach()` |
| Missing `cx.notify()` | View doesn't update | Call after state changes |
| Storing `cx` | Compilation error / unsound | Pass as parameter |
| Blocking in render | UI freezes | Cache computed values |
| Blocking in spawn | Thread pool exhaustion | Use async APIs |
| Clone in every render | Performance | Cache or borrow |
