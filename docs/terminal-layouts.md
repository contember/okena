# Terminal Layouts

Okena supports flexible terminal layouts with splits, tabs, minimize, and detach.

## Splits

Split the focused terminal to create side-by-side or stacked panes.

| Action | macOS | Linux/Windows | Description |
|--------|-------|---------------|-------------|
| SplitVertical | `Cmd+D` | `Ctrl+Shift+D` | Split into left and right panes |
| SplitHorizontal | `Cmd+Shift+D` | `Ctrl+D` | Split into top and bottom panes |

Splits can be nested to any depth. Drag the divider between panes to resize (minimum 5% per pane).

### Equalize Layout

| Action | macOS | Linux/Windows |
|--------|-------|---------------|
| EqualizeLayout | `Cmd+Shift+E` | `Ctrl+Shift+E` |

Resets all split sizes to equal proportions.

## Tabs

Group multiple terminals into a tabbed view.

| Action | macOS | Linux/Windows | Description |
|--------|-------|---------------|-------------|
| AddTab | `Cmd+T` | `Ctrl+Shift+T` | Add a new tab to the current group |

Tab interactions:

- **Click** a tab to switch to it
- **Double-click** a tab to rename it
- **Middle-click** a tab to close it
- **Drag** a tab to reorder within the group or move to another tab group
- **Right-click** for context menu (move, close, etc.)

## Close

| Action | macOS | Linux/Windows |
|--------|-------|---------------|
| CloseTerminal | `Cmd+W` | `Ctrl+Shift+W` |

Closing a terminal in a split or tab group automatically collapses the parent if only one child remains.

## Minimize

| Action | macOS | Linux/Windows |
|--------|-------|---------------|
| MinimizeTerminal | `Cmd+M` | `Ctrl+Shift+M` |

Minimized terminals stay in the layout tree but are hidden from view, freeing space for other panes. Minimized terminals appear in the project column's hidden taskbar at the bottom. Click to restore.

## Detach

Detached terminals are removed from the visible split layout. Like minimized terminals, they remain connected to their session backend (tmux/dtach) and can be re-attached.

Detach is available via the terminal context menu (right-click). There is no default keybinding -- you can bind the `DetachTerminal` action in `keybindings.json` if needed.

## Layout Structure

Internally, layouts are a tree of three node types:

- **Terminal** -- a single terminal pane (with `minimized` and `detached` flags)
- **Split** -- horizontal or vertical split with percentage-based sizes
- **Tabs** -- tabbed container with an active tab index

The layout tree is persisted in `workspace.json` and restored on app restart.
