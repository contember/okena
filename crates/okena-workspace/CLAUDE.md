# okena-workspace — State Management & Persistence

Central state management for projects, layouts, focus, and cross-cutting UI requests.

## Key Types

- `Workspace` (GPUI entity in `state.rs`) — owns `WorkspaceData`, `ProjectData`, `LayoutNode`, `FolderData`. All project and layout state.
- `LayoutNode` — recursive enum: `Terminal(id)`, `Split { axis, children, ratios }`, `Tabs { children, active }`. Navigation via `Vec<usize>` path indexing.
- `FocusManager` (`focus.rs`) — bounded stack for focus restoration. Tracks focused project + terminal path.
- `RequestBroker` (`request_broker.rs`) — decoupled transient UI request routing. `VecDeque` queues drained by observers.
- `SettingsState` (`settings.rs`) — `AppSettings`, `HooksConfig`, `SidebarSettings` loaded from `settings.json`.

## Key Files

| File | Purpose |
|------|---------|
| `state.rs` | `Workspace` entity — all project/layout/folder state (~3900 lines) |
| `persistence.rs` | Load/save `workspace.json`. Validation, migration, layout normalization on load. |
| `settings.rs` | Settings schema types, debounced auto-save. |
| `hooks.rs` | Project lifecycle hooks — shell commands on project open/close, env var injection. |
| `sessions.rs` | Workspace export/import, named sessions. |
| `actions/` | Workspace mutations split by domain: project, folder, layout, terminal, focus. |

## Key Patterns

- **RequestBroker**: Decouples workspace actions from UI. Code that needs to show an overlay pushes a request; RootView observer picks it up. Avoids circular entity dependencies.
- **Folder model**: Folder IDs go into `project_order` alongside project IDs. Projects inside a folder live in `folder.project_ids`, NOT duplicated in `project_order`.
- **`#[serde(default)]`**: Used on new fields for backward-compatible workspace.json migration.
- **LayoutNode tree**: Recursive tree navigated via `Vec<usize>` path. Actions in `actions/layout.rs` for split, close, move, reorder.
