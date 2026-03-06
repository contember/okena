# Issue 07: Generalize PaneDrag, tab bar, and focus navigation for apps

**Priority:** medium
**Files:** `src/views/layout/pane_drag.rs`, `src/views/layout/tabs/mod.rs`, `src/workspace/focus.rs`

## Description

Update drag-and-drop, tab bar rendering, and focus navigation to support both terminal and app panes. This makes apps first-class citizens in the layout interaction model.

## Changes to `src/views/layout/pane_drag.rs`

### Generalize `PaneDrag`

Replace terminal-specific fields with generic pane fields:

```rust
pub struct PaneDrag {
    pub project_id: String,
    pub layout_path: Vec<usize>,
    pub pane_id: String,       // terminal_id or app_id
    pub pane_name: String,     // display name
    pub icon_path: String,     // "icons/terminal.svg" or "icons/kruh.svg"
}
```

### Update `PaneDragView`

The drag preview view should use `icon_path` instead of a hardcoded terminal icon. Find where the terminal icon is rendered in the drag preview and replace with the dynamic `icon_path` field.

### Update all PaneDrag construction sites

Search for where `PaneDrag` is constructed (in `layout_container.rs`, `tabs/mod.rs`, etc.) and update to use the new field names:
- For terminals: `pane_id: terminal_id, pane_name: terminal_name, icon_path: "icons/terminal.svg".into()`
- For apps: `pane_id: app_id, pane_name: display_name, icon_path: app_pane.icon_path().into()`

### Update drop handling

In `move_pane()` and related drop handlers, the `pane_id` field is used to identify what's being moved. Verify that `move_pane()` in `actions/layout.rs` works correctly for both terminal and app nodes — it should already work since it operates on layout paths, not terminal IDs directly. If it uses `terminal_id` specifically, update to be pane-type-aware.

## Changes to `src/views/layout/tabs/mod.rs`

### Tab rendering for App children

In tab bar rendering, each tab currently shows a terminal icon and name. Update to detect `App` vs `Terminal` children:

```rust
// When iterating over tab children to render tab items:
match child {
    LayoutNode::Terminal { terminal_id, .. } => {
        // Existing: terminal icon + terminal name from workspace.terminal_names
    }
    LayoutNode::App { app_id, app_kind, .. } => {
        let icon = match app_kind {
            AppKind::Kruh => "icons/kruh.svg",
        };
        let name = match app_kind {
            AppKind::Kruh => "Kruh",
        };
        // Render tab with app icon + name
    }
    _ => {} // Split nodes shouldn't be tab children, but handle gracefully
}
```

### Context menu for App tabs

In the tab context menu (right-click), add appropriate options for App tabs:
- "Close App" (same as close terminal)
- Remove terminal-specific options (rename, detach, shell selector) that don't apply to apps

Check how the context menu is built and add a branch for App nodes.

## Changes to `src/workspace/focus.rs`

### Update `FocusTarget`

Add optional `app_id` field:

```rust
pub struct FocusTarget {
    pub project_id: String,
    pub layout_path: Vec<usize>,
    pub terminal_id: Option<String>,
    pub app_id: Option<String>,
}
```

### Update focus navigation

Focus navigation methods (`focus_next_terminal`, `focus_prev_terminal` in `actions/focus.rs`) traverse the layout tree collecting leaf paths. They should also visit `App` leaves:

1. Find where leaves are collected for focus cycling
2. Add `LayoutNode::App` to the leaf collection — apps are focusable leaves just like terminals
3. When focusing an App leaf, set `app_id` on the `FocusTarget` and clear `terminal_id`
4. When focusing a Terminal leaf, set `terminal_id` and clear `app_id`

The focus navigation likely uses `collect_terminal_ids()` or a similar traversal. It may need a new method like `collect_leaf_paths()` that returns paths to both Terminal and App leaves.

## Acceptance Criteria

- `PaneDrag` supports both terminal and app panes with dynamic icons
- Tab bar renders app tabs with correct icon and name
- Tab context menu shows appropriate options for app tabs
- Focus navigation visits App leaves alongside Terminal leaves
- Drag-and-drop works between terminals and apps
- `cargo build` succeeds
