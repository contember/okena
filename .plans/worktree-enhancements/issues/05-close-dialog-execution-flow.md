# Issue 05: Implement enhanced execution flow in close dialog

**Priority:** high
**Files:** `src/views/overlays/close_worktree_dialog.rs`

Update the `execute()` method to handle the new stash/fetch/push/delete-branch steps. The existing flow is:
1. pre_merge hook → rebase → merge → post_merge hook
2. before_worktree_remove hook → remove → worktree_removed hook

## Enhanced Flow

Update the `execute()` method's async spawn to:

### Before merge (if merge enabled):
1. **Stash** (if `stash_enabled` and `is_dirty`):
   - Set state to `Stashing`
   - Call `git::stash_changes()` via `smol::unblock`
   - On error: show error, reset to Idle, return
   - Track `did_stash = true` for recovery

2. **Fetch** (if `fetch_enabled`):
   - Set state to `Fetching`
   - Call `git::fetch_all()` via `smol::unblock`
   - On error: if `did_stash`, call `git::stash_pop()`. Show error, reset to Idle, return

3. **pre_merge hook** (existing, unchanged)

4. **Rebase** (existing):
   - On error: if `did_stash`, call `git::stash_pop()`. Show error, reset to Idle, return

5. **Merge** (existing):
   - On error: if `did_stash`, call `git::stash_pop()`. Show error, reset to Idle, return

6. **post_merge hook** (existing, unchanged)

### After merge (if merge enabled):
7. **Push** (if `push_enabled`):
   - Set state to `Pushing`
   - Push the **default branch** (target branch that received the merge): `git::push_branch(&main_path, &default_branch)`
   - On error: log warning but continue (push failure shouldn't block worktree removal)

8. **Delete branch** (if `delete_branch_enabled`):
   - Set state to `DeletingBranch`
   - Delete local: `git::delete_local_branch(&main_path, &branch)` — log warning on error, continue
   - Delete remote: `git::delete_remote_branch(&main_path, &branch)` — log warning on error, continue
   - Branch deletion failures are non-fatal (branch may not exist on remote, etc.)

### Removal (unchanged):
9. before_worktree_remove hook
10. `ws.remove_worktree_project()` — force flag: `is_dirty && !did_stash` (if stash was used, working dir is clean)
11. worktree_removed hook

## Key Implementation Details

- Clone all needed variables before the async spawn (branch, default_branch, main_repo_path, etc.)
- Add `did_stash: bool` tracking variable inside the async block
- For stash recovery on error, fire-and-forget the `stash_pop` (log warning if it fails)
- Push and delete-branch errors are non-fatal: log with `log::warn!` and continue

Run `cargo build` to verify compilation.
