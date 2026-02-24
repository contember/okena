# Native App Pane Support + KruhPane for Okena

> Source: (conversation — plan mode transcript ca5f357e)

## Context

Okena's layout system (`LayoutNode`) currently only supports `Terminal`, `Split`, and `Tabs` leaf/container types. We want to embed custom GPUI apps as first-class panes alongside terminals — splittable, tabbable, draggable. The first app is **KruhPane**: a native Rust/GPUI rewrite of [kruh](https://github.com/contember/kruh), an automated AI agent loop tool.

**Scope (this PR):** App pane infrastructure + core kruh loop (config UI, agent spawning, output streaming, progress tracking, pause/skip/quit controls). TDD mode, task browser, plan picker, per-issue overrides deferred to follow-ups.

---

## Architecture Decisions

### LayoutNode::App Variant

Add `App` variant to `LayoutNode` as a leaf (like `Terminal`):

```rust
App {
    app_id: Option<String>,       // None = uninitialized, Some = active
    #[serde(default)]
    app_kind: AppKind,
    #[serde(default)]
    app_config: serde_json::Value, // Per-app serialized config
},
```

`AppKind` enum (extensible for future apps):
```rust
#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum AppKind { #[default] Kruh }
```

### AppPaneEntity Enum (not trait object)

GPUI entities are concrete types, so we use an enum rather than trait objects:

```rust
pub enum AppPaneEntity {
    Kruh(Entity<KruhPane>),
}
impl AppPaneEntity {
    pub fn into_any_element(&self, cx: &mut App) -> AnyElement;
    pub fn display_name(&self, cx: &App) -> String;
    pub fn icon_path(&self) -> &str;
    pub fn app_id(&self, cx: &App) -> Option<String>;
}
```

### PaneDrag Generalization

`PaneDrag` currently has `terminal_id` and `terminal_name`. Generalize to support both:

```rust
pub struct PaneDrag {
    pub project_id: String,
    pub layout_path: Vec<usize>,
    pub pane_id: String,       // terminal_id or app_id
    pub pane_name: String,
    pub icon_path: String,     // "icons/terminal.svg" or "icons/kruh.svg"
}
```

### KruhPane Agent Communication

Same pattern as Okena's PTY reader threads:
- `std::process::Command` with `Stdio::piped()`
- Background thread reads stdout line-by-line into `async_channel`
- `cx.spawn()` async task polls the channel, updates entity via `WeakEntity<KruhPane>` + `this.update(cx, |pane, cx| { ... cx.notify() })`

---

## File Structure

### New Files (10)

| File | Purpose |
|------|---------|
| `src/views/layout/app_pane.rs` | `AppPaneEntity` enum |
| `src/views/layout/kruh_pane/mod.rs` | `KruhPane` entity |
| `src/views/layout/kruh_pane/config.rs` | `KruhConfig`, agent definitions |
| `src/views/layout/kruh_pane/agent.rs` | Agent subprocess management |
| `src/views/layout/kruh_pane/status_parser.rs` | STATUS.md parser, prompt builder |
| `src/views/layout/kruh_pane/git.rs` | Git snapshot/diff helpers |
| `src/views/layout/kruh_pane/render.rs` | GPUI render implementation |
| `src/views/layout/kruh_pane/loop_runner.rs` | Async iteration loop |
| `src/views/layout/kruh_pane/types.rs` | Shared types and enums |
| `assets/icons/kruh.svg` | App icon |

### Modified Files (~15)

