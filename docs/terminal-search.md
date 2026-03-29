# Terminal Search

Search within terminal output to find text in the scrollback history and visible screen.

## Usage

**Shortcut:** `Cmd+F` / `Ctrl+F` (when a terminal pane is focused)

The search bar appears at the top of the terminal pane with:

- Text input field
- Case sensitivity toggle (**Aa** button)
- Regex mode toggle (**.*** button)
- Match counter (e.g., "3 / 15")
- Previous / Next navigation buttons

## Navigation

| Key | Action |
|-----|--------|
| Enter | Jump to next match |
| Shift+Enter | Jump to previous match |
| Escape | Close search bar |

The terminal auto-scrolls to center the current match on screen.

## Search Modes

- **Literal** (default) -- plain text substring matching
- **Regex** -- full regular expression support (Rust `regex` syntax)

Both modes support case-sensitive and case-insensitive matching via the **Aa** toggle.

## Highlights

- **Current match** -- bright highlight with border
- **Other matches** -- dimmed background highlight

Highlights update automatically as new terminal output arrives.

## Scope

Search covers the entire scrollback buffer plus the visible screen. The scrollback size is controlled by the `scrollback_lines` setting (default: 10,000 lines, max: 100,000).
