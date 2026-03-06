# Issue 02: Create VCS dispatch module and register modules

**Priority:** high
**Files:** `src/vcs.rs`, `src/main.rs`

Create the `src/vcs.rs` dispatch layer that detects which VCS backend to use and delegates to the appropriate module. Also register both new modules in `src/main.rs`.

## `src/vcs.rs` — Types and functions

### Enum
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VcsBackend {
    Git,
    Jujutsu,
}
```

### `pub fn detect_vcs(path: &Path) -> Option<VcsBackend>`
1. Check `crate::jj::is_jj_repo(path)` first — if true, return `Some(VcsBackend::Jujutsu)`
2. Check `crate::git::is_git_repo(path)` — if true, return `Some(VcsBackend::Git)`
3. Return `None`

jj is checked first because colocated repos have both `.jj/` and `.git/`, and we prefer jj.

### `pub fn is_vcs_repo(path: &Path) -> bool`
```rust
detect_vcs(path).is_some()
```

### `pub fn get_vcs_status(path: &Path) -> Option<GitStatus>`
1. Call `detect_vcs(path)`
2. For `Git` → return `crate::git::get_git_status(path)` (uses git's own cache)
3. For `Jujutsu` → check jj cache, if fresh return cached; else call `crate::jj::get_status(path)` and cache
4. For `None` → return `None`

Implement a jj-specific cache using the same pattern as `src/git/mod.rs`:
```rust
use std::sync::Mutex;
use std::time::Instant;
use std::collections::HashMap;

struct CacheEntry {
    status: Option<GitStatus>,
    timestamp: Instant,
}

static JJ_CACHE: Mutex<Option<HashMap<PathBuf, CacheEntry>>> = Mutex::new(None);
const CACHE_TTL: Duration = Duration::from_secs(5);
```

### `pub fn get_diff_file_summary(path: &Path) -> Vec<FileDiffSummary>`
Dispatch to `crate::jj::get_diff_file_summary(path)` or `crate::git::get_diff_file_summary(path)` based on `detect_vcs(path)`. Return empty vec for `None`.

### `pub fn get_diff_with_options(path: &Path, mode: DiffMode, ignore_whitespace: bool) -> Result<DiffResult, String>`
Dispatch to jj or git based on `detect_vcs(path)`. Return `Err("Not a version-controlled repository")` for `None`.

### `pub fn get_file_contents_for_diff(repo_path: &Path, file_path: &str, mode: DiffMode) -> (Option<String>, Option<String>)`
Dispatch to jj or git based on `detect_vcs(repo_path)`. Return `(None, None)` for `None`.

## `src/main.rs` — Module registration

Add two lines near the existing `mod git;` declaration (around line 9):
```rust
mod jj;
mod vcs;
```

## Imports needed in `src/vcs.rs`
```rust
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use std::collections::HashMap;
use crate::git::{self, GitStatus, FileDiffSummary};
use crate::git::diff::{DiffResult, DiffMode};
use crate::jj;
```

## Tests to include

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_vcs_nonexistent_path() {
        assert!(detect_vcs(Path::new("/nonexistent/path")).is_none());
    }

    #[test]
    fn test_is_vcs_repo_nonexistent_path() {
        assert!(!is_vcs_repo(Path::new("/nonexistent/path")));
    }
}
```

## Acceptance Criteria
- `VcsBackend` enum with `Git` and `Jujutsu` variants
- Detection prefers jj over git
- All dispatch functions implemented
- jj-specific status cache with 5-second TTL
- `mod jj;` and `mod vcs;` added to `src/main.rs`
- Tests pass
- `cargo build` succeeds
