# State Management

Patterns for managing application state in GPUI applications.

---

## State Location

### Decision Guide

```
Where should this state live?

├── Global to app? → AppState entity (singleton)
│   Examples: settings, theme, user session
│
├── Per-window? → Window-level entity
│   Examples: layout, active tab
│
├── Shared between views? → Shared Model entity
│   Examples: document content, selection
│
├── View-specific? → Field on view struct
│   Examples: scroll position, hover state
│
└── Computed from other state? → Derive on demand
    Examples: filtered list, search results
```

---

## Centralized State

### Global State Pattern

GPUI has built-in support for global state via `impl Global`:

```rust
pub struct AppState {
    pub settings: Settings,
    pub theme: Theme,
    pub sessions: Vec<Entity<Session>>,
    pub active_session: Option<usize>,
}

impl Global for AppState {}

impl AppState {
    pub fn active(&self) -> Option<&Entity<Session>> {
        self.active_session.and_then(|i| self.sessions.get(i))
    }
}

// Initialize during app startup
app.run(|cx: &mut App| {
    cx.set_global(AppState {
        settings: Settings::load().unwrap_or_default(),
        theme: Theme::default(),
        sessions: Vec::new(),
        active_session: None,
    });
});

// Read from anywhere (immutable)
fn some_function(cx: &App) {
    let state = cx.global::<AppState>();
    let settings = &state.settings;
}

// Update (mutable)
fn update_theme(cx: &mut App) {
    cx.global_mut::<AppState>().theme = Theme::dark();
}

// Observe changes from a view
cx.observe_global::<AppState>(|this, cx| {
    this.refresh_theme(cx);
    cx.notify();
}).detach();
```

### State Events

Notify observers about specific changes:

```rust
#[derive(Clone)]
pub enum AppEvent {
    SessionAdded(usize),
    SessionRemoved(usize),
    ActiveChanged(Option<usize>),
    SettingsChanged,
    ThemeChanged,
}

impl AppState {
    pub fn add_session(&mut self, session: Entity<Session>, cx: &mut Context<Self>) {
        let index = self.sessions.len();
        self.sessions.push(session);
        self.active_session = Some(index);
        
        cx.emit(AppEvent::SessionAdded(index));
        cx.emit(AppEvent::ActiveChanged(self.active_session));
        cx.notify();
    }
}

// Subscribe to specific events (in window context)
cx.subscribe_in(&app_state, window, |this, _state, event, window, cx| {
    match event {
        AppEvent::ThemeChanged => this.update_colors(window, cx),
        AppEvent::SettingsChanged => this.reload_config(window, cx),
        _ => {}
    }
}).detach();
```

---

## View-Model Separation

Keep business logic out of views:

```rust
// Model: business logic, no UI
pub struct Document {
    content: Rope,
    cursor: Position,
    selection: Option<Selection>,
    undo_stack: Vec<Edit>,
}

impl Document {
    pub fn insert(&mut self, text: &str, cx: &mut Context<Self>) {
        let edit = Edit::insert(self.cursor, text);
        self.apply_edit(&edit);
        self.undo_stack.push(edit);
        cx.emit(DocumentEvent::Changed);
        cx.notify();
    }
    
    pub fn selected_text(&self) -> Option<String> {
        self.selection.map(|sel| self.content.slice(sel.range()).to_string())
    }
}

// View: UI only, delegates to model
pub struct EditorView {
    document: Entity<Document>,
    scroll_offset: f32,
    line_height: f32,
}

impl EditorView {
    fn handle_keypress(&mut self, key: &str, _window: &mut Window, cx: &mut Context<Self>) {
        self.document.update(cx, |doc, cx| {
            doc.insert(key, cx);
        });
    }
}
```

---

## State Machines

### Enum-Based State

```rust
#[derive(Debug, Clone)]
pub enum ProcessState {
    Idle,
    Starting { config: Config },
    Running { pid: u32, started_at: Instant },
    Paused { pid: u32, reason: String },
    Exited { code: i32, duration: Duration },
    Failed { error: String },
}

pub struct Process {
    state: ProcessState,
}

impl Process {
    pub fn start(&mut self, config: Config, cx: &mut Context<Self>) -> Result<(), ProcessError> {
        match &self.state {
            ProcessState::Idle | ProcessState::Exited { .. } | ProcessState::Failed { .. } => {
                self.state = ProcessState::Starting { config };
                self.spawn_process(cx);
                cx.notify();
                Ok(())
            }
            ProcessState::Starting { .. } => Err(ProcessError::AlreadyStarting),
            ProcessState::Running { .. } => Err(ProcessError::AlreadyRunning),
            ProcessState::Paused { .. } => Err(ProcessError::IsPaused),
        }
    }
    
    pub fn is_running(&self) -> bool {
        matches!(self.state, ProcessState::Running { .. })
    }
}
```

