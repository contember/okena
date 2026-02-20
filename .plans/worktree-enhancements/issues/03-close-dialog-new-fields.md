# Issue 03: Add new fields and processing states to close worktree dialog

**Priority:** high
**Files:** `src/views/overlays/close_worktree_dialog.rs`

Extend the existing `CloseWorktreeDialog` with new fields and processing states. This issue adds the data model; the UI and execution flow changes come in issues 04 and 05.

## Changes

### Add ProcessingState variants
Extend the existing enum:
```rust
enum ProcessingState {
    Idle,
    Stashing,    // NEW
    Fetching,    // NEW
    Rebasing,
    Merging,
    Pushing,         // NEW
    DeletingBranch,  // NEW
    Removing,
}
```

### Add new fields to `CloseWorktreeDialog`
```rust
stash_enabled: bool,          // checkbox: stash before merge (only when dirty + merge)
fetch_enabled: bool,          // checkbox: fetch before rebase (default true when merge)
delete_branch_enabled: bool,  // checkbox: delete branch after merge
push_enabled: bool,           // checkbox: push target branch after merge
unpushed_count: usize,        // count of unpushed commits
```

### Constructor changes
In `new()`:
- Call `git::count_unpushed_commits(&path)` to populate `unpushed_count`
- Initialize `stash_enabled: false`
- Initialize `fetch_enabled: true` (default on)
- Initialize `delete_branch_enabled: false`
- Initialize `push_enabled: false`

### Update `can_merge()` method
Currently: `!self.is_dirty && self.branch.is_some() && self.default_branch.is_some()`
Change to: `(!self.is_dirty || self.stash_enabled) && self.branch.is_some() && self.default_branch.is_some()`
This allows merge when dirty if stash is enabled.

### Update `confirm_label()` method
No changes needed (already returns "Merge & Close" vs "Close Worktree" based on `merge_enabled && can_merge()`).

### Update `status_text` in render
Add the new variants to the match in `render()`:
```rust
ProcessingState::Stashing => Some("Stashing changes..."),
ProcessingState::Fetching => Some("Fetching remote..."),
ProcessingState::Pushing => Some("Pushing branch..."),
ProcessingState::DeletingBranch => Some("Deleting branch..."),
```

Run `cargo build` to verify compilation.
