# Issue 03: Update watcher and diff viewer to use VCS dispatch

**Priority:** high
**Files:** `src/git/watcher.rs`, `src/views/overlays/diff_viewer/mod.rs`, `src/views/overlays/diff_viewer/syntax.rs`

Update the status watcher and diff viewer to call through the VCS dispatch layer instead of directly calling git functions.

## Changes

### `src/git/watcher.rs`

**Line ~67** — In the polling loop inside `start_watching`, change:
```rust
// Before:
let status = git::get_git_status(Path::new(&path));
// After:
let status = crate::vcs::get_vcs_status(Path::new(&path));
```

Add import at top if needed: `use crate::vcs;` — or use full path `crate::vcs::get_vcs_status`.

### `src/views/overlays/diff_viewer/mod.rs`

**Line 12 — Import change:**
```rust
// Before:
use crate::git::{get_diff_with_options, is_git_repo, DiffMode, DiffResult, FileDiff};
// After:
use crate::git::{DiffMode, DiffResult, FileDiff};
use crate::vcs;
```

**Lines 131–134 — In `DiffViewer::new()`:**
```rust
// Before:
if !is_git_repo(std::path::Path::new(&project_path)) {
    viewer.error_message = Some("Not a git repository".to_string());
    return viewer;
}
// After:
if !vcs::is_vcs_repo(std::path::Path::new(&project_path)) {
    viewer.error_message = Some("Not a version-controlled repository".to_string());
    return viewer;
}
```

**Line 173 — In `load_diff()`:**
```rust
// Before:
match get_diff_with_options(path, mode, self.ignore_whitespace) {
// After:
match vcs::get_diff_with_options(path, mode, self.ignore_whitespace) {
```

### `src/views/overlays/diff_viewer/syntax.rs`

**Line 4 — Import change:**
```rust
// Before:
use crate::git::{get_file_contents_for_diff, DiffLineType, DiffMode, FileDiff};
// After:
use crate::git::{DiffLineType, DiffMode, FileDiff};
use crate::vcs;
```

**Line 69 — In `process_file()`:**
```rust
// Before:
let (old_content, new_content) = get_file_contents_for_diff(repo_path, path, diff_mode);
// After:
let (old_content, new_content) = vcs::get_file_contents_for_diff(repo_path, path, diff_mode);
```

## Acceptance Criteria
- Watcher polls via `vcs::get_vcs_status` instead of `git::get_git_status`
- Diff viewer uses `vcs::is_vcs_repo`, `vcs::get_diff_with_options`, `vcs::get_file_contents_for_diff`
- Error message says "Not a version-controlled repository" instead of "Not a git repository"
- Types (`DiffMode`, `DiffResult`, `FileDiff`, `DiffLineType`) still imported from `crate::git` (they live there)
- `cargo build` succeeds
