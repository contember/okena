# Issue 07: Add worktree count badge on parent projects in sidebar

**Priority:** medium
**Files:** `src/views/panels/sidebar/mod.rs`, `src/views/panels/sidebar/project_list.rs`, `src/views/panels/sidebar/item_widgets.rs`

Show a small badge with the worktree count on parent projects that have active worktrees.

## Changes

### `src/views/panels/sidebar/mod.rs`

1. Add `worktree_count: usize` field to `SidebarProjectInfo` struct.

2. When building `SidebarProjectInfo` items during render, populate `worktree_count` by counting how many projects in the workspace have `worktree_info.parent_project_id == this_project.id`. This data is already computed in the `worktree_children_map` — use its length.

### `src/views/panels/sidebar/item_widgets.rs`

Add a new widget function:
```rust
pub fn sidebar_worktree_badge(count: usize, t: &ThemeColors) -> impl IntoElement {
    div()
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap(px(2.0))
        .child(
            svg()
                .path("icons/git-branch.svg")
                .size(px(10.0))
                .text_color(rgb(t.text_muted))
        )
        .child(
            div()
                .text_size(px(10.0))
                .text_color(rgb(t.text_muted))
                .child(format!("{}", count))
        )
}
```

Follow the pattern of existing `sidebar_terminal_badge()` for sizing and style.

### `src/views/panels/sidebar/project_list.rs`

In `render_project_item()`, after the existing `sidebar_terminal_badge` child (around the `.child(sidebar_terminal_badge(...))` line), add:
```rust
.when(project.worktree_count > 0, |d| {
    d.child(sidebar_worktree_badge(project.worktree_count, &t))
})
```

Run `cargo build` to verify compilation.
