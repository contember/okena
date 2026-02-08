---
description: Rebase current branch onto a target, resolving conflicts correctly
allowed-tools: Bash(git:*), Bash(cargo check), Bash(cargo build), Read, Edit, Glob, Grep
---

# Rebase: $ARGUMENTS

If `$ARGUMENTS` is empty, check for a rebase already in progress (`git status`). If no rebase is in progress and no target was given, default to `origin/main`.

## Procedure

### 1. Pre-flight

- Ensure working tree is clean (`git status`). Stash or abort if dirty.
- Fetch latest (`git fetch origin`).
- Start the rebase: `git rebase <target>`.

### 2. Resolve conflicts commit-by-commit

For each conflicting commit, follow this loop:

1. Run `git status` and `git diff` to understand what conflicts exist.
2. Classify each conflict by type (see below) and resolve it.
3. After resolving all files in the commit, run `cargo check` to verify it compiles.
4. If it compiles: `git add -A && git rebase --continue`.
5. If it does NOT compile: fix the errors, then go to step 3.

### 3. Conflict resolution strategies

#### Import / use-statement merges
Both sides added different `use` items to the same block. **Combine both sets of imports.** Remove duplicates. Keep them sorted if the surrounding style is sorted.

#### Modify/delete (CRITICAL)
One side deleted or renamed a file; the other side modified it. This is the most dangerous conflict type.

- **Identify why the file was deleted.** Usually it was refactored/split into submodules.
- **Port the incoming changes** from the deleted file into whatever new file(s) replaced it. Read both the old content (from the conflict) and the new module structure to find where each change belongs.
- **Never just `git rm` the file and move on** — that silently drops the modifications.
- After porting, mark resolved: `git add <new files> && git rm <old file>`.

#### Add/add on version or generated files
Both sides changed `Cargo.toml` version, `Cargo.lock`, or similar. **Keep the HEAD (ours) version** for version numbers. For `Cargo.lock`, see the dedicated section below.

#### Content conflicts (both sides edited the same region)
Read the surrounding context to understand intent. Merge logically — don't just pick one side. If one side intentionally removed code (e.g., removed a workaround), keep it removed even if the other side still has it.

#### Removed code on one side
If HEAD intentionally removed a block (e.g., an ad-hoc workaround, deprecated feature), and the incoming branch still has it, **keep it removed**. Verify by reading the commit message that removed it.

### 4. Cargo.lock handling

**Never manually edit Cargo.lock.** When it conflicts:

```
git checkout HEAD -- Cargo.lock    # or: git checkout --theirs Cargo.lock
cargo check                        # regenerates the lock file
git add Cargo.lock
```

### 5. Post-rebase verification

After `git rebase --continue` reports success:

1. `cargo check` — must compile cleanly.
2. `git log --oneline -20` — review the rebase result, ensure commits look correct.
3. Report the result: how many commits were rebased, any notable conflict resolutions.

## Notes

- If the rebase becomes too tangled, `git rebase --abort` is always safe.
- Never force-push without explicit user confirmation.
- When unsure about a conflict resolution, ask the user before proceeding.
