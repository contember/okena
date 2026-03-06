# Issue 10: KruhPane entity — main module wiring

**Priority:** high
**Files:** `src/views/layout/kruh_pane/mod.rs` (new), `src/views/layout/mod.rs`

## Description

Create the main `KruhPane` entity that wires together all the submodules (types, config, agent, status_parser, git, render, loop_runner). This is the central struct that GPUI manages.

## New file: `src/views/layout/kruh_pane/mod.rs`

### Module declarations

```rust
pub mod types;
pub mod config;
pub mod agent;
pub mod status_parser;
pub mod git;
pub mod render;
pub mod loop_runner;
```

### Imports

```rust
use gpui::*;
use std::time::Instant;
use crate::impl_focusable;
use crate::workspace::state::Workspace;
use self::types::*;
use self::config::KruhConfig;
use crate::views::components::simple_input::SimpleInputState;
```

### `KruhPane` struct

```rust
pub struct KruhPane {
    // Context
    pub(crate) workspace: Entity<Workspace>,
    pub(crate) project_id: String,
    pub(crate) project_path: String,
    pub(crate) layout_path: Vec<usize>,
    pub(crate) app_id: Option<String>,
    pub(crate) focus_handle: FocusHandle,

    // Config
    pub(crate) config: KruhConfig,

    // State
    pub(crate) state: KruhState,
    pub(crate) iteration: usize,
    pub(crate) pass_count: usize,
    pub(crate) fail_count: usize,
    pub(crate) start_time: Option<Instant>,

    // Agent
    pub(crate) agent_handle: Option<agent::AgentHandle>,

    // Output
    pub(crate) output_lines: Vec<OutputLine>,
    pub(crate) output_scroll: ScrollHandle,
    pub(crate) diff_stat: Option<String>,

    // Progress
    pub(crate) progress: StatusProgress,

    // Control flags
    pub(crate) paused: bool,
    pub(crate) step_mode: bool,
    pub(crate) skip_requested: bool,
    pub(crate) quit_requested: bool,
    pub(crate) step_advance_requested: bool,

    // Config input entities (for Idle state UI)
    pub(crate) docs_dir_input: Entity<SimpleInputState>,
    pub(crate) agent_input: Entity<SimpleInputState>,
    pub(crate) model_input: Entity<SimpleInputState>,
    pub(crate) max_iterations_input: Entity<SimpleInputState>,
    pub(crate) dangerous_input: Entity<SimpleInputState>,

    // Background task
    _loop_task: Option<gpui::Task<()>>,
}
```

### Constructor

```rust
impl KruhPane {
    pub fn new(
        workspace: Entity<Workspace>,
        project_id: String,
        project_path: String,
        layout_path: Vec<usize>,
        app_id: Option<String>,
        config: KruhConfig,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();

        // Create input entities pre-filled with config values
        let docs_dir_input = cx.new(|cx| {
            SimpleInputState::new(cx)
                .placeholder("Path to docs directory...")
                .default_value(&config.docs_dir)
        });
        let agent_input = cx.new(|cx| {
            SimpleInputState::new(cx)
                .placeholder("Agent name...")
                .default_value(&config.agent)
        });
        let model_input = cx.new(|cx| {
            SimpleInputState::new(cx)
                .placeholder("Model name...")
                .default_value(&config.model)
        });
        let max_iterations_input = cx.new(|cx| {
            SimpleInputState::new(cx)
                .placeholder("Max iterations...")
                .default_value(&config.max_iterations.to_string())
        });
        let dangerous_input = cx.new(|cx| {
            SimpleInputState::new(cx)
                .placeholder("yes/no")
                .default_value(if config.dangerous { "yes" } else { "no" })
        });

        Self {
            workspace,
            project_id,
            project_path,
            layout_path,
            app_id,
            focus_handle,
            config,
            state: KruhState::Idle,
            iteration: 0,
            pass_count: 0,
            fail_count: 0,
            start_time: None,
            agent_handle: None,
            output_lines: Vec::new(),
            output_scroll: ScrollHandle::new(),
            diff_stat: None,
            progress: StatusProgress::default(),
            paused: false,
            step_mode: false,
            skip_requested: false,
            quit_requested: false,
            step_advance_requested: false,
            docs_dir_input,
            agent_input,
            model_input,
            max_iterations_input,
            dangerous_input,
            _loop_task: None,
        }
    }
}
```

