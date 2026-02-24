# Issue 01: Create Jujutsu backend module

**Priority:** high
**Files:** `src/jj/mod.rs`

Create `src/jj/mod.rs` implementing jj-specific VCS commands that produce the same types as the git module (`GitStatus`, `FileDiffSummary`, `DiffResult`).

## Functions to implement

### `pub fn is_jj_repo(path: &Path) -> bool`
Walk up directories from `path` checking for a `.jj/` directory. Pure filesystem check — no subprocess needed. This is fast and avoids spawning a process just to detect jj.

### `pub fn get_status(path: &Path) -> Option<GitStatus>`
1. Get bookmark name: `jj log -r @ --no-graph -T 'separate(" ", bookmarks)' -R <path>`
   - If output is non-empty, use the first bookmark as `branch`
   - If empty, fallback to change ID: `jj log -r @ --no-graph -T 'change_id.short(8)' -R <path>`
2. Get diff stats: run `jj diff --git -R <path>` and parse with `crate::git::diff::parse_unified_diff`
3. Sum up `lines_added` and `lines_removed` from the parsed `DiffResult`
4. Return `Some(GitStatus { branch, lines_added, lines_removed })`, or `None` if jj commands fail

### `pub fn get_diff_file_summary(path: &Path) -> Vec<FileDiffSummary>`
1. Run `jj diff --git -R <path>`
2. Parse with `crate::git::diff::parse_unified_diff`
3. Convert each `FileDiff` to `FileDiffSummary` by counting added/removed lines in hunks
4. Detect new files (files where old path is `/dev/null`)

### `pub fn get_diff_with_options(path: &Path, _mode: DiffMode, ignore_whitespace: bool) -> Result<DiffResult, String>`
1. Build command: `jj diff --git -R <path>`
2. If `ignore_whitespace`, the `_mode` parameter is ignored — jj has no staging area so both `DiffMode::WorkingTree` and `DiffMode::Staged` produce the same output
3. Parse output with `crate::git::diff::parse_unified_diff`
4. Return `Ok(result)` or `Err(stderr)`

### `pub fn get_file_contents_for_diff(repo_path: &Path, file_path: &str, _mode: DiffMode) -> (Option<String>, Option<String>)`
1. Old content: `jj file show --revision @- -R <repo_path> <file_path>` — returns `None` if command fails (new file)
2. New content: read directly from filesystem at `repo_path.join(file_path)` — returns `None` if file doesn't exist (deleted file)
3. `_mode` is ignored (no staging area in jj)

## Imports needed
```rust
use std::path::Path;
use std::process::Command;
use crate::git::{GitStatus, FileDiffSummary};
use crate::git::diff::{parse_unified_diff, DiffResult, DiffMode};
```

## Tests to include

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_jj_repo_nonexistent_path() {
        assert!(!is_jj_repo(Path::new("/nonexistent/path")));
    }

    #[test]
    fn test_get_status_non_jj_path() {
        assert!(get_status(Path::new("/tmp")).is_none());
    }

    #[test]
    fn test_jj_diff_parses_through_unified_parser() {
        // Sample jj diff --git output
        let sample = "diff --git a/src/main.rs b/src/main.rs\n\
                       index abc1234..def5678 100644\n\
                       --- a/src/main.rs\n\
                       +++ b/src/main.rs\n\
                       @@ -1,3 +1,4 @@\n\
                        fn main() {\n\
                       +    println!(\"hello\");\n\
                        }\n";
        let result = parse_unified_diff(sample);
        assert_eq!(result.files.len(), 1);
        assert_eq!(result.stats.additions, 1);
    }
}
```

## Acceptance Criteria
- All five public functions implemented
- Uses `crate::git::diff::parse_unified_diff` for all diff parsing (no duplication)
- Subprocess calls use `-R <path>` flag for jj repo path
- Tests pass with `cargo test`
- File compiles (may need `mod jj;` in main.rs — see issue 02)
