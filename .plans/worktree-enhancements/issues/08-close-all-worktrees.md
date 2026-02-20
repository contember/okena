# Issue 08: Add "Close All Worktrees" context menu item

**Priority:** medium
**Files:** `src/views/overlays/context_menu.rs`, `src/views/overlay_manager.rs`, `src/views/root/handlers.rs`

Add a context menu option on parent projects (non-worktree git repos) that closes all their child worktrees at once.

## Changes

### `src/views/overlays/context_menu.rs`

1. Add variant to `ContextMenuEvent`:
   ```rust
   CloseAllWorktrees { project_id: String },
   ```

2. Add method:
   ```rust
   fn close_all_worktrees(&self, cx: &mut Context<Self>) {
       cx.emit(ContextMenuEvent::CloseAllWorktrees {
           project_id: self.request.project_id.clone(),
       });
   }
   ```

3. Determine worktree count in `render()`. Read workspace to count worktrees whose `parent_project_id == project_id`:
   ```rust
   let worktree_count = ws.data().projects.iter()
       .filter(|p| p.worktree_info.as_ref().map_or(false, |wt| wt.parent_project_id == self.request.project_id))
       .count();
   ```

4. Add menu item conditionally after "Create Worktree...":
   ```rust
   .when(is_git_repo && !is_worktree && worktree_count > 0, |d| {
       d.child(
           menu_item_with_color(
               "context-menu-close-all-worktrees",
               "icons/git-branch.svg",
               &format!("Close All Worktrees ({})", worktree_count),
               t.warning, t.warning, &t,
           )
           .on_click(cx.listener(|this, _, _window, cx| {
               this.close_all_worktrees(cx);
           }))
       )
   })
   ```

### `src/views/overlay_manager.rs`

1. Add to `OverlayManagerEvent`:
   ```rust
   CloseAllWorktrees { project_id: String },
   ```

2. In the context menu event subscription, add handler:
   ```rust
   ContextMenuEvent::CloseAllWorktrees { project_id } => {
       this.hide_context_menu(cx);
       cx.emit(OverlayManagerEvent::CloseAllWorktrees {
           project_id: project_id.clone(),
       });
   }
   ```

### `src/views/root/handlers.rs`

Add handler in `handle_overlay_manager_event`:
```rust
OverlayManagerEvent::CloseAllWorktrees { project_id } => {
    // Collect all worktree project IDs for this parent
    let worktree_ids: Vec<String> = self.workspace.read(cx)
        .data().projects.iter()
        .filter(|p| p.worktree_info.as_ref()
            .map_or(false, |wt| wt.parent_project_id == *project_id))
        .map(|p| p.id.clone())
        .collect();

    // Remove each one (non-force, skip dirty ones)
    let mut errors = Vec::new();
    for wt_id in &worktree_ids {
        let result = self.workspace.update(cx, |ws, cx| {
            ws.remove_worktree_project(wt_id, false, cx)
        });
        if let Err(e) = result {
            errors.push(e);
        }
    }
    if !errors.is_empty() {
        log::warn!("Some worktrees could not be closed: {:?}", errors);
    }
}
```

Run `cargo build` to verify compilation.
