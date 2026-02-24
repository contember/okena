# Issue 02: Workspace app actions + API action variants + execute + dispatch

**Priority:** high
**Files:** `src/workspace/actions/app.rs` (new), `src/workspace/actions/mod.rs`, `src/workspace/actions/execute.rs`, `crates/okena-core/src/api.rs`, `src/action_dispatch.rs`

## Description

Create workspace methods for managing app panes and wire them through the action dispatch system. This enables creating and closing apps via both the desktop UI and the remote API.

## New file: `src/workspace/actions/app.rs`

`impl Workspace` block with three methods:

### `add_app(project_id, kind: AppKind, config: serde_json::Value, cx) -> Option<String>`

Creates a new `LayoutNode::App` and inserts it into the project's layout:
1. Generate a new `app_id` via `uuid::Uuid::new_v4().to_string()`
2. Get the project's current layout
3. If layout is `None`, set it to the new App node directly
4. If layout exists, wrap current root in a horizontal split: `Split { direction: Horizontal, sizes: [0.5, 0.5], children: [existing_root, new_app_node] }`
5. Call `self.notify_data(cx)`
6. Return the generated `app_id`

### `set_app_id(project_id, path: &[usize], app_id: String, cx)`

Sets the `app_id` on an existing App node at the given path:
1. Use `self.with_layout_node(project_id, path, cx, |node| { ... })`
2. If node is `LayoutNode::App { app_id: ref mut id, .. }`, set `*id = Some(app_id)`

### `close_app(project_id, app_id: &str, cx) -> bool`

Finds and removes an App node:
1. Get the project's layout, call `find_app_path(app_id)`
2. If found, use existing `close_terminal` logic (which works on any leaf via `remove_at_path`) or replicate the removal + normalize pattern
3. Call `self.notify_data(cx)`
4. Return `true` if found and removed

## Changes to `src/workspace/actions/mod.rs`

Add `pub mod app;` alongside the existing module declarations.

## Changes to `crates/okena-core/src/api.rs`

Add two new variants to `ActionRequest`:

```rust
CreateApp {
    project_id: String,
    app_kind: String,
    #[serde(default)]
    app_config: serde_json::Value,
},
CloseApp {
    project_id: String,
    app_id: String,
},
```

These should follow the existing pattern with `#[serde(tag = "action", rename_all = "snake_case")]`.

## Changes to `src/workspace/actions/execute.rs`

Add match arms in `execute_action()`:

```rust
ActionRequest::CreateApp { project_id, app_kind, app_config } => {
    let kind = match app_kind.as_str() {
        "kruh" => AppKind::Kruh,
        _ => return ActionResult::error(format!("Unknown app kind: {}", app_kind)),
    };
    match ws.add_app(&project_id, kind, app_config, cx) {
        Some(app_id) => ActionResult::success_json(serde_json::json!({ "app_id": app_id })),
        None => ActionResult::error("Failed to create app"),
    }
}
ActionRequest::CloseApp { project_id, app_id } => {
    if ws.close_app(&project_id, &app_id, cx) {
        ActionResult::success()
    } else {
        ActionResult::error("App not found")
    }
}
```

Also update `spawn_uninitialized_terminals()` / `collect_uninitialized_terminals()` to skip `LayoutNode::App` nodes (they should not be treated as uninitialized terminals).

## Changes to `src/action_dispatch.rs`

Add routing for `CreateApp` and `CloseApp` in the `ActionDispatcher::dispatch()` method:
- `Local` variant: execute via `execute_action()` (same as other actions)
- `Remote` variant: send via HTTP to remote server (same as other actions)

These are not "visual-only" actions, so they should go through the standard dispatch path for both Local and Remote.

## Acceptance Criteria

- `add_app()` creates an App node with a generated UUID, splits from existing layout
- `close_app()` finds and removes the App node, normalizes the tree
- `CreateApp` and `CloseApp` API actions work through `execute_action()`
- Remote dispatch sends these actions to the remote server
- `spawn_uninitialized_terminals()` skips App nodes
- `cargo build` succeeds
