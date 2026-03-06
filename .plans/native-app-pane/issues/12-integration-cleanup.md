# Issue 12: Integration wiring, cleanup, and persistence

**Priority:** medium
**Files:** `src/views/layout/layout_container.rs`, `src/app/mod.rs`, various

## Description

Final wiring pass: ensure all pieces work together, cleanup on close, persistence behavior, and verify the full flow end-to-end.

## Layout container agent cleanup

In `src/views/layout/layout_container.rs`:

When an App node is removed from the layout (via close_terminal/close_tab/etc), the `LayoutContainer`'s `app_pane` entity should be dropped, which triggers `KruhPane::drop()` and kills the agent. Verify this happens correctly:

1. When `LayoutContainer` is dropped (container removed from tree), its `app_pane: Option<AppPaneEntity>` is dropped, which drops the `Entity<KruhPane>`
2. When the layout node switches from App to something else, `self.app_pane = None` in the render path handles cleanup
3. If `LayoutContainer` is reused for a different node, ensure the old `app_pane` is dropped before creating a new one

The key entities to check are already in the `ensure_app_pane()` method: if the app_id doesn't match, a new entity is created and the old one is dropped.

## App lifecycle on workspace quit

In `src/app/mod.rs`:

When the app quits, all running agent subprocesses should be terminated. This should happen automatically through the drop chain:
- `Okena` dropped → `Workspace` entity dropped → `LayoutContainer` entities dropped → `KruhPane` entities dropped → `AgentHandle::drop()` → `kill()`

Verify this works by checking the drop order. If GPUI doesn't guarantee drop order of entities, add explicit cleanup in the quit handler:

```rust
// In the quit/close handler (if needed):
// Iterate all projects, find App nodes, and signal them to quit
```

## Persistence behavior

Verify the following persistence requirements:

1. **Config persistence**: When `start_loop()` is called, `KruhConfig` is serialized to the `app_config` field of `LayoutNode::App`. This means the config is saved with the workspace JSON and restored on reload.

2. **State on restore**: When workspace is loaded from JSON, `LayoutNode::App` nodes exist in the tree. `LayoutContainer::ensure_app_pane()` creates a new `KruhPane` in `Idle` state. The loop does NOT auto-restart — the user must click "Start" again.

3. **Output not persisted**: `output_lines` is in-memory only. Fresh on each session.

4. **app_id preservation**: The `app_id` is stored in `LayoutNode::App` and restored on reload, ensuring the pane maintains its identity.

## Edge cases to verify

1. **Close while running**: Close an App tab while the agent is running → agent should be killed
2. **Quit while running**: Quit Okena while an agent is running → all agents killed
3. **Multiple Kruh panes**: Two Kruh panes running simultaneously in different projects → they should be independent
4. **Empty docs_dir**: Starting with empty docs_dir shows error, doesn't crash
5. **Missing agent binary**: Agent not found on PATH → clear error message
6. **Layout operations**: Split, tab, drag operations work with App leaves mixed with Terminal leaves

## Workspace export/API

Verify that the API endpoint for workspace layout includes App nodes:
- `ApiLayoutNode::App` is returned in layout queries
- `CreateApp` and `CloseApp` work via the remote API

## Final cargo check

Run `cargo build` and `cargo test` to verify everything compiles and tests pass. Fix any remaining warnings.

## Acceptance Criteria

- Agent killed on pane close
- Agent killed on app quit
- Config persisted and restored
- Loop does NOT auto-restart on restore
- Multiple simultaneous Kruh panes work independently
- All edge cases handled gracefully
- API includes App nodes and actions
- `cargo build` succeeds with no warnings related to new code
- `cargo test` passes
