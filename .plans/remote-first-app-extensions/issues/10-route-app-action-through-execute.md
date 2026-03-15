# Issue 10: Route AppAction through execute_action

**Priority:** medium
**Files:** `src/workspace/actions/execute.rs`

## Description

Add handling for `ActionRequest::AppAction` in the main action executor so that REST API `POST /v1/actions` can dispatch app-specific actions.

## Implementation

### `src/workspace/actions/execute.rs`

Add a match arm for the new `AppAction` variant:

```rust
ActionRequest::AppAction { project_id, app_id, action } => {
    match app_registry.handle_action(&app_id, action, cx) {
        Ok(()) => Ok(serde_json::json!({})),
        Err(e) => Err(e),
    }
}
```

The `app_registry: Arc<AppEntityRegistry>` needs to be threaded into `execute_action()`. Update the function signature and all call sites:

1. `execute_action()` — add `app_registry: &AppEntityRegistry` parameter
2. Call sites in `remote_commands.rs` — pass the registry
3. Any other call sites — pass the registry

This follows the same pattern as how `workspace`, `backend`, and `terminals` are already threaded through.

## Acceptance Criteria

- `POST /v1/actions` with `{ "action": "AppAction", "project_id": "...", "app_id": "...", "action": {...} }` routes to the correct app
- Invalid app_id returns an error
- Invalid action payload returns a deserialization error
- Function signature change doesn't break existing call sites
- `cargo build` succeeds
