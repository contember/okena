# Sessions

Okena supports named workspace sessions, letting you save and restore complete workspace states.

## What is a Session?

A session is a saved snapshot of your entire `WorkspaceData`:

- All projects and their configurations
- Terminal layout trees (splits, tabs, positions)
- Terminal custom names and minimized/detached state
- Project ordering and folder groupings
- Worktree metadata

Sessions are stored as JSON files in `~/.config/okena/sessions/`.

## Session Manager

**Shortcut:** `Cmd+K Cmd+W` / `Ctrl+K Ctrl+W`

The session manager has two tabs:

### Sessions Tab

- **Save** -- type a name and save the current workspace as a new session
- **Load** -- click a saved session to replace the current workspace with it
- **Rename** -- rename a saved session
- **Delete** -- delete a saved session (with confirmation)

Sessions are listed sorted by modification time (most recent first), with project count displayed.

### Export / Import Tab

- **Export** -- save the current workspace to an arbitrary file path (default: `~/workspace-export.json`)
- **Import** -- load a workspace from a file

Export wraps the workspace data in an `ExportedWorkspace` envelope with version and timestamp metadata. Import accepts both the envelope format and raw `WorkspaceData` JSON.

## Loading Behavior

Loading a session **replaces the entire workspace** -- all current projects and terminals are removed and replaced with the session's content.

- If the session backend supports persistence (tmux/dtach), terminals reconnect to their saved sessions
- If not (backend `None`), terminal IDs are cleared and new empty terminals are created
- Imported workspaces always clear terminal IDs (since imported sessions can't have live terminals)

The loaded workspace goes through migration and validation (layout normalization, folder consistency checks) before being applied.

## Active Session

The `active_session` field in `settings.json` tracks the name of the currently loaded session. When no session is active (`null`), the default `workspace.json` is used.
