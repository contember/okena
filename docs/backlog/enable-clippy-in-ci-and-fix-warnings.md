# Enable clippy in CI + fix 118 existing warnings

- **Severity:** Medium (hygiene)
- **Type:** hygiene / CI
- **Area:** workspace-wide
- **Location:** `.github/workflows/build.yml`; warnings in `okena-views-terminal` (74), `okena-views-git` (44)

## Problem

Clippy is not run in CI (`build.yml` has no clippy step; `rust-toolchain.toml` only
lists the component). As a result 118 warnings have accumulated, 91 of them
auto-fixable. Observed lints include `clone_on_copy` (e.g. `ThemeColors` which is
`Copy`), `let_unit_value` (the `let _ = cx.update(...)` pattern), `needless_return`,
and `collapsible_if`.

## Suggested fix

1. `cargo clippy --fix --lib -p okena-views-terminal` and
   `-p okena-views-git` to apply the 91 auto-fixable suggestions.
2. Review and fix the remaining ~27 manually.
3. Add a `cargo clippy --workspace --all-targets -- -D warnings` gate to
   `build.yml` to prevent regression.
