# Issue 01: Add git stash and fetch operations

**Priority:** high
**Files:** `src/git/repository.rs`, `src/git/mod.rs`

Add 3 new git functions following the existing `command("git")` pattern in `repository.rs`:

1. **`stash_changes(path: &Path) -> Result<(), String>`**
   - Runs `git -C <path> stash`
   - Returns Ok on success, Err with stderr on failure

2. **`stash_pop(path: &Path) -> Result<(), String>`**
   - Runs `git -C <path> stash pop`
   - Returns Ok on success, Err with stderr on failure
   - Used for recovery when rebase/merge fails after stash

3. **`fetch_all(path: &Path) -> Result<(), String>`**
   - Runs `git -C <path> fetch --all`
   - Returns Ok on success, Err with stderr on failure

Add re-exports for all 3 functions in `src/git/mod.rs`.

Add tests in the existing `#[cfg(test)] mod tests` block:
- `stash_changes_returns_err_for_invalid_path`
- `stash_pop_returns_err_for_invalid_path`
- `fetch_all_returns_err_for_invalid_path`

Run `cargo build` and `cargo test` to verify.