| File | Change |
|------|--------|
| `src/workspace/state.rs` | `App` variant + `AppKind` enum + helper methods + match arm updates |
| `src/workspace/actions/mod.rs` | Add `pub mod app;` |
| `src/workspace/actions/app.rs` *(new)* | `add_app`, `set_app_id`, `close_app` methods |
| `src/workspace/actions/execute.rs` | Handle `CreateApp`/`CloseApp` actions |
| `src/workspace/focus.rs` | `app_id` field on `FocusTarget` |
| `src/views/layout/mod.rs` | Add `pub mod app_pane; pub mod kruh_pane;` |
| `src/views/layout/layout_container.rs` | `app_pane` field, `ensure_app_pane()`, `render_app()`, render match |
| `src/views/layout/pane_drag.rs` | Generalize to support app panes (icon_path field) |
| `src/views/layout/tabs/mod.rs` | App-aware tab rendering + context menu |
| `src/views/panels/sidebar/mod.rs` | `SidebarCursorItem::App` variant |
| `src/views/panels/sidebar/project_list.rs` | `render_app_item()` |
| `src/views/overlays/command_palette.rs` | "New Kruh App" command |
| `crates/okena-core/src/api.rs` | `App` API node + `CreateApp`/`CloseApp` actions |
| `src/action_dispatch.rs` | Route new action variants |
| `Cargo.toml` | Add `which` crate |

---

## Reusable Existing Code

- `crate::theme::theme(cx)` — all styling (`src/theme/mod.rs`)
- `crate::views::components::SimpleInput` / `SimpleInputState` — config form inputs (`src/views/components/simple_input.rs`)
- `crate::views::components::ui_helpers::*` — buttons, badges, kbd hints (`src/views/components/ui_helpers.rs`)
- `crate::process::command()` — cross-platform subprocess spawning (`src/process.rs`)
- `crate::impl_focusable!()` — focus handle boilerplate (`src/macros.rs`)
- `async_channel` — line-by-line output streaming (same as PTY reader pattern)
- `smol::unblock` / `smol::Timer` — async file I/O and sleep
- `ScrollHandle` — output auto-scroll

---

## KruhPane — Core Logic (ported from TypeScript kruh)

### Config (`config.rs`)

`KruhConfig` struct (serializable to `app_config` JSON):
- `agent: String` — one of: claude, codex, opencode, aider, goose, amp, cursor, copilot
- `model: String` — model name (e.g. "claude-sonnet-4-6")
- `max_iterations: usize` — default 100
- `sleep_secs: u64` — default 2
- `docs_dir: String` — path to docs directory containing INSTRUCTIONS.md + STATUS.md
- `dangerous: bool` — skip permission prompts (claude-specific --dangerously-skip-permissions)

