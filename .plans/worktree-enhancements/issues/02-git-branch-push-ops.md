# Issue 02: Add git branch delete, push, and unpushed commit operations

**Priority:** high
**Files:** `src/git/repository.rs`, `src/git/mod.rs`

Add 5 new git functions following the existing `command("git")` pattern in `repository.rs`:

1. **`delete_local_branch(repo_path: &Path, branch: &str) -> Result<(), String>`**
   - Runs `git -C <path> branch -d <branch>`
   - Uses `-d` (not `-D`) so it fails if branch has unmerged changes

2. **`delete_remote_branch(repo_path: &Path, branch: &str) -> Result<(), String>`**
   - Runs `git -C <path> push origin --delete <branch>`
   - Returns Ok on success, Err with stderr on failure

3. **`push_branch(repo_path: &Path, branch: &str) -> Result<(), String>`**
   - Runs `git -C <path> push origin <branch>`
   - Returns Ok on success, Err with stderr on failure

4. **`count_unpushed_commits(path: &Path) -> usize`**
   - Runs `git -C <path> rev-list @{u}..HEAD --count`
   - Returns the count as usize, or 0 on any error (no upstream, not a git repo, etc.)
   - Parse stdout as usize, default 0

5. **`list_git_worktrees(repo_path: &Path) -> Vec<(String, String)>`**
   - Runs `git -C <path> worktree list --porcelain`
   - Parses output: each worktree block has `worktree <path>` and `branch refs/heads/<name>` lines
   - Returns vec of (path, branch_name) pairs
   - Returns empty vec on error

Add re-exports for all 5 functions in `src/git/mod.rs`.

Add tests in the existing `#[cfg(test)] mod tests` block:
- `delete_local_branch_returns_err_for_invalid_path`
- `delete_remote_branch_returns_err_for_invalid_path`
- `push_branch_returns_err_for_invalid_path`
- `count_unpushed_commits_returns_zero_for_invalid_path`
- `list_git_worktrees_returns_empty_for_invalid_path`

Run `cargo build` and `cargo test` to verify.
