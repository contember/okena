---
model: claude-sonnet-4-6
---

# Issue 06: Verify build and run all tests

**Priority:** medium
**Files:** (none — verification only)

Final verification step. Ensure the entire project compiles and all tests pass.

## Steps

1. Run `cargo build` — must compile without errors or warnings (beyond pre-existing ones)
2. Run `cargo test` — all existing tests must pass, plus new tests in `src/jj/mod.rs` and `src/vcs.rs`
3. Run `cargo clippy` (if available) — no new warnings

## Fix any issues found

If compilation fails:
- Check for missing imports, incorrect paths, or type mismatches
- Ensure `pub` visibility is correct on functions that are now called cross-module
- Ensure `parse_unified_diff` is accessible from `src/jj/mod.rs` (may need to make it `pub` in `src/git/diff.rs` if not already)

If tests fail:
- Check that existing git tests haven't been broken by import changes
- Ensure new tests handle the case where `jj` binary is not installed (tests should not require jj to be installed)

## Acceptance Criteria
- `cargo build` succeeds
- `cargo test` passes all tests (existing + new)
- No new compiler warnings introduced