### Typestate (Compile-Time Safety)

```rust
// States as types
pub struct Disconnected;
pub struct Connecting;
pub struct Connected { session_id: String }

pub struct Connection<S> {
    config: Config,
    _state: std::marker::PhantomData<S>,
}

impl Connection<Disconnected> {
    pub fn new(config: Config) -> Self {
        Self { config, _state: std::marker::PhantomData }
    }
    
    pub async fn connect(self) -> Result<Connection<Connected>, Error> {
        let session_id = establish_connection(&self.config).await?;
        Ok(Connection {
            config: self.config,
            _state: std::marker::PhantomData,
        })
    }
}

impl Connection<Connected> {
    pub fn send(&mut self, msg: Message) -> Result<(), Error> {
        // Can only send when connected
    }
}

// Compiler enforces: can't call send() on Disconnected
```

---

## Derived State

### Compute on Demand

```rust
impl Document {
    // Base state
    fn lines(&self) -> &[Line] {
        &self.lines
    }
    
    // Derived: compute when needed
    fn visible_lines(&self, viewport: Range<usize>) -> &[Line] {
        &self.lines[viewport]
    }
    
    fn line_count(&self) -> usize {
        self.lines.len()
    }
    
    fn is_modified(&self) -> bool {
        !self.undo_stack.is_empty()
    }
}
```

### Cached Derived State

For expensive computations:

```rust
pub struct SearchableList {
    items: Vec<Item>,
    query: String,
    
    // Cached search results
    filtered_indices: Vec<usize>,
    cache_valid: bool,
}

impl SearchableList {
    pub fn set_query(&mut self, query: String, cx: &mut Context<Self>) {
        if self.query != query {
            self.query = query;
            self.cache_valid = false;
            cx.notify();
        }
    }
    
    pub fn filtered(&mut self) -> impl Iterator<Item = &Item> {
        if !self.cache_valid {
            self.filtered_indices = self.items.iter()
                .enumerate()
                .filter(|(_, item)| item.matches(&self.query))
                .map(|(i, _)| i)
                .collect();
            self.cache_valid = true;
        }
        
        self.filtered_indices.iter().map(|&i| &self.items[i])
    }
    
    pub fn add_item(&mut self, item: Item, cx: &mut Context<Self>) {
        self.items.push(item);
        self.cache_valid = false;  // Invalidate cache
        cx.notify();
    }
}
```

---

## Data Flow

### Unidirectional Flow

```
┌─────────────────────────────────────────────────────┐
│                     User Action                      │
│                   (click, keypress)                  │
└────────────────────────┬────────────────────────────┘
                         │
                         ▼
┌─────────────────────────────────────────────────────┐
│                       Action                         │
│                (Copy, Paste, Save, etc.)            │
└────────────────────────┬────────────────────────────┘
                         │
                         ▼
┌─────────────────────────────────────────────────────┐
│                    State Update                      │
│              (model.update() + cx.notify())          │
└────────────────────────┬────────────────────────────┘
                         │
                         ▼
┌─────────────────────────────────────────────────────┐
│                     Re-render                        │
│                   (views observe)                    │
└─────────────────────────────────────────────────────┘
```

### Anti-Pattern: Bidirectional Updates

```rust
// ❌ Circular observation - infinite loop risk
impl ViewA {
    fn new(view_b: Entity<ViewB>, cx: &mut Context<Self>) {
        cx.observe(&view_b, |this, _, cx| {
            this.sync_from_b(cx);  // Updates ViewA
        }).detach();
    }
}

impl ViewB {
    fn new(view_a: Entity<ViewA>, cx: &mut Context<Self>) {
        cx.observe(&view_a, |this, _, cx| {
            this.sync_from_a(cx);  // Updates ViewB → triggers ViewA again
        }).detach();
    }
}

// ✅ Single source of truth
struct SharedState {
    data: Data,
}

impl ViewA {
    fn new(state: Entity<SharedState>, cx: &mut Context<Self>) {
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        Self { state }
    }
}

impl ViewB {
    fn new(state: Entity<SharedState>, cx: &mut Context<Self>) {
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        Self { state }
    }
}
```

---

## Checklist

When designing state:

- [ ] Can I identify the single source of truth for each piece of data?
- [ ] Is state ownership clear (who creates, who updates)?
- [ ] Are state changes always followed by `cx.notify()`?
- [ ] Is derived state computed rather than duplicated?
- [ ] Is data flow unidirectional?
- [ ] Can I trace how any piece of state changes?
