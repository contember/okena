# elements/ — Custom GPUI Elements

Custom low-level GPUI `Element` implementations for terminal grid rendering and layout resizing.

## Files

| File | Purpose |
|------|---------|
| `mod.rs` | Module re-exports. |
| `terminal_element.rs` | Custom `Element` impl for the terminal grid. Handles layout, painting cells, cursor, selection highlighting. |
| `terminal_rendering.rs` | `BatchedTextRun` for efficient text painting. ANSI-to-GPUI color conversion. Background/foreground color mapping. |
| `terminal_input.rs` | `InputHandler` trait impl — bridges GPUI input events to terminal key/char input. IME support. |
| `resize_handle.rs` | Split pane resize divider element — draggable handle between split panes. |

## Key Patterns

- **Element trait**: `terminal_element.rs` implements GPUI's `Element` trait directly (not `Render`) for pixel-level control over terminal grid painting.
- **Batched rendering**: Text runs with the same style are batched into `BatchedTextRun` to minimize draw calls.
- **Color mapping**: ANSI 256-color and true-color values are mapped to GPUI `Hsla` colors, respecting the current theme.