Eight agent definitions (ported from kruh's `agents.ts`):

| Agent | Binary | Command pattern |
|---|---|---|
| claude | `claude` | `claude [--model M] [--dangerously-skip-permissions] -p <prompt>` |
| codex | `codex` | `codex -q <prompt>` |
| opencode | `opencode` | `opencode run [--model M] <prompt>` |
| aider | `aider` | `aider --yes --message <prompt> [--model M]` |
| goose | `goose` | `goose run -t <prompt>` |
| amp | `amp` | `amp -x <prompt>` |
| cursor | `cursor` | `cursor --cli <prompt>` |
| copilot | `copilot` | `copilot <prompt>` |

Agent detection: `which::which(binary)` to check availability.

### Status Parser (`status_parser.rs`)

`StatusProgress` struct: `pending`, `done`, `total` counts + `pending_issues`/`done_issues` name lists.

Parsing: read STATUS.md, match lines:
- `^- \[ \] (.+)$` → pending
- `^- \[x\] (.+)$` → done

Extract issue refs: `^(\d+)\s*[—–-]\s*(.+)$` for number + name pairs.

### Prompt Builder (`status_parser.rs`)

`build_prompt(docs_dir)` — reads INSTRUCTIONS.md + STATUS.md, generates agent prompt:
> "Read INSTRUCTIONS.md and STATUS.md. Find the first pending issue, [read its file from issues/,] implement it, verify (type-check, tests, build), and update STATUS.md. If no pending issues remain, respond: `<done>promise</done>`"

Checks whether `issues/` subdirectory exists to customize the prompt.

### Agent Process (`agent.rs`)

`AgentHandle` struct:
- Wraps `std::process::Child`
- `stdout_receiver: async_channel::Receiver<String>` for line-by-line output
- Background thread reads stdout via `BufRead::lines()` and sends into channel

`spawn_agent(config, project_path)` — builds command from agent definition, spawns with piped stdio.
`kill_agent(handle)` — SIGTERM, escalate to SIGKILL after 3s.

### Git Helpers (`git.rs`)

- `get_snapshot(project_path)` → `git rev-parse HEAD`
- `get_diff_stat(project_path, before, after)` → `git diff --stat before..after`
- Uses `crate::process::command("git")`

### Loop Runner (`loop_runner.rs`)

Main iteration loop as `cx.spawn()` async task:
```
1. Parse STATUS.md (via smol::unblock for file I/O)
2. Check completion (all done or max iterations)
3. Build prompt
4. Spawn agent subprocess
5. Stream output lines (poll async_channel receiver)
6. Wait for agent exit
7. Capture git diff
8. Update KruhPane state via this.update(cx, ...)
9. Sleep (smol::Timer) or wait for step confirmation
10. Loop or break
```

Communication with KruhPane entity via `WeakEntity<KruhPane>` + `this.update(cx, |pane, cx| { ... cx.notify() })`.

### Render (`render.rs`)

GPUI render regions:
1. **Header bar** — App icon + "Kruh" + config summary (agent, model, docs dir)
2. **Progress bar** — `done/total` with colored fill (red < 25%, yellow < 75%, green >= 75%)
3. **Iteration banner** — "Iteration N/MAX" + elapsed time
4. **Output display** — Scrollable div with styled text lines (auto-scroll, basic ANSI stripping)
5. **Diff display** — Git diff --stat with green/red coloring
6. **Control bar** — Keyboard hints: (P)ause (S)kip (Q)uit S(t)ep + state indicator
7. **Config panel** (Idle state) — Editable config fields via `SimpleInput` + "Start" button

### KruhPane Entity (`mod.rs`)

Main struct fields:
```rust
pub struct KruhPane {
    workspace: Entity<Workspace>,
    project_id: String,
    project_path: String,
    layout_path: Vec<usize>,
    app_id: Option<String>,
    focus_handle: FocusHandle,
    config: KruhConfig,
    state: KruhState,  // Idle | Running | Paused | Completed
    iteration: usize,
    pass_count: usize,
    fail_count: usize,
    start_time: Option<Instant>,
    agent_handle: Option<AgentHandle>,
    output_lines: Vec<OutputLine>,
    output_scroll: ScrollHandle,
    diff_stat: Option<String>,
    progress: StatusProgress,
    paused: bool,
    step_mode: bool,
    skip_requested: bool,
    quit_requested: bool,
    _loop_task: Option<gpui::Task<()>>,
}
```

Implements `Render`, `EventEmitter<KruhPaneEvent>`, `Focusable`.

---

## Persistence Behavior

- `app_config` in `LayoutNode::App` stores `KruhConfig` as JSON
- On workspace restore, KruhPane is recreated in `Idle` state (loop does NOT auto-restart)
- Output history is NOT persisted (starts fresh)

---

## Testing

### Unit Tests (`state.rs` `#[cfg(test)]`)
- Serialization round-trip for `LayoutNode::App`
- `collect_terminal_ids()` on mixed Terminal + App trees (apps excluded)
- `collect_app_ids()` returns only app IDs
- `find_app_path()` in nested split/tab trees
- Backward compat: old JSON without App nodes deserializes

### Unit Tests (`status_parser.rs` `#[cfg(test)]`)
- `parse_status()` on various STATUS.md formats
- `build_prompt()` with/without issues/ directory
- Agent command building for each agent

### Manual Verification
- Create Kruh app pane via command palette → appears in layout
- Split/tab alongside terminals → drag between splits
- Configure docs dir, agent, model → start loop
- Live output streaming → pause/skip/quit
- Close app pane → agent subprocess killed
- Restart Okena → app pane restored in Idle state with saved config
