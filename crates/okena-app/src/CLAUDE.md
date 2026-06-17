# okena-app — UI/App Layer

The desktop UI/app layer, extracted out of the `okena` binary so it compiles as
its own crate (the binary is now a thin entry point: `src/main.rs` +
`src/assets.rs` + `src/smoke_tests.rs`). Real code lives in `app/`, `views/`,
and `keybindings/`; the remaining subdirectories are thin re-export modules
(`pub use okena_*::*`).

`lib.rs` re-exports the lower-level crates so the moved code's `crate::...`
paths keep resolving:
- `pub use okena_remote_server as remote;` → `crate::remote`
- `pub use okena_app_core::{settings, workspace};` → `crate::settings` / `crate::workspace`
- `#[macro_use] mod macros;` keeps `impl_focusable!` exported at the crate root

## Module Structure

```
crates/okena-app/src/
├── lib.rs                # Crate root: shim re-exports + module declarations
├── macros.rs             # Shared macros (impl_focusable!)
├── action_dispatch.rs    # Action → workspace dispatch glue
├── logging.rs            # In-app log console (ring buffer + reloadable filter)
├── simple_root.rs        # Linux Wayland maximize workaround (cfg(target_os = "linux"))
├── soft_close.rs         # Confirm-before-close helpers
├── app/                  # Main app entity — real code (see app/CLAUDE.md)
├── views/                # UI views — real code (overlays, chrome, panels, components)
├── keybindings/          # Keyboard actions — real code (see keybindings/CLAUDE.md)
├── elements/             # Re-exports okena-views-terminal elements
├── terminal/             # Re-exports okena-terminal
├── git/                  # Re-exports okena-git + okena-views-git
├── theme/                # Re-exports okena-theme (+ desktop theme() helper)
├── ui/                   # Re-exports okena-ui
├── services/             # Re-exports okena-services
└── remote_client/        # Re-exports okena-remote-client
```

(`settings` / `workspace` / `remote` are re-export shims in `lib.rs`, not
directories — see above.)

## Architecture

### GPUI Entities

Observable state with auto-notify:
- `Workspace` — projects, layouts, focus (via FocusManager)
- `RequestBroker` — decoupled transient UI request routing (overlay/sidebar requests)
- `SettingsState` — user preferences with debounced auto-save
- `AppTheme` — current theme mode and colors
- `WindowView` — per-window view, owns SidebarController + OverlayManager
- `OverlayManager` — centralized modal overlay lifecycle

### Event Flow

1. **PTY events**: `PtyManager` → `async_channel` → `Okena` → `Terminal` (+ `PtyBroadcaster` for remote clients)
2. **UI requests**: `RequestBroker` → `cx.notify()` → observers in WindowView/Sidebar
3. **State mutations**: `Workspace` notify → observers update UI
4. **Persistence**: debounced 500ms save to disk

### Configuration Files

Located in the platform config dir (macOS: `~/Library/Application Support/okena/`, Linux: `~/.config/okena/`):
- `workspace.json` — projects, layouts, terminal state
- `settings.json` — font, theme, shell, session backend
- `keybindings.json` — custom keyboard shortcuts
- `themes/*.json` — custom theme files
- `remote.json` — remote server discovery (auto-generated)

## Testing

Tests live in `#[cfg(test)]` modules inside source files. Run with `cargo test`.

Every implementation plan should include a section on which tests to add, update, or delete. Identify the functions that contain real logic worth testing (see rules below) and list concrete test cases. If the change only touches trivial code (simple setters, UI wiring), explicitly state that no tests are needed and why.

### What to test

- Branching logic, conditional behavior (if/match with multiple arms)
- Recursive or iterative algorithms (tree traversal, normalization, flattening)
- Multi-step state mutations where ordering matters
- Edge cases and boundary conditions (empty input, out-of-bounds, overflow)
- Index arithmetic (reorder, move, insert-at-position, active_tab adjustment after removal)
- Data validation and migration (corrupt input recovery, version upgrades)
- Focus stack management (push/pop/restore with context switching)
- Serialization round-trips for complex nested structures

### What NOT to test

- Trivial getters/setters, bool toggles, simple renames
- HashMap/Vec lookups, counter increments
- Redundant simulation tests — if a `#[gpui::test]` tests the real method, don't also write a pure test with a `simulate_*` helper that duplicates the same logic

### GPUI test setup

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
