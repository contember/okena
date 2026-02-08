# keybindings/ — Keyboard Actions & Configuration

Defines all keyboard actions, default bindings, and user-configurable keybinding overrides.

## Files

| File | Purpose |
|------|---------|
| `mod.rs` | ~40 actions defined via `actions!()` macro. `register_keybindings()` binds defaults + user overrides to GPUI context. |
| `config.rs` | `KeybindingConfig` — loads `keybindings.json`, merges with defaults, conflict detection. |
| `types.rs` | `KeybindingEntry` — keystroke + action + context scope. Serialization types. |
| `descriptions.rs` | Human-readable action descriptions grouped by category (for keybindings help overlay). |

## Key Patterns

- **`actions!()` macro**: GPUI macro that generates action structs from names. Each action is a zero-sized type.
- **Context-scoped dispatch**: Bindings can be scoped to specific contexts (e.g., terminal-focused vs global).
- **User overrides**: `~/.config/okena/keybindings.json` overrides are merged on top of defaults at startup.
