# Issue 09: Add custom worktree path option to creation dialog

**Priority:** medium
**Files:** `src/views/overlays/worktree_dialog.rs`

Add an option to specify a custom path when creating a worktree, instead of always using the auto-generated `{repo}-wt/{branch}/` path.

## Changes

### New fields
Add to `WorktreeDialog`:
```rust
custom_path_input: Entity<SimpleInputState>,
use_custom_path: bool,
```

### Constructor
In `new()`:
```rust
let custom_path_input = cx.new(|cx| {
    SimpleInputState::new(cx)
        .placeholder("Custom worktree path...")
});
// use_custom_path: false
```

### Update `create_worktree()`
After determining the branch, compute target path:
```rust
let target_path = if self.use_custom_path {
    let custom = self.custom_path_input.read(cx).value().trim().to_string();
    if custom.is_empty() {
        self.error_message = Some("Custom path cannot be empty".to_string());
        cx.notify();
        return;
    }
    custom
} else {
    self.get_target_path(&branch)
};
```

### UI changes in `render()`
After the branch list, add a custom path section:

1. A checkbox row: "Use custom path" with toggle behavior
2. When enabled, show:
   - The `SimpleInput` for custom path, pre-filled with `get_target_path()` result for the selected branch
   - When a branch is selected (or typed), update the custom path input's value to `get_target_path(&branch)` if the user hasn't manually edited it

Implementation approach for the auto-fill:
- When `use_custom_path` is toggled ON, pre-fill with the auto-generated path for the currently selected branch
- Track whether the user has manually edited the custom path (e.g. by comparing to the auto-generated value)
- Keep it simple: just pre-fill once on toggle, don't continuously update

### Checkbox UI pattern
Follow the same checkbox pattern from `close_worktree_dialog.rs`:
```rust
div()
    .id("custom-path-checkbox")
    .flex().items_center().gap(px(8.0)).py(px(4.0))
    .cursor_pointer()
    .on_click(cx.listener(|this, _, _window, cx| {
        this.use_custom_path = !this.use_custom_path;
        if this.use_custom_path {
            // Pre-fill with auto-generated path
            let branch = /* get selected or typed branch */;
            let path = this.get_target_path(&branch);
            this.custom_path_input.update(cx, |input, _cx| {
                input.set_value(&path);
            });
        }
        cx.notify();
    }))
    // ... checkbox indicator div + label
```

Run `cargo build` to verify compilation.
