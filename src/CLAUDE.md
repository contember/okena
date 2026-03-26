# src/ — Desktop Application

The main binary. Most logic has been extracted into `crates/okena-*`; the `src/` subdirectories are thin re-export modules (`pub use okena_*::*`). Real code still lives in `src/app/`, `src/views/`, `src/remote/`, and `src/keybindings/`.

## Module Structure

```
src/
├── main.rs               # Entry point, GPUI setup, window creation
├── settings.rs           # Global settings entity (SettingsState, auto-save)
├── assets.rs             # Embedded fonts and icons
├── process.rs            # Cross-platform subprocess spawning
├── macros.rs             # Shared macros (impl_focusable!)
├── simple_root.rs        # Linux Wayland maximize workaround
├── app/                  # Main app entity — real code (see app/CLAUDE.md)
├── views/                # UI views — real code (overlays, chrome, panels, components)
├── keybindings/          # Keyboard actions — real code (see keybindings/CLAUDE.md)
├── remote/               # Remote server — real code (see remote/CLAUDE.md)
├── terminal/             # Re-exports okena-terminal
├── workspace/            # Re-exports okena-workspace (+ local actions/)
├── git/                  # Re-exports okena-git + okena-views-git
├── theme/                # Re-exports okena-theme
├── ui/                   # Re-exports okena-ui
├── elements/             # Re-exports okena-views-terminal elements
├── services/             # Re-exports okena-services
└── remote_client/        # Re-exports okena-remote-client
```

## Architecture

### GPUI Entities

Observable state with auto-notify:
- `Workspace` — projects, layouts, focus (via FocusManager)
- `RequestBroker` — decoupled transient UI request routing (overlay/sidebar requests)
- `SettingsState` — user preferences with debounced auto-save
- `AppTheme` — current theme mode and colors
- `RootView` — main view, owns SidebarController + OverlayManager
- `OverlayManager` — centralized modal overlay lifecycle

### Event Flow

1. **PTY events**: `PtyManager` → `async_channel` → `Okena` → `Terminal` (+ `PtyBroadcaster` for remote clients)
2. **UI requests**: `RequestBroker` → `cx.notify()` → observers in RootView/Sidebar
3. **State mutations**: `Workspace` notify → observers update UI
4. **Persistence**: debounced 500ms save to disk

### Configuration Files

Located in `~/.config/okena/`:
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
