# Comprehensive Worktree Workflow Enhancements

> Source: `/Users/nebula/.claude/plans/fuzzy-brewing-scott.md`

## Context

The basic worktree close dialog (with dirty check, merge, and hooks) has been implemented in `src/views/overlays/close_worktree_dialog.rs`. This plan adds the next set of worktree workflow improvements: enhanced close flow (stash, fetch, delete branch, push, unpushed detection), sidebar enhancements (jump to parent, worktree badge, close-all), creation flow improvements (custom path, create from PR), and housekeeping (orphan detection).

## Architecture Overview

### Git Operations Pattern
All git commands in `src/git/repository.rs` follow this pattern:
```rust
pub fn operation(path: &Path) -> Result<(), String> {
    let path_str = path.to_str().ok_or("Invalid path")?;
    let output = command("git")
        .args(["-C", path_str, "subcommand", "args"])
        .output()
        .map_err(|e| format!("Failed to execute git: {}", e))?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(stderr.trim().to_string())
    }
}
```
Uses `crate::process::command("git")` helper. Re-exports go in `src/git/mod.rs`.

### Close Dialog Async Pattern
The dialog uses `cx.spawn(async move |this, cx| { ... })` with `smol::unblock()` for blocking git ops. State updates use:
```rust
let _ = cx.update(|cx| {
    this.update(cx, |this, cx| {
        this.processing = ProcessingState::Variant;
        cx.notify();
    })
});
```
`AsyncApp::update()` returns `()`. `Entity::update()` with `&mut App` also returns `()`. The final workspace removal must be done inside `cx.update()` closure.

### Context Menu Event Flow
1. `ContextMenu` renders items conditionally (`.when(condition, |d| d.child(...))`)
2. Click handler calls method that emits `ContextMenuEvent::Variant`
3. `OverlayManager` subscribes to context menu, maps to `OverlayManagerEvent::Variant`
4. `RootView::handle_overlay_manager_event()` handles the final event

### Sidebar Data Flow
- `SidebarProjectInfo` struct in `sidebar/mod.rs` holds computed display data
- `worktree_children_map: HashMap<String, Vec<SidebarProjectInfo>>` built during render by matching `worktree_info.parent_project_id`
- `render_project_item()` renders parent projects, `render_worktree_item()` renders worktree children indented at 28px

### Key Files Reference
- **Git ops**: `src/git/repository.rs`, `src/git/mod.rs`
- **Close dialog**: `src/views/overlays/close_worktree_dialog.rs`
- **Context menu**: `src/views/overlays/context_menu.rs`
- **Overlay manager**: `src/views/overlay_manager.rs`
- **Root handlers**: `src/views/root/handlers.rs`
- **Sidebar**: `src/views/panels/sidebar/mod.rs`, `project_list.rs`, `item_widgets.rs`
- **Worktree create dialog**: `src/views/overlays/worktree_dialog.rs`
- **Workspace actions**: `src/workspace/actions/project.rs`
- **Workspace state**: `src/workspace/state.rs` (`WorktreeMetadata`, `ProjectData`)

### Existing ProcessingState enum
```rust
enum ProcessingState {
    Idle,
    Rebasing,
    Merging,
    Removing,
}
```

### Existing CloseWorktreeDialog fields
```rust
pub struct CloseWorktreeDialog {
    workspace: Entity<Workspace>,
    focus_handle: FocusHandle,
    project_id: String,
    project_name: String,
    project_path: String,
    branch: Option<String>,
    default_branch: Option<String>,
    main_repo_path: Option<String>,
    is_dirty: bool,
    merge_enabled: bool,
    error_message: Option<String>,
    processing: ProcessingState,
}
```

### Existing ContextMenuEvent variants
```rust
pub enum ContextMenuEvent {
    Close,
    AddTerminal { project_id: String },
    CreateWorktree { project_id: String, project_path: String },
    RenameProject { project_id: String, project_name: String },
    CloseWorktree { project_id: String },
    DeleteProject { project_id: String },
    ConfigureHooks { project_id: String },
}
```

### UI Component Patterns
- Buttons: `button(id, label, &t)`, `button_primary(id, label, &t)`
- Checkboxes: Custom div with border + check icon (see close_worktree_dialog.rs lines 461-534)
- Badge: `sidebar_terminal_badge(has_layout, terminal_count, &t)` in item_widgets.rs
- Menu items: `menu_item(id, icon, label, &t)`, `menu_item_with_color(id, icon, label, color, hover_color, &t)`
- Modal: `modal_backdrop(id, &t)`, `modal_content(id, &t)`

### Testing Guidelines
- Tests live in `#[cfg(test)]` modules inside source files
- Git functions: test error paths with invalid paths (returns Err/false/0/empty)
- Use `#[test]` (not `#[gpui::test]`) for pure logic tests
- Files with `use gpui::*;` import gpui's `test` proc macro — in `#[cfg(test)]` submodules, use specific imports
