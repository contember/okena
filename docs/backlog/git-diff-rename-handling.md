# Git diff parser mishandles renames

- **Severity:** Low (correctness, edge case)
- **Type:** bug
- **Area:** `okena-git`
- **Location:** `crates/okena-git/src/diff.rs:112-263`, `lib.rs:371-384`, `repository.rs:325`

## Problem

Two rename-handling gaps:

1. `parse_unified_diff` keys file paths only off `--- `/`+++ ` lines. A pure rename
   (100% similarity) emits `rename from`/`rename to` and *no* `---`/`+++` lines, so
   the `FileDiff` ends up with both paths `None` → `display_name()` returns
   `"unknown"`.
2. `get_diff_file_summary` (lib.rs:371) parses `git diff --numstat`, but renames are
   emitted as `0\t0\told => new` or `{a => b}/f`; the code stores the literal
   `"old => new"` arrow text as the path. Inconsistent treatment vs `get_diff_stats`
   in repository.rs:325.

## Suggested fix

Parse `rename from`/`rename to` (and `copy from`/`to`) headers in
`parse_unified_diff`. For the numstat call sites, pass `--no-renames` or detect and
normalize the ` => ` form consistently.
