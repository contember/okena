# GPUI Framework - Comprehensive Patterns & Architecture Guide

> Extracted from Zed editor source at commit `b642565` in `~/.cargo/git/checkouts/zed-a70e2ad075855582/b642565/`

---

## Table of Contents

1. [Application Architecture](#1-application-architecture)
2. [Context Types](#2-context-types)
3. [Entity & State Management](#3-entity--state-management)
4. [Component Composition](#4-component-composition)
5. [Actions & Click Handlers](#5-actions--click-handlers)
6. [Event Propagation & Occlusion](#6-event-propagation--occlusion)
7. [Modals & Dialogs](#7-modals--dialogs)
8. [Text Inputs & IME](#8-text-inputs--ime)
9. [Drag & Drop, Menus, Tooltips](#9-drag--drop-context-menus-popovers-tooltips)
10. [View-Model Separation](#10-view-model-separation)
11. [UI Testing](#11-ui-testing)

---

## 1. Application Architecture

### Bootstrap Flow

```rust
// crates/zed/src/main.rs
let app = Application::new().with_assets(Assets);

app.run(move |cx| {
    // 1. Initialize global systems
    settings::init(cx);
    theme::init(theme::LoadThemes::All(...), cx);

    // 2. Register actions
    menu::init();
    zed_actions::init();

    // 3. Set up global state
    <dyn Fs>::set_global(fs.clone(), cx);

    // 4. Open window with root view
    cx.open_window(options, |window, cx| {
        cx.new(|cx| Workspace::new(..., window, cx))
    });
});
```

### Window Creation

```rust
pub fn open_window<V: 'static + Render>(
    &mut self,
    options: WindowOptions,
    build_root_view: impl FnOnce(&mut Window, &mut App) -> Entity<V>,
) -> Result<WindowHandle<V>>
```

**WindowOptions** (key fields):
- `window_bounds` - size, position, maximized
- `titlebar` - custom titlebar config
- `focus`, `show` - auto behavior
- `window_min_size`, `is_resizable`, `is_movable`
- `window_background` - transparent, opaque
- `window_decorations` - CSD/SSD on Linux

### Asset Management

Assets are embedded at compile time via `RustEmbed`:
```rust
#[derive(RustEmbed)]
#[folder = "../../assets"]
#[include = "fonts/**/*"]
#[include = "icons/**/*"]
pub struct Assets;

// Loading fonts:
for font_path in asset_source.list("fonts")? {
    let font_bytes = asset_source.load(font_path)?;
    cx.text_system().add_fonts(vec![font_bytes]);
}
```

Icons are referenced as `icons/<name>.svg` in code.

---

## 2. Context Types

GPUI has three main context types with different scopes:

| Context | Scope | Access | When Used |
|---------|-------|--------|-----------|
| `&mut App` | Global | All entities, windows, globals | App init, global operations |
| `&mut Window` | Window | Single window, rendering | Window rendering, events |
| `&mut Context<T>` | Entity | Entity state + App (via Deref) | Entity methods, render() |

```rust
// App context - initialization
app.run(|cx: &mut App| {
    cx.set_global(MyGlobal::new());
});

// Window context - from open_window builder
cx.open_window(opts, |window: &mut Window, cx: &mut App| { ... });

// Entity context - inside entity methods
impl MyView {
    fn do_something(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.notify(); // triggers re-render
        cx.emit(MyEvent); // emit event
    }
}
```

---

## 3. Entity & State Management

### Creating Entities

```rust
let entity: Entity<MyState> = cx.new(|cx| MyState::new());
```

### Reading & Updating

```rust
// Read (immutable borrow, cheap)
let value = entity.read(cx).some_field;

// Update (mutable, uses "lease" pattern internally)
entity.update(cx, |state, cx| {
    state.some_field = 42;
    cx.notify(); // MUST call to trigger re-renders
});
```

### Observers & Subscriptions

```rust
// Observe notify() calls
cx.observe(&entity, |this, changed_entity, cx| {
    // Fires when entity calls cx.notify()
}).detach();

// Subscribe to emitted events
cx.subscribe(&entity, |this, emitter, event: &MyEvent, cx| {
    // Fires when entity calls cx.emit(MyEvent)
}).detach();

// Subscribe in window context (most common in views)
cx.subscribe_in(&entity, window, |this, emitter, event, window, cx| {
    // Has access to both window and entity context
}).detach();

// Cleanup on entity drop
cx.on_release(|this, cx| {
    // Entity is being dropped
}).detach();
```

### Event Emission

```rust
// 1. Implement EventEmitter
impl EventEmitter<MyEvent> for MyView {}

// 2. Define event type
pub enum MyEvent { Changed, Closed }

// 3. Emit events
cx.emit(MyEvent::Changed);
```

### Global State

```rust
// Define global
impl Global for AppSettings {}

// Set
cx.set_global(AppSettings::new());

// Read
let settings = cx.global::<AppSettings>();

// Observe changes
cx.observe_global::<AppSettings>(|cx| {
    // React to global state change
}).detach();
```

### WeakEntity (Break Reference Cycles)

```rust
let weak = entity.downgrade(); // WeakEntity<T>

// In closures (prevent entity from being kept alive)
cx.spawn(async move |cx| {
    if let Some(entity) = weak.upgrade() {
        entity.update(&mut cx, |this, cx| { ... });
    }
});
```

### Async Operations

```rust
// Spawn task with entity access
cx.spawn(async move |this: WeakEntity<Self>, mut cx: AsyncApp| {
    let result = fetch_data().await;
    this.update(&mut cx, |this, cx| {
        this.data = result;
        cx.notify();
    })?;
    Ok(())
});

// Spawn in window context (common)
cx.spawn_in(window, async move |this, cx| {
    // Has access to both window and entity
});
```

---

## 4. Component Composition

### Render vs RenderOnce

| Trait | Ownership | State | Use Case |
|-------|-----------|-------|----------|
| `Render` | `&mut self` | Stateful (Entity-backed) | Complex views with lifecycle |
| `RenderOnce` | `self` (consumed) | Stateless (data struct) | Reusable components |

```rust
// Stateful view (backed by Entity<V>)
impl Render for MyView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div().child("Hello")
    }
}

// Stateless component (consumed on render)
#[derive(IntoElement)]
pub struct MyButton {
    label: SharedString,
    on_click: Option<Box<dyn Fn(&ClickEvent, &mut Window, &mut App)>>,
}

impl RenderOnce for MyButton {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        div().child(self.label)
    }
}
```

### The div() Builder Pattern

```rust
div()
    .id("my-element")           // Required for stateful interactions
    .flex()                      // Display: flex
    .flex_col()                  // flex-direction: column
    .gap_4()                     // gap between children
    .p_6()                       // padding
    .rounded_md()                // border-radius
    .bg(cx.theme().colors().background)
    .child(Label::new("Title"))
    .child(Button::new("btn", "Click"))
```

### Layout Helpers

```rust
// Horizontal flex with items_center
h_flex().gap_4().child(icon).child(label)

// Vertical flex
v_flex().gap_2().children(items)
```

### Conditional Rendering (FluentBuilder)

```rust
div()
    .when(condition, |this| this.bg(red))
    .when_else(cond, |this| this.visible(), |this| this.invisible())
    .when_some(optional_value, |this, val| this.child(Label::new(val)))
    .when_none(&optional, |this| this.child("No value"))
    .map(|this| custom_transform(this))
```

### ParentElement Trait

```rust
// Any component can accept children
impl ParentElement for Modal {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements)
    }
}

// Usage:
Modal::new("id")
    .child(header)
    .child(content)
    .children(items.iter().map(|i| render_item(i)))
```

### Key Traits for Components

| Trait | Purpose | Method |
|-------|---------|--------|
| `Render` | Stateful rendering | `render(&mut self, ...)` |
| `RenderOnce` | Stateless rendering | `render(self, ...)` |
| `IntoElement` | Convert to element | `into_element()` |
| `ParentElement` | Accept children | `child()`, `children()` |
| `Styled` | CSS-like styling | `flex()`, `p()`, `bg()` |
| `FluentBuilder` | Conditional logic | `when()`, `when_some()` |
| `Clickable` | Click handling | `on_click()` |
| `Focusable` | Focus management | `focus_handle()` |

---

## 5. Actions & Click Handlers

### Defining Actions

```rust
// Simple actions (unit structs)
actions!(workspace, [
    ActivateNextPane,
    ActivatePreviousPane,
    CloseWindow,
]);

// Actions with parameters
#[derive(Clone, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = editor)]
pub struct SelectNext {
    pub replace_newest: bool,
}
```

### Registering Action Handlers

```rust
impl Render for MyView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .key_context("MyView")  // Scope for keybindings
            .on_action(cx.listener(Self::handle_close))
            .on_action(cx.listener(Self::handle_save))
    }
}

impl MyView {
    fn handle_close(&mut self, _: &CloseWindow, window: &mut Window, cx: &mut Context<Self>) {
        // Handle action
        cx.notify();
    }
}
```

### The `cx.listener()` Pattern

Creates a closure that maintains view state access:
```rust
// cx.listener wraps entity access
.on_action(cx.listener(|this: &mut Self, action: &MyAction, window, cx| {
    this.do_something(action, window, cx);
}))

// Equivalent to:
.on_action({
    let view = cx.entity().downgrade();
    move |action: &MyAction, window: &mut Window, cx: &mut App| {
        view.update(cx, |this, cx| this.do_something(action, window, cx)).ok();
    }
})
```

### Click Handlers

```rust
div()
    .id("button")
    .on_click(cx.listener(|this, event: &ClickEvent, window, cx| {
        // ClickEvent can be Mouse or Keyboard (Enter/Space on focused element)
    }))
    .on_mouse_down(MouseButton::Left, cx.listener(|this, event, window, cx| {
        cx.stop_propagation(); // Prevent parent handlers
    }))
```

### Action Dispatch

```rust
// Dispatch programmatically
window.dispatch_action(MyAction.boxed_clone(), cx);

// From focus handle
focus_handle.dispatch_action(&MyAction, window, cx);
```

### Propagation Control

```rust
fn handle_action(&mut self, _: &MyAction, _: &mut Window, cx: &mut Context<Self>) {
    if !self.can_handle {
        cx.propagate(); // Let parent handle it
        return;
    }
    // Handle action (stops propagation by default in bubble phase)
}
```

### Action Dispatch Flow

```
1. Global Capture Phase (top-down)
2. Window Capture Phase (root → focused)
3. Window Bubble Phase (focused → root) ← most common
4. Global Bubble Phase (bottom-up)
```

---

## 6. Event Propagation & Occlusion

### Two-Phase Event Model

```rust
pub enum DispatchPhase {
    Capture,  // Back-to-front (root toward target)
    Bubble,   // Front-to-back (target toward root) - DEFAULT
}
```

### Stopping Propagation

```rust
// Stop all further handlers
cx.stop_propagation();

// Prevent default behavior (e.g., parent auto-focus)
window.prevent_default();

// Resume propagation (undo stop)
cx.propagate();
```

### Hitbox Occlusion System

This is the KEY mechanism for overlays blocking clicks underneath:

```rust
pub enum HitboxBehavior {
    Normal,                  // Default, doesn't block
    BlockMouse,              // Blocks ALL mouse events behind
    BlockMouseExceptScroll,  // Blocks clicks but allows scroll-through
}

// Usage:
div()
    .occlude()                    // BlockMouse - blocks everything behind
    .block_mouse_except_scroll()  // BlockMouseExceptScroll
```

### How Hit Testing Works

1. Hitboxes tested from TOP to BOTTOM (reverse paint order)
2. `BlockMouse` hitbox stops further testing
3. `is_hovered()` only checks hitboxes above the blocking layer
4. `should_handle_scroll()` checks ALL hitboxes (ignores blocking)

### Modal Overlay Pattern

```rust
div()
    .absolute().size_full().inset_0()
    .occlude()  // Block ALL mouse events to background
    .bg(semi_transparent_background)
    .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| {
        this.dismiss(window, cx); // Click backdrop = dismiss
    }))
    .child(
        h_flex()
            .occlude()  // Modal content also blocks
            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                cx.stop_propagation(); // Don't trigger backdrop dismiss
            })
            .child(modal_content)
    )
```

---

## 7. Modals & Dialogs

### ModalView Trait

```rust
pub trait ModalView: ManagedView {
    fn on_before_dismiss(&mut self, _: &mut Window, _: &mut Context<Self>) -> DismissDecision {
        DismissDecision::Dismiss(true)
    }
    fn fade_out_background(&self) -> bool { false }
    fn render_bare(&self) -> bool { false }
}

pub trait ManagedView: Focusable + EventEmitter<DismissEvent> + Render {}
```

### Creating a Modal

```rust
// 1. Define modal struct
pub struct MyModal {
    focus_handle: FocusHandle,
    // ... fields
}

// 2. Implement required traits
impl EventEmitter<DismissEvent> for MyModal {}
impl ModalView for MyModal {
    fn fade_out_background(&self) -> bool { true }
}

impl Focusable for MyModal {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

// 3. Implement Render
impl Render for MyModal {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .key_context("MyModal")
            .on_action(cx.listener(|_, _: &menu::Cancel, _, cx| {
                cx.emit(DismissEvent); // Escape to close
            }))
            .on_action(cx.listener(|this, _: &menu::Confirm, window, cx| {
                this.confirm(window, cx);
                cx.emit(DismissEvent);
            }))
            .child(/* content */)
    }
}

// 4. Show it
workspace.toggle_modal(window, cx, |window, cx| {
    MyModal::new(window, cx)
});
```

### DismissDecision

```rust
pub enum DismissDecision {
    Dismiss(true),   // Allow dismissal
    Dismiss(false),  // Block dismissal
    Pending,         // Async operation in progress, defer
}

impl ModalView for MyModal {
    fn on_before_dismiss(&mut self, _: &mut Window, _: &mut Context<Self>) -> DismissDecision {
        if self.has_unsaved_changes() {
            DismissDecision::Dismiss(false)
        } else {
            DismissDecision::Dismiss(true)
        }
    }
}
```

### Modal Focus Management

ModalLayer automatically:
1. Saves `previous_focus_handle` before showing modal
2. Focuses modal via `cx.defer_in(window, ...)` (next frame)
3. Restores previous focus on dismiss
4. Optionally dismisses on focus loss (`dismiss_on_focus_lost`)

### Dismiss Flow

```
User presses Escape
  → Keybinding dispatches menu::Cancel
  → Modal's on_action listener catches it
  → Modal emits DismissEvent
  → ModalLayer subscribes, calls hide_modal()
  → on_before_dismiss() checked
  → If Dismiss(true): remove modal, restore focus, cx.notify()
```

---

## 8. Text Inputs & IME

### InputHandler Trait

The core interface for text input (mirrors Apple's NSTextInputClient):

```rust
pub trait InputHandler: 'static {
    fn selected_text_range(&mut self, ...) -> Option<UTF16Selection>;
    fn marked_text_range(&mut self, ...) -> Option<Range<usize>>;
    fn text_for_range(&mut self, range: Range<usize>, ...) -> Option<String>;
    fn replace_text_in_range(&mut self, range: Option<Range<usize>>, text: &str, ...);
    fn replace_and_mark_text_in_range(&mut self, ...); // IME composition
    fn unmark_text(&mut self, ...);
    fn bounds_for_range(&mut self, ...) -> Option<Bounds<Pixels>>;
    fn character_index_for_point(&mut self, ...) -> Option<usize>;
    fn accepts_text_input(&mut self, ...) -> bool { true }
}
```

### Registering Input Handler

```rust
// During element paint phase:
window.handle_input(
    &self.focus_handle,
    ElementInputHandler::new(element_bounds, view_entity),
    cx,
);
```

### High-Level InputField Component

```rust
// crates/ui_input/src/input_field.rs
let field = InputField::new(window, cx, "Placeholder text");
field.text(cx);  // Get text
field.set_text("value", window, cx);  // Set text
```

### Editor as Text Input (Inline Editing Pattern)

```rust
// 1. Create single-line editor
let editor = cx.new(|cx| Editor::single_line(window, cx));

// 2. Subscribe to events
cx.subscribe_in(&editor, window, |this, _, event, window, cx| {
    match event {
        EditorEvent::BufferEdited => this.validate(cx),
        EditorEvent::Blurred => this.confirm_or_cancel(window, cx),
        _ => {}
    }
}).detach();

// 3. Render conditionally
if is_editing {
    h_flex().child(self.editor.clone())
} else {
    h_flex().child(Label::new(display_text))
}
```

### Clipboard

```rust
// Copy
cx.write_to_clipboard(ClipboardItem { text: "copied".into() });

// Paste
if let Some(item) = cx.read_from_clipboard() {
    let text = item.text;
}
```

---

## 9. Drag & Drop, Context Menus, Popovers, Tooltips

### Drag & Drop

```rust
// 1. Make element draggable
div()
    .id(("item", index))
    .on_drag(MyDragData { id }, |data, cursor_offset, _, cx| {
        cx.new(|_| DragGhost::new(data)) // Visual during drag
    })

// 2. Style drop targets
div()
    .drag_over::<MyDragData>(|style, data, _, _| {
        style.bg(highlight_color)
    })

// 3. Handle drops
div()
    .on_drop(cx.listener(|this, data: &MyDragData, _, cx| {
        this.handle_drop(data);
        cx.notify();
    }))

// 4. Track drag movement
div()
    .on_drag_move::<MyDragData>(|event, window, cx| {
        let position = event.event.position;
    })
```

### Context Menus

```rust
// Build a context menu
let menu = ContextMenu::build(window, cx, |menu, _, _| {
    menu.entry("Copy", None, |_, _| { /* handler */ })
        .entry("Paste", None, |_, _| { /* handler */ })
        .separator()
        .submenu("More", |menu, _, _| {
            menu.entry("Sub Option", None, |_, _| {})
        })
});

// Right-click menu element
right_click_menu::<ContextMenu>("menu-id")
    .menu(|window, cx| {
        ContextMenu::build(window, cx, |menu, _, _| { ... })
    })
    .trigger(|is_open, window, cx| {
        my_element.toggle_state(is_open)
    })
```

### Popover Menus

```rust
PopoverMenu::new("menu-id")
    .menu(|window, cx| {
        Some(ContextMenu::build(window, cx, |menu, _, _| { ... }))
    })
    .trigger(Button::new("trigger", "Open"))
    .anchor(Corner::TopLeft)
    .attach(Corner::BottomLeft)
    .offset(point(px(0.), px(4.)))
```

**PopoverMenuHandle** for programmatic control:
```rust
let handle = PopoverMenuHandle::default();
handle.show(window, cx);
handle.hide(cx);
handle.toggle(window, cx);
handle.is_deployed();
```

### Tooltips

```rust
// Simple text tooltip
button.tooltip(Tooltip::text("Delete file"))

// With keybinding
button.tooltip(Tooltip::for_action_title("Save", &Save))

// With metadata
button.tooltip(move |window, cx| {
    Tooltip::with_meta("Save", Some(&Save), "Saves current file", cx)
})

// Hoverable (doesn't dismiss on mouse move into tooltip)
button.hoverable_tooltip(|window, cx| Tooltip::text("Hover me"))
```

### Deferred Rendering (Portals)

Elements that paint AFTER all ancestors (for floating UI):

```rust
deferred(
    anchored()
        .anchor(Corner::TopLeft)
        .snap_to_window_with_margin(px(8.))
        .child(floating_content)
)
.with_priority(1)  // Higher = on top
```

---

## 10. View-Model Separation

### Architecture

| Layer | Example | Has Render | Has Events | Purpose |
|-------|---------|-----------|-----------|---------|
| Model | Project, Buffer | No | Yes | Business logic, data |
| View | Editor, Pane | Yes | Yes | UI rendering |
| Bridge | Item trait | N/A | Yes | Connects models to views |

### Model Entity (No Render)

```rust
pub struct Project {
    buffer_store: Entity<BufferStore>,
    worktree_store: Entity<WorktreeStore>,
    // ... no UI fields
}

impl EventEmitter<ProjectEvent> for Project {}
// Does NOT implement Render
```

### View Entity (With Render)

```rust
pub struct Editor {
    buffer: Entity<MultiBuffer>,        // Reference to model
    display_map: Entity<DisplayMap>,    // View-specific state
    selections: SelectionsCollection,   // View state
    focus_handle: FocusHandle,
}

impl Render for Editor { ... }
impl EventEmitter<EditorEvent> for Editor {}
```

### Bridge: Item Trait

```rust
pub trait Item: Focusable + EventEmitter<Self::Event> + Render {
    type Event;
    fn tab_content_text(&self, detail: usize, cx: &App) -> SharedString;
    fn is_dirty(&self, cx: &App) -> bool;
    fn can_split(&self) -> bool;
    fn clone_on_split(&self, ...) -> Task<Option<Entity<Self>>>;
}

// Type-erased in containers:
pub struct Pane {
    items: Vec<Box<dyn ItemHandle>>,  // Any Item type
}
```

### Subscription-Based Reactivity

```rust
// View subscribes to model events
cx.subscribe_in(&project, window, |workspace, _, event, window, cx| {
    match event {
        ProjectEvent::WorktreeAdded(id) => {
            workspace.update_title(window, cx);
            workspace.serialize(window, cx);
        }
        // ...
    }
    cx.notify(); // Trigger re-render
}).detach();
```

---

## 11. UI Testing

### Test Infrastructure

```rust
#[gpui::test]
fn test_basic(cx: &mut TestAppContext) {
    // Create entity
    let counter = cx.new(|cx| Counter::new(cx));

    // Update state
    counter.update(cx, |c, _| c.count = 42);

    // Read state
    let count = counter.read_with(cx, |c, _| c.count);
    assert_eq!(count, 42);
}
```

### Window Tests

```rust
#[gpui::test]
fn test_with_window(cx: &mut TestAppContext) {
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| MyView::new(cx))
        }).unwrap()
    });

    let mut cx = VisualTestContext::from_window(window.into(), cx);
    let view = window.root(&mut cx).unwrap();
}
```

### Simulating Input

```rust
// Keystrokes
cx.simulate_keystrokes("cmd-p escape");

// Text input
cx.simulate_input("hello world");

// Mouse
cx.simulate_click(position, Modifiers::none());
cx.simulate_mouse_down(pos, MouseButton::Left, Modifiers::none());
cx.simulate_mouse_move(pos, Some(MouseButton::Left), Modifiers::none());

// Actions
cx.dispatch_action(MyAction);
```

### Async & Tasks

```rust
#[gpui::test]
async fn test_async(cx: &mut TestAppContext) {
    let entity = cx.new(|cx| MyEntity::new(cx));

    // Await a task directly
    entity.update(cx, |e, cx| e.load(cx)).await;

    // For detached tasks, run until parked
    entity.update(cx, |e, cx| e.reload(cx)); // detached
    cx.run_until_parked(); // execute pending tasks

    // Advance timers
    cx.executor().advance_clock(Duration::from_millis(500));
}
```

### Property Testing

```rust
#[gpui::test(iterations = 10)]
fn test_random(cx: &mut TestAppContext, mut rng: StdRng) {
    // Runs 10 times with different random seeds
    // Dispatcher randomly interleaves task execution
}
```

### Focus Testing

```rust
let focus_handle = view.read_with(&cx, |v, _| v.focus_handle.clone());

cx.update(|window, cx| {
    focus_handle.dispatch_action(&MyAction, window, cx);
});
```

### Key Test Methods

| Method | Purpose |
|--------|---------|
| `cx.new(\|cx\| ...)` | Create entity |
| `entity.update(cx, \|e, cx\| ...)` | Update entity |
| `entity.read_with(cx, \|e, _\| ...)` | Read entity |
| `cx.run_until_parked()` | Execute pending tasks |
| `cx.executor().advance_clock(dur)` | Advance timers |
| `cx.simulate_keystrokes("...")` | Type keys |
| `cx.dispatch_action(action)` | Dispatch action |
| `cx.simulate_click(pos, mods)` | Click at position |
| `VisualTestContext::from_window(...)` | Window-aware context |
| `cx.debug_bounds("element-id")` | Get element bounds |
