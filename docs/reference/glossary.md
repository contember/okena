# Glossary

Domain terms used across Okena. Implementation lives in code; this is for shared language.

## Workspace

The unit of persisted state: all known projects, folders, and their layouts. One workspace per install (file: `workspace.json`).

## Project

A directory tracked by Okena. Displayed as a vertical **project column** containing the terminal tree for that directory. Identified by a stable `id`. May be local or remote.

## Worktree (project)

A project flagged as a git worktree of a **parent project**. Renders as a child column grouped under its parent. Created from the parent's git repo.

## Folder

A user-defined grouping of projects in the sidebar. Has its own ordering of child projects. Pure UX organization — does not affect persistence ownership.

## Layout

The split/tabs/terminals tree inside a single project column. Lives on `ProjectData.layout`.

## Window

A viewport onto the Workspace. Each window owns its own **view state**:

- folder filter (which folder is selected, if any)
- hidden project set (per-window show/hide overrides)
- focus zoom (which project, if any, is in single-project mode)
- column widths in the projects grid
- folder collapsed states in the sidebar
- OS bounds

Windows are not user-named; they are addressed positionally (main, then auto-numbered extras).

The underlying Workspace (projects, folders, ordering, layouts, hooks) is shared across windows. Sidebar renders in every window but reflects that window's filter state. Closing a window does not remove anything from the Workspace. Terminals, PTYs, git watchers, and settings are shared across all windows.

When a project is added from a given window, it becomes visible only in that window — hidden by default in all others.
