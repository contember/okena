# Split okena-git/repository.rs (1846-line god module)

- **Severity:** Medium (maintainability)
- **Type:** refactor
- **Area:** `okena-git`
- **Location:** `crates/okena-git/src/repository.rs` (whole file, 1846 lines)

## Problem

`repository.rs` is the single largest hand-written Rust file in the repo. It
interleaves at least five cohesive responsibilities:

- worktree ops (create/remove/list/clean)
- branch ops (list/checkout/create/delete/push)
- working-tree status & diff-stats
- CI/PR integration (`get_pr_info`, `parse_ci_checks`, `parse_branch_ci`,
  `compute_elapsed_ms` — ~330 lines, self-contained and pure, already unit-tested)
- path utilities (`normalize_path`, `compute_target_paths`, ...)

The `crates/okena-git/CLAUDE.md:5-13` "Files" table is also stale and no longer
describes this layout.

## Suggested fix

Split into `worktree.rs`, `branch.rs`, `status.rs`, `ci.rs`, `paths.rs`
submodules. The CI parsing block lifts out cleanly first. Update CLAUDE.md.
