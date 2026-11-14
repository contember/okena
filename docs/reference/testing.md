# Testing

Repo-wide rules for Rust tests. Tests live in `#[cfg(test)]` modules inside
source files; run with `cargo test --workspace`, or `cargo test -p <crate>` for
one crate. A bare `cargo test` runs the root package alone — the workspace
declares no `default-members`.

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


## Manual terminal-title refresh test (Windows)

From the repository root, run the [PowerShell 7 helper](test-terminal-title.ps1)
in an Okena terminal:

```powershell
pwsh -NoProfile -File ./docs/reference/test-terminal-title.ps1
```

It animates a leading icon and frame counter every 150 ms until **Ctrl+C**, without
ordinary terminal output. Optional flags:

- `-IntervalMs 50`: faster updates to exercise repaint coalescing.
- `-DurationSeconds 10`: stop automatically; the default `0` runs indefinitely.
- `-Mode Console`: use the Windows console-title API instead of OSC 0.
- `-WithOutput`: also print frames to compare title-only and content updates.

Watch the sidebar and terminal headers while switching tabs and OS windows. Check
background tabs, nested tabs/splits and detached windows, including the native
window title when enabled. Leave the test running without clicking the terminal:
received titles should keep updating. In regular Okena windows without OS focus,
tab/pane headers update at up to 10 FPS while the cap is on; the sidebar is
throttled to about 4 FPS regardless. Detached-window headers and native window
titles are not paced by these limits. The
last update must still appear after output stops. These caps are presentation
limits, not a guarantee that every animation frame will be displayed.

Cleanup restores the saved console API title and output encoding. An earlier
OSC-only title cannot be queried reliably, so use a disposable test terminal if
preserving its exact original label matters.

### psmux prerequisite

Okena can only display titles that the session backend forwards. psmux's separate
unsolicited title-delivery bug is fixed by
[psmux PR #1047](https://github.com/psmux/psmux/pull/1047), incorporated into master
as [c22e60aa](https://github.com/psmux/psmux/commit/c22e60aad3cb1269d4acce57261954ee20e33d88).
The latest release checked on 2026-11-14, **v3.4.0**, does not contain that fix.
Build psmux from that commit (or a descendant containing it), or wait for and
install an official release that includes it. This Okena change alone does not
fix missing title delivery from an older psmux server.

Enable title forwarding in `%USERPROFILE%\.psmux.conf`:

```tmux
set -g allow-set-title on
set -g set-titles on
set -g set-titles-string "#{pane_title}"
```

Verify which executable is selected with `Get-Command psmux -All` and
`psmux --version`. Existing servers keep running their old code after replacing
the executable or restarting Okena. Finish/save their work before restarting the
affected psmux sessions with the corrected binary: stopping servers terminates
shells and agents inside them.
