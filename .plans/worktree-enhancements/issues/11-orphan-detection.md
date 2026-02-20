# Issue 11: Add orphan worktree detection with visual indicator

**Priority:** low
**Files:** `src/views/panels/sidebar/mod.rs`, `src/views/panels/sidebar/project_list.rs`

Detect worktree projects whose parent project no longer exists in the workspace and show a visual indicator.

## Changes

### `src/views/panels/sidebar/mod.rs`

Add `is_orphan: bool` field to `SidebarProjectInfo`.

When building `SidebarProjectInfo` for worktree projects, check if the parent exists:
```rust
let is_orphan = project.worktree_info.as_ref().map_or(false, |wt| {
    !ws.data().projects.iter().any(|p| p.id == wt.parent_project_id)
});
```

Also, orphan worktrees won't appear under any parent in `worktree_children_map`. They should be rendered as top-level items (they already would be if not found as children). Verify this behavior — if a worktree's parent doesn't exist, it should still appear in the sidebar as a top-level project item (using `render_worktree_item` style but at top level).

### `src/views/panels/sidebar/project_list.rs`

In `render_worktree_item()`, add an orphan indicator:
- Replace the git-branch icon color: when `project.is_orphan`, use `t.warning` color instead of `t.text_secondary`
- Optionally add a small warning icon or tooltip indicating "Parent project not found"

Simple approach — just change the icon color:
```rust
.child(
    svg()
        .path("icons/git-branch.svg")
        .size(px(14.0))
        .text_color(if project.is_orphan {
            rgb(t.warning)
        } else {
            rgb(t.text_secondary)
        })
)
```

This is a subtle visual indicator that doesn't add UI noise but alerts users to orphaned worktrees.

Run `cargo build` to verify compilation.
