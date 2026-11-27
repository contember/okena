# keybindings/ — Keyboard Actions & Configuration

Defines all keyboard actions, default bindings, and user-configurable keybinding overrides.

## Files

| File | Purpose |
|------|---------|
| `mod.rs` | Actions defined via `actions!()` macro. `register_keybindings()` binds defaults + user overrides to GPUI context. |
| `config.rs` | `KeybindingConfig` — loads `keybindings.json`, merges with defaults, conflict detection. |
| `types.rs` | `KeybindingEntry` — keystroke + action + context scope. Serialization types. |
| `descriptions.rs` | Human-readable action descriptions grouped by category (for keybindings help overlay). |
| `start_page.rs` | Shortcuts for Start Page command buttons, read from `start_page_buttons` in settings. Validated and bound on every keymap rebuild; the result is cached per button for the tooltip, help dialog, settings rows and dispatch. |

## Key Patterns

- **`actions!()` macro**: GPUI macro that generates action structs from names. Each action is a zero-sized type.
- **Context-scoped dispatch**: Bindings can be scoped to specific contexts (e.g., terminal-focused vs global).
- **User overrides**: `keybindings.json` in the platform config dir (macOS: `~/Library/Application Support/okena/`, Linux: `~/.config/okena/`) overrides are merged on top of defaults at startup.

## Start Page command shortcuts

- The assignment lives on the button in `settings.json`, not in `keybindings.json`.
  `observe_start_page_shortcuts` rebuilds the keymap when assignments change, including
  settings synced from the daemon.
- A shortcut that fails validation (bad syntax, duplicate, taken by an action or the
  first stroke of a chord) is not bound; it is logged and its Settings row shows the reason.
- The Keyboard Shortcuts recorder rejects an action binding whose first stroke is a
  registered Start Page shortcut.
- The recorder also rejects a Git Diff binding (`DiffViewer` context, or a global
  binding of a diff action) whose first stroke is a key Git Diff handles itself;
  `okena_views_git::diff_viewer::is_builtin_diff_key` is the single list.
- The binding's context excludes text inputs, so typing in a field never launches a command.

## Global scope and Windows key policy

- A binding without a context is global **inside Okena**, not an operating-system hotkey.
- Windows assignments must never use the Win modifier (`cmd`, `win`, and `super` in JSON),
  including any stroke of a chord. Keep the recorder rejection and the registration guard
  in `KeybindingEntry::is_supported_on` aligned; platform filtering alone is insufficient.
- Preserve existing JSON entries for editing, but leave unsupported Win assignments inactive
  on Windows. Explicit macOS Cmd and Linux Super bindings remain supported on those platforms.
- Persist only complete custom action overrides; omitted actions inherit defaults and an
  explicit empty array suppresses defaults. Use the same comparison for persistence and Custom badges.
- See [configuration reference](../../../../docs/reference/configuration.md#keybindingsjson)
  for the file format, global/contextual assignment and platform restrictions.

- Loading (startup, dialog open or reload) must never write or reformat keybindings.json.
  Remove redundant defaults only on an explicit save or confirmed **Clean keybindings**.
- Preserve the original Ctrl alternatives on macOS and Super alternatives on Linux.
  The Win-key prohibition is Windows-only; do not use platform filtering to drop
  existing macOS/Linux default assignments.
- Confirm **Clean keybindings** with the existing `gpui-component::AlertDialog`
  on all platforms. The component Root hosts the dialog layer; Linux keeps
  SimpleRoot's resize workaround inside it.
- Keep Cancel explicit, block background clicks, and save only from the positive
  callback. Do not restore the native Window::prompt / basic Linux fallback path.
- Label the launcher **Clean keybindings…** because it opens a confirmation;
  keep the final **Clean keybindings** action without an ellipsis.
