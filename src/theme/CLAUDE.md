# theme/ — Theming System

Built-in and custom themes with live preview support.

## Files

| File | Purpose |
|------|---------|
| `mod.rs` | `AppTheme` entity — current theme mode and resolved colors. System appearance detection. Live preview (temporary theme without persisting). |
| `types.rs` | `ThemeMode` enum (6 modes: Dark, Light, Monokai, Solarized, Nord, Custom). `FolderColor` enum (8 colors for sidebar folders). |
| `colors.rs` | `ThemeColors` struct (~50 color fields). 4 built-in theme color constants (dark, light, monokai, nord). |
| `custom.rs` | Custom theme loading from `~/.config/okena/themes/*.json` files. Partial theme overrides (unset fields fall back to base theme). |

## Key Patterns

- **`AppTheme` entity**: GPUI entity that notifies on theme change, causing a full UI repaint.
- **Live preview**: Theme selector temporarily applies a theme via `AppTheme` without saving to settings. Reverts on cancel.
- **Partial custom themes**: Custom JSON themes only need to specify the colors they want to override; missing fields fall back to the closest built-in theme.
