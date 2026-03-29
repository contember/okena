# Sidebar

The sidebar provides an overview of all projects, folders, terminals, services, and worktrees.

## Toggle

| Action | macOS | Linux/Windows | Description |
|--------|-------|---------------|-------------|
| ToggleSidebar | `Cmd+B` | `Ctrl+B` | Show/hide the sidebar |
| ToggleSidebarAutoHide | `Cmd+Shift+B` | `Ctrl+Shift+B` | Toggle auto-hide mode |
| FocusSidebar | `Cmd+1` | `Ctrl+1` | Focus the sidebar for keyboard navigation |

### Auto-Hide

When auto-hide is enabled, the sidebar appears on hover and disappears when focus leaves it. Toggle with `Cmd+Shift+B` / `Ctrl+Shift+B`.

### Settings

| Setting | Default | Description |
|---------|---------|-------------|
| `sidebar.is_open` | `false` | Whether the sidebar starts open |
| `sidebar.auto_hide` | `false` | Enable auto-hide mode |
| `sidebar.width` | `250` | Sidebar width in pixels (150-500) |

## Sidebar Contents

The sidebar shows, in order:

1. **Folders** -- collapsible groups of projects
2. **Top-level projects** -- projects not in any folder
3. **Expanded project details:**
   - **Terminals** -- terminal sessions within the project
   - **Services** -- Okena and Docker Compose services
   - **Hooks** -- running lifecycle hook terminals

Click a project to expand and see its terminals/services. Click again to collapse.

## Keyboard Navigation

When the sidebar is focused (`Cmd+1` / `Ctrl+1`):

| Key | Action |
|-----|--------|
| Up / Down | Navigate items |
| Enter | Select / activate item |
| Space, Left, Right | Toggle expand/collapse |
| Escape | Exit sidebar, restore previous focus |

## Drag and Drop

Reorder items by dragging:

- **Projects** -- drag to reorder within the sidebar or move into/out of folders
- **Folders** -- drag to reorder relative to other folders and projects
- **Worktrees** -- drag to reorder within their parent project

Drop a project onto a folder header to move it into that folder.

## Folder Colors

Both folders and projects support color indicators. Click the color dot to open the color picker with 16 color options:

Default, Red, Orange, Yellow, Lime, Green, Teal, Cyan, Blue, Indigo, Purple, Pink

- **Folder color** applies to the folder icon and all contained projects
- **Project color** can be set independently
- **Worktree color** inherits from parent, with optional override (reset via "Reset to parent")

Enable `color_tinted_background` in settings to tint project column backgrounds with the folder color.

## Context Menus

**Right-click a project** for options like rename, delete, create worktree, toggle visibility, and change color.

**Right-click a folder** for options like rename, delete, and filter to show only that folder's projects.

## Inline Rename

Double-click a project name, folder name, or terminal name in the sidebar to rename it inline.
