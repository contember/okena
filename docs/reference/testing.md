# Testing

Repo-wide rules for Rust tests. Tests live in `#[cfg(test)]` modules inside
source files; run with `cargo test`, or `cargo test -p <crate>` for one crate.

Every implementation plan should say which tests to add, update, or delete.
Identify the functions with real logic worth testing (rules below) and list
concrete cases. If a change only touches trivial code (simple setters, UI
wiring), state explicitly that no tests are needed and why.

## What to test

- Branching logic, conditional behavior (if/match with multiple arms)
- Recursive or iterative algorithms (tree traversal, normalization, flattening)
- Multi-step state mutations where ordering matters
- Edge cases and boundary conditions (empty input, out-of-bounds, overflow)
- Index arithmetic (reorder, move, insert-at-position, active_tab adjustment after removal)
- Data validation and migration (corrupt input recovery, version upgrades)
- Focus stack management (push/pop/restore with context switching)
- Serialization round-trips for complex nested structures

## What NOT to test

- Trivial getters/setters, bool toggles, simple renames
- HashMap/Vec lookups, counter increments
- Redundant simulation tests — if a `#[gpui::test]` tests the real method, don't also write a pure test with a `simulate_*` helper that duplicates the same logic

## GPUI test setup

- Use `#[gpui::test]` with `gpui` in `[dev-dependencies]` (feature `test-support`)
- Use `use gpui::AppContext as _;` for `cx.new()`
- Explicit closure types: `|ws: &mut Workspace, cx|`
- For tests calling `add_project`/`delete_project` (which fire hooks), initialize GlobalSettings first:
  ```rust
  fn init_test_settings(cx: &mut gpui::TestAppContext) {
      cx.update(|cx| {
          let entity = cx.new(|_cx| SettingsState::new(Default::default()));
          cx.set_global(GlobalSettings(entity));
      });
  }
  ```
- Files with `use gpui::*;` import gpui's `test` proc macro which shadows std `#[test]`. In `#[cfg(test)]` submodules, use specific imports instead of glob.
