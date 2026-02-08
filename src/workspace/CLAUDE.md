# workspace/ — State Management & Persistence

Central state management for projects, layouts, focus, and cross-cutting UI requests.

## Files

| File | Purpose |
|------|---------|
| `mod.rs` | Module re-exports. |
| `state.rs` | `Workspace` entity — `WorkspaceData`, `ProjectData`, `LayoutNode` (recursive tree: Terminal/Split/Tabs), `FolderData`. Owns all project and layout state. |
| `persistence.rs` | Load/save `workspace.json`. Validation, migration, consistency checks on load (normalize layouts, fix `project_order`). |
| `settings.rs` | `AppSettings`, `HooksConfig`, `SidebarSettings` — settings schema types loaded from `settings.json`. |
| `focus.rs` | `FocusManager` — bounded stack for focus restoration. Tracks focused project + terminal path. |
| `request_broker.rs` | `RequestBroker` entity — decoupled transient UI request routing. `VecDeque` queues for overlay and sidebar requests. Observers drain queues on notify. |
| `requests.rs` | `OverlayRequest` and `SidebarRequest` enum types (e.g., OpenSettings, ShowAddProject, RenameProject). |
| `hooks.rs` | Project lifecycle hooks — shell commands triggered on project open/close. Environment variable injection. |
| `sessions.rs` | Session management — workspace export/import, named sessions. |
| `actions/` | Workspace mutation methods split by domain: |

### actions/

| File | Purpose |
|------|---------|
| `mod.rs` | Re-exports action submodules. |
| `project.rs` | Add, remove, reorder projects. |
| `folder.rs` | Folder CRUD, move projects in/out of folders. |
| `layout.rs` | Split, close, move panes within the `LayoutNode` tree. |
| `terminal.rs` | Spawn, close, resize terminals within a project layout. |
| `focus.rs` | Focus navigation (next/prev terminal, project switching). |

## Key Patterns

- **LayoutNode tree**: Recursive enum — `Terminal(id)`, `Split { axis, children, ratios }`, `Tabs { children, active }`. Navigation via `Vec<usize>` path indexing.
- **RequestBroker**: Decouples workspace actions from UI. Code that needs to show an overlay pushes a request; the RootView observer picks it up. Avoids circular entity dependencies.
- **Folder model**: Folder IDs go into `project_order` alongside project IDs (UUIDs won't collide). Projects inside a folder live in `folder.project_ids`, NOT duplicated in `project_order`.
- **`#[serde(default)]`**: Used on new fields for backward-compatible workspace.json migration.
