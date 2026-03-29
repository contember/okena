# Projects

Okena organizes your work into **projects** -- vertical columns displayed side by side. Each project has its own terminal layout, git status, services, and settings.

## Project Columns

Projects are rendered as a horizontal grid of columns. Each column shows:

- **Header** with project name, folder color indicator, and git branch/status
- **Terminal layout** (splits, tabs, terminals)
- **Git header** for diff popover and commit log
- **Service panel** for Docker/Okena services
- **Hidden taskbar** showing minimized/detached terminals

### Column Resizing

Drag the divider between columns to resize. Column widths are persisted and normalized to percentages.

| Setting | Default | Description |
|---------|---------|-------------|
| `min_column_width` | `400` | Minimum pixel width per project column |
| `color_tinted_background` | `false` | Tint project backgrounds with folder color |

## Creating Projects

- **From sidebar:** Click the **+** button to open the Add Project dialog
- **From command palette:** `Cmd+Shift+P` / `Ctrl+Shift+P`, then search for "NewProject"

The `NewProject` action has no default keybinding but can be bound in `keybindings.json`.

Each project has a name, filesystem path, and optionally starts with an initial terminal.

## Project Switching

**Shortcut:** `Cmd+E` / `Ctrl+E`

The project switcher dialog lets you:

- **Search** projects by name or path (fuzzy matching)
- **Enter** to focus a project (zooms to fullscreen view showing only that column)
- **Space** to toggle project visibility (show/hide in the overview grid)
- **Esc** to close

Projects are sorted by most recently used.

### Focus and Zoom

| Action | macOS | Linux/Windows | Description |
|--------|-------|---------------|-------------|
| ShowProjectSwitcher | `Cmd+E` | `Ctrl+E` | Open project switcher |
| FocusActiveProject | `Cmd+Shift+0` | `Ctrl+Shift+0` | Zoom to the project containing the active terminal |
| ClearFocus | `Cmd+0` | `Ctrl+0` | Exit zoom, show all projects |

When a project is focused (zoomed), only that column is visible in fullscreen. Press `Cmd+0` / `Ctrl+0` to return to the multi-column view.

## Project Visibility

Projects can be hidden from the overview grid without deleting them. Toggle visibility via:

- **Project switcher:** Press Space on a project
- **Sidebar context menu:** Right-click a project

Hidden projects retain their terminal sessions and settings.

## Folders

Projects can be organized into collapsible folders in the sidebar:

- **Create folder:** Right-click in the sidebar, select "New Folder"
- **Move project to folder:** Drag a project onto a folder header
- **Collapse/expand:** Click the folder chevron
- **Folder color:** Click the color dot on the folder to choose from 16 colors
- **Filter:** Right-click a folder to show only its projects

Folder ordering and project ordering within folders is controlled by drag-and-drop.

## Per-Project Settings

Each project can override:

- **Default shell** -- different shell per project
- **Hooks** -- project-specific lifecycle hooks
- **Folder color** -- visual color indicator (propagates to worktree children)

These are managed through the project settings UI and stored in `workspace.json`.