### Helper methods

```rust
impl KruhPane {
    /// Add an output line (called from the loop runner and UI actions)
    pub(crate) fn add_output(&mut self, text: &str, is_error: bool) {
        self.output_lines.push(OutputLine {
            text: text.to_string(),
            timestamp: Instant::now(),
            is_error,
        });
        // Auto-scroll to bottom
        self.output_scroll.scroll_to_end();
    }

    /// Read config from input fields and start the loop
    pub(crate) fn start_loop(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Read current values from input entities
        self.config.docs_dir = self.docs_dir_input.read(cx).value().to_string();
        self.config.agent = self.agent_input.read(cx).value().to_string();
        self.config.model = self.model_input.read(cx).value().to_string();
        self.config.max_iterations = self.max_iterations_input.read(cx).value()
            .parse().unwrap_or(100);
        self.config.dangerous = self.dangerous_input.read(cx).value() == "yes";

        // Validate docs_dir
        if self.config.docs_dir.is_empty() {
            self.add_output("Error: docs directory is required", true);
            cx.notify();
            return;
        }

        // Reset state
        self.state = KruhState::Running;
        self.iteration = 0;
        self.pass_count = 0;
        self.fail_count = 0;
        self.output_lines.clear();
        self.diff_stat = None;
        self.start_time = Some(Instant::now());
        self.quit_requested = false;
        self.skip_requested = false;
        self.paused = false;

        // Save config to layout node's app_config
        if let Ok(config_json) = serde_json::to_value(&self.config) {
            self.workspace.update(cx, |ws, cx| {
                ws.with_layout_node(&self.project_id, &self.layout_path, cx, |node| {
                    if let LayoutNode::App { ref mut app_config, .. } = node {
                        *app_config = config_json;
                    }
                });
            });
        }

        // Spawn the loop task
        let this = cx.weak_entity();
        self._loop_task = Some(loop_runner::start_loop(this, cx));

        self.add_output("Loop started.", false);
        cx.notify();
    }
}
```

### Trait implementations

```rust
impl EventEmitter<KruhPaneEvent> for KruhPane {}

impl_focusable!(KruhPane);

impl Render for KruhPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.render_view(window, cx)
    }
}
```

### Keyboard action handling

Register keyboard handlers for the control keys. Use GPUI's action system or `on_key_down`:

```rust
impl KruhPane {
    fn handle_key(&mut self, key: &str, _window: &mut Window, cx: &mut Context<Self>) {
        match (key, &self.state) {
            ("p", KruhState::Running | KruhState::Paused) => {
                self.paused = !self.paused;
                if !self.paused {
                    self.state = KruhState::Running;
                }
                cx.notify();
            }
            ("s", KruhState::Running | KruhState::Paused) => {
                self.skip_requested = true;
                cx.notify();
            }
            ("q", _) if self.state != KruhState::Idle => {
                self.quit_requested = true;
                cx.notify();
            }
            ("t", _) if self.state != KruhState::Idle => {
                self.step_mode = !self.step_mode;
                cx.notify();
            }
            _ => {}
        }
    }
}
```

Note: Check how TerminalPane handles key events in this codebase and follow the same pattern. It might use GPUI actions registered via `on_action()` or direct `on_key_down()`.

### Drop cleanup

```rust
impl Drop for KruhPane {
    fn drop(&mut self) {
        // Kill any running agent
        if let Some(mut handle) = self.agent_handle.take() {
            handle.kill();
        }
        // The _loop_task is dropped automatically, which cancels the async task
    }
}
```

## Changes to `src/views/layout/mod.rs`

Add the module declarations:

```rust
pub mod app_pane;
pub mod kruh_pane;
```

## Acceptance Criteria

- `KruhPane` struct compiles with all fields
- Constructor initializes all fields and creates SimpleInput entities
- `start_loop()` reads config from inputs, validates, and spawns the loop task
- `add_output()` appends lines and auto-scrolls
- Keyboard shortcuts control pause/skip/quit/step
- `Render` trait delegates to `render_view()`
- `EventEmitter` and `Focusable` traits implemented
- Drop handler kills the agent process
- Module declarations added to `layout/mod.rs`
- `cargo build` succeeds
