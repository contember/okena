# Issue 04: Add checkbox UI for new close dialog options

**Priority:** high
**Files:** `src/views/overlays/close_worktree_dialog.rs`

Add the UI elements for the new options in the close worktree dialog's `render()` method. Follow the existing checkbox pattern used for the merge checkbox (lines 461-534 in the current file).

## UI Elements to Add

### 1. Unpushed commits warning
Below the dirty warning (after the `.when(self.is_dirty, ...)` block), add:
```
.when(self.unpushed_count > 0 && !self.merge_enabled, |d| {
    // Warning box similar to dirty warning
    // Text: "N unpushed commit(s) on this branch."
    // Subtext: "Enable merge to preserve them, or they will remain on the unmerged branch."
    // Use amber/warning color (same as dirty warning)
})
```

### 2. Stash checkbox
Visible when dirty AND merge checkbox row is visible. Place right after the merge checkbox row:
```
.when(self.is_dirty && self.branch.is_some() && self.default_branch.is_some(), |d| {
    // Checkbox: "Stash changes before merge"
    // Subtext: "Auto-pop on failure"
    // Enabled when merge is enabled (or always clickable)
    // Clicking toggles self.stash_enabled and cx.notify()
})
```

### 3. Fetch checkbox
Visible when merge is enabled. Place after stash checkbox:
```
.when(self.merge_enabled && self.can_merge(), |d| {
    // Checkbox: "Fetch remote before rebase"
    // Subtext: "git fetch --all"
    // Default checked (self.fetch_enabled starts true)
})
```

### 4. Delete branch checkbox
Visible when merge is enabled:
```
.when(self.merge_enabled && self.can_merge(), |d| {
    // Checkbox: "Delete branch after merge"
    // Subtext: "local + remote"
})
```

### 5. Push checkbox
Visible when merge is enabled:
```
.when(self.merge_enabled && self.can_merge(), |d| {
    // Checkbox: "Push target branch after merge"
    // Subtext: format!("git push origin {}", default_branch_display)
})
```

## Checkbox Pattern
Reuse the exact same checkbox pattern from the existing merge checkbox: a 16x16 div with border, rounded corners, check icon when enabled. Each checkbox row has a label and sublabel. Follow the color scheme: `t.border_active` for active border, `t.text_primary` for label, `t.text_muted` for sublabel.

## Layout
The merge options (fetch, delete branch, push) should be grouped visually — consider indenting them slightly (e.g. 8px extra left padding) under the merge checkbox to show they are sub-options of merge.

Run `cargo build` to verify compilation.
