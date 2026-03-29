# Terminal Navigation

Okena provides multiple ways to navigate between terminal panes.

## Arrow Navigation

Move focus to an adjacent pane using spatial proximity.

| Action | Shortcut |
|--------|----------|
| FocusLeft | `Cmd+Alt+Left` |
| FocusRight | `Cmd+Alt+Right` |
| FocusUp | `Cmd+Alt+Up` |
| FocusDown | `Cmd+Alt+Down` |

On macOS `Cmd` is the Command key. On Linux `Cmd` maps to the Super key.

Arrow navigation uses weighted distance calculation between pane centers to find the nearest target in the specified direction.

## Sequential Navigation (Ctrl+Tab)

Cycle through panes in reading order (column-first: top to bottom within each project, then left to right across projects).

| Action | macOS | Linux/Windows |
|--------|-------|---------------|
| FocusNextTerminal | `Cmd+Shift+]` or `Ctrl+Tab` | `Ctrl+Tab` |
| FocusPrevTerminal | `Cmd+Shift+[` or `Ctrl+Shift+Tab` | `Ctrl+Shift+Tab` |

Wraps around when reaching the last/first pane.

## Pane Switcher (Number Overlay)

Instantly jump to any visible pane by its label.

| Action | macOS | Linux/Windows |
|--------|-------|---------------|
| TogglePaneSwitcher | `` Cmd+` `` | `` Ctrl+` `` |

How it works:

1. Press the shortcut to show numbered badges on each pane (`0-9`, then `a-z`)
2. Press any digit or letter to focus that pane
3. Press any other key or click to dismiss without switching

Supports up to 36 visible panes. Labels are assigned in reading order.

## Fullscreen (Terminal Zoom)

Expand a single terminal to fill its entire project column.

| Action | macOS | Linux/Windows | Description |
|--------|-------|---------------|-------------|
| ToggleFullscreen | `Shift+Escape` | `Shift+Escape` | Enter/exit fullscreen |
| FullscreenNextTerminal | `Cmd+]` | `Ctrl+]` | Next terminal in fullscreen |
| FullscreenPrevTerminal | `Cmd+[` | `Ctrl+[` | Previous terminal in fullscreen |

In fullscreen mode, a header bar appears with navigation buttons and the terminal name. The rest of the layout is hidden.

## Project Focus

Focus (zoom) to show only one project's column, hiding all others.

| Action | macOS | Linux/Windows | Description |
|--------|-------|---------------|-------------|
| FocusActiveProject | `Cmd+Shift+0` | `Ctrl+Shift+0` | Zoom to active project |
| ClearFocus | `Cmd+0` | `Ctrl+0` | Show all projects |
| ShowProjectSwitcher | `Cmd+E` | `Ctrl+E` | Search and focus a project |

## Pane Sorting (Reading Order)

The reading order for sequential navigation and the pane switcher follows this algorithm:

1. Group panes by project (visual columns)
2. Sort groups left-to-right by position
3. Within each group, sort top-to-bottom, then left-to-right
