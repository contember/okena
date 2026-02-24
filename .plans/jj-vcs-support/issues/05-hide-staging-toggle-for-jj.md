# Issue 05: Hide staging mode toggle for jj repos in diff viewer

**Priority:** medium
**Files:** `src/views/overlays/diff_viewer/mod.rs`

Jujutsu has no staging area (no index), so the "Unstaged/Staged" toggle in the diff viewer should be hidden when the VCS backend is jj. Both modes would show the same diff anyway.

## Changes

### `src/views/overlays/diff_viewer/mod.rs`

Add a field to `DiffViewer` to track the VCS backend:
```rust
use crate::vcs::VcsBackend;

// In the DiffViewer struct:
vcs_backend: Option<VcsBackend>,
```

In `DiffViewer::new()`, after the VCS repo check, detect the backend:
```rust
let vcs_backend = vcs::detect_vcs(std::path::Path::new(&project_path));
// ... store in viewer.vcs_backend
```

In the render method, find where the "Unstaged"/"Staged" toggle button is rendered and wrap it with a condition:
```rust
.when(self.vcs_backend != Some(VcsBackend::Jujutsu), |el| {
    // render the mode toggle button
})
```

If the toggle is hidden, force `DiffMode::WorkingTree` as the default (it should already be the default).

## Acceptance Criteria
- Diff viewer shows the staging toggle for Git repos (existing behavior unchanged)
- Diff viewer hides the staging toggle for jj repos
- jj repos always use `DiffMode::WorkingTree` (both modes produce identical output anyway)
- `cargo build` succeeds
