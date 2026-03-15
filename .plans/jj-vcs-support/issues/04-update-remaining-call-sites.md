# Issue 04: Update remaining call sites to use VCS dispatch

**Priority:** high
**Files:** `src/views/panels/project_column.rs`, `src/views/overlays/context_menu.rs`, `src/views/root/terminal_actions.rs`, `src/workspace/actions/execute.rs`

Update all remaining call sites that use git functions directly to go through the VCS dispatch layer.

## Changes

### `src/views/panels/project_column.rs`

**Line 1 — Import change:**
```rust
// Before:
use crate::git::{self, FileDiffSummary};
// After:
use crate::git::FileDiffSummary;
use crate::vcs;
```

**Line 108 — In `show_diff_popover()`:**
```rust
// Before:
let summaries = git::get_diff_file_summary(Path::new(&project_path));
// After:
let summaries = vcs::get_diff_file_summary(Path::new(&project_path));
```

Note: Keep `use crate::git::watcher::GitStatusWatcher;` (line 2) — the watcher itself is unchanged, it just calls through vcs internally now.

### `src/views/overlays/context_menu.rs`

**Line 3 — Add import:**
```rust
use crate::vcs;
```
Keep `use crate::git;` — still needed for git-specific features.

**Line 128 — Change VCS detection to use dispatch, but also detect backend for gating:**
```rust
// Before:
let is_git_repo = git::get_git_status(std::path::Path::new(&project_path)).is_some();
// After:
let vcs_backend = vcs::detect_vcs(std::path::Path::new(&project_path));
let is_vcs_repo = vcs_backend.is_some();
let is_git_repo = vcs_backend == Some(vcs::VcsBackend::Git);
```

**Lines 167, 179 — Worktree menu items stay gated on `is_git_repo`:**
The existing `.when(is_git_repo && !is_worktree, ...)` conditions for "Create Worktree..." and "Close All Worktrees" already use `is_git_repo`, which now specifically means Git backend (not just any VCS). This is correct — worktrees are git-only.

If there are other menu items that should work for any VCS (e.g., showing the diff viewer), update those to use `is_vcs_repo` instead of `is_git_repo`.

### `src/views/root/terminal_actions.rs`

**Line 119 — In `create_worktree_from_focus()`:**
```rust
// Before:
let is_git = crate::git::get_git_status(std::path::Path::new(&project_path)).is_some();
// After:
let is_git = crate::vcs::detect_vcs(std::path::Path::new(&project_path)) == Some(crate::vcs::VcsBackend::Git);
```

This correctly gates worktree creation on Git backend only. The log message can stay as-is ("Cannot create worktree: project is not a git repo or is already a worktree").

### `src/workspace/actions/execute.rs`

**Lines 252 — `ActionRequest::GitStatus`:**
```rust
// Before:
let status = crate::git::get_git_status(std::path::Path::new(&path));
// After:
let status = crate::vcs::get_vcs_status(std::path::Path::new(&path));
```

**Lines 262 — `ActionRequest::GitDiffSummary`:**
```rust
// Before:
let summary = crate::git::get_diff_file_summary(std::path::Path::new(&path));
// After:
let summary = crate::vcs::get_diff_file_summary(std::path::Path::new(&path));
```

**Lines 272 — `ActionRequest::GitDiff`:**
```rust
// Before:
match crate::git::get_diff_with_options(std::path::Path::new(&path), mode, ignore_whitespace) {
// After:
match crate::vcs::get_diff_with_options(std::path::Path::new(&path), mode, ignore_whitespace) {
```

**Lines 294 — `ActionRequest::GitFileContents`:**
```rust
// Before:
let (old, new) = crate::git::get_file_contents_for_diff(
// After:
let (old, new) = crate::vcs::get_file_contents_for_diff(
```

**Lines 280 — `ActionRequest::GitBranches`:**
Keep as-is — `crate::git::get_available_branches_for_worktree` is git-only. Branches/worktrees are not part of the VCS abstraction.

## Acceptance Criteria
- All four files updated with VCS dispatch calls
- Worktree-related features remain gated on `VcsBackend::Git` specifically
- `GitBranches` handler stays git-only
- API action names (`GitStatus`, `GitDiffSummary`, etc.) unchanged for backward compat
- `cargo build` succeeds
- `cargo test` passes all existing tests
