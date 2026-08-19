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
├── soft_close.rs         # Soft-close + restart-daemon toast-action ids (grace logic lives on the daemon)
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

### This process owns no PTYs and no workspace state

The desktop is a **thin client** of a daemon it spawns (ADR-0001). It has no
`PtyManager`, no `RemoteServer`, no git watcher, and no workspace autosave — the
GUI's `Workspace` is a pure mirror fed by `apply_remote_snapshot`. Do not add a
local write path for anything daemon-owned; add the action to the daemon and
mirror it back.

### Event Flow

1. **Terminal output**: daemon PTY → WS stream → `RemoteManager` → activity pump → `Terminal` → pane notify
2. **UI requests**: `RequestBroker` → `cx.notify()` → observers in WindowView/Sidebar
3. **State mutations**: action → daemon → snapshot → `apply_remote_snapshot` → `Workspace` notify → observers update UI
4. **Persistence**: only `window-layout.json` (client-owned, debounced). Everything else is written by the daemon.

### Configuration Files

In the platform config dir (macOS: `~/Library/Application Support/okena/`, Linux: `~/.config/okena/`):

| File | Written by |
|------|------------|
| `window-layout.json` — window placement, per-window presentation | **this process** |
| `workspace.json` — projects, layouts, terminal state | daemon |
| `settings.json` — font, theme, shell, session backend | daemon |
| `keybindings.json` — custom keyboard shortcuts | user / daemon |
| `themes/*.json` — custom theme files | user / daemon |
| `remote.json` — daemon discovery (port + auth) | daemon (auto-generated) |

## Testing

Tests live in `#[cfg(test)]` modules inside source files. Run with `cargo test`.

Test-selection rules and the GPUI test harness setup are repo-wide — see
[`docs/reference/testing.md`](../../../docs/reference/testing.md).
