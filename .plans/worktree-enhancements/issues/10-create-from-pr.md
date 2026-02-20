# Issue 10: Add "From PR" mode to worktree creation dialog

**Priority:** medium
**Files:** `src/views/overlays/worktree_dialog.rs`

Add a mode to the worktree creation dialog that lists open PRs from GitHub, letting users create a worktree directly from a PR's branch.

## New types

```rust
#[derive(Clone, Debug)]
struct PrInfo {
    number: u32,
    title: String,
    branch: String,
}
```

## New fields on `WorktreeDialog`

```rust
pr_mode: bool,           // toggle between branch list and PR list
pr_list: Vec<PrInfo>,    // loaded PRs
loading_prs: bool,       // true while fetching
pr_error: Option<String>, // error message if gh fails
```

## Constructor changes

In `new()`, initialize `pr_mode: false`, `pr_list: vec![]`, `loading_prs: false`, `pr_error: None`.

## PR loading

Add a method `load_prs(&mut self, cx: &mut Context<Self>)` that:
1. Sets `loading_prs = true`, `cx.notify()`
2. Spawns `cx.spawn(async move |this, cx| { ... })`:
   - Uses `smol::unblock` to run: `command("gh").args(["pr", "list", "--json", "number,title,headRefName", "--limit", "20"]).current_dir(&project_path).output()`
   - On success: parse JSON output as `Vec<{number, title, headRefName}>`, map to `PrInfo`
   - On error (gh not found, not a GitHub repo): set `pr_error = Some("GitHub CLI not found or not a GitHub repository")`
   - Update `this` with results, set `loading_prs = false`, `cx.notify()`

Call `load_prs()` when user toggles PR mode on (not in constructor, to avoid unnecessary gh calls).

## UI changes

### Mode toggle
Add two tab-like buttons above the search input / PR list:
- "Branches" (active when `!pr_mode`)
- "From PR" (active when `pr_mode`)

Clicking toggles `pr_mode`. When switching to PR mode for the first time, call `load_prs()`.

### PR list (when `pr_mode`)
Replace the branch list with a PR list:
- If `loading_prs`: show "Loading PRs..." text
- If `pr_error`: show error message in muted text
- Otherwise: render each PR as a row:
  ```
  #123  Fix authentication bug
        feature/auth-fix
  ```
  - PR number in muted text, title in primary text
  - Branch name below in smaller muted text
  - Clicking a PR sets `selected_branch_index` to match the PR's branch in the branches list (or store the branch directly)

### Branch selection from PR
When a PR is clicked:
- Set a field like `selected_pr_branch: Option<String>` with the PR's `branch`
- In `create_worktree()`, if `pr_mode` and `selected_pr_branch` is set, use that branch name (treat as existing branch, `create_branch = false`)
- The branch should exist on remote; if it's a remote-only branch, the `git worktree add` command should fetch it automatically

## Graceful fallback
- If `gh` CLI is not installed: show a friendly message, don't crash
- The "Branches" tab always works regardless of gh availability

Run `cargo build` to verify compilation.
