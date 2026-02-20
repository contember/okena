# Issue 06: Add "Focus Parent Project" context menu item for worktrees

**Priority:** medium
**Files:** `src/views/overlays/context_menu.rs`, `src/views/overlay_manager.rs`, `src/views/root/handlers.rs`

Add a context menu option on worktree projects that focuses their parent project in the sidebar.

## Changes

### `src/views/overlays/context_menu.rs`

1. Add variant to `ContextMenuEvent`:
   ```rust
   FocusParent { project_id: String },
   ```

2. Add method:
   ```rust
   fn focus_parent(&self, cx: &mut Context<Self>) {
       cx.emit(ContextMenuEvent::FocusParent {
           project_id: self.request.project_id.clone(),
       });
   }
   ```

3. Add menu item in `render()`, inside the existing `.when(is_worktree, ...)` block, **before** the "Close Worktree" item. Add a new `.when(is_worktree, ...)` block with:
   ```rust
   menu_item("context-menu-focus-parent", "icons/arrow-up.svg", "Focus Parent Project", &t)
       .on_click(cx.listener(|this, _, _window, cx| {
           this.focus_parent(cx);
       }))
   ```

   Note: Check if `icons/arrow-up.svg` exists. If not, use an appropriate existing icon like `icons/chevron-up.svg` or `icons/external-link.svg`. As a fallback, use `icons/git-branch.svg`.

### `src/views/overlay_manager.rs`

1. Add to `OverlayManagerEvent`:
   ```rust
   FocusParent { project_id: String },
   ```

2. In the context menu event subscription (inside `show_context_menu()`), add handler:
   ```rust
   ContextMenuEvent::FocusParent { project_id } => {
       this.hide_context_menu(cx);
       cx.emit(OverlayManagerEvent::FocusParent {
           project_id: project_id.clone(),
       });
   }
   ```

### `src/views/root/handlers.rs`

Add handler in `handle_overlay_manager_event`:
```rust
OverlayManagerEvent::FocusParent { project_id } => {
    let parent_id = self.workspace.read(cx)
        .project(project_id)
        .and_then(|p| p.worktree_info.as_ref())
        .map(|wt| wt.parent_project_id.clone());

    if let Some(parent_id) = parent_id {
        self.workspace.update(cx, |ws, cx| {
            ws.set_focused_project(Some(parent_id), cx);
        });
    }
}
```

Run `cargo build` to verify compilation.
