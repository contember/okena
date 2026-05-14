# okena-workspace — Workspace coordinator (GPUI entity)

GPUI entity that wires together persistent data (`okena-state`), layout
algorithms (`okena-layout`), hook execution (`okena-hooks`), and persistence.

## Layered crates

| Crate | Role |
|-------|------|
| `okena-state` | Pure data: `WorkspaceData`, `ProjectData`, `FolderData`, `WorktreeMetadata`, `HookTerminalEntry`, `HooksConfig`, `Toast`. No GPUI. |
| `okena-layout` | `LayoutNode` recursive tree + tree algorithms (split/normalize/merge_visual_state). |
| `okena-hooks` | `HookRunner` (PTY) + `HookMonitor`. Decoupled from `okena-workspace` — receives metadata in, returns `HookTerminalResult`. |
| `okena-workspace` | This crate — `Workspace` entity, `actions/`, persistence, settings, sessions. |

`crate::state::*` re-exports the moved types so existing `use crate::state::X`
imports keep working. Same for `crate::settings::HooksConfig`,
`crate::hooks::*`, `crate::hook_monitor::*`, and `crate::toast::Toast`.

## Key Types

- `Workspace` (GPUI entity in `state.rs`) — coordinator over `WorkspaceData` from `okena-state`. Holds focus, lifecycle, remote-sync, access-history.
- `FocusManager` (`focus.rs`) — bounded stack for focus restoration. Tracks focused project + terminal path.
- `RequestBroker` (`request_broker.rs`) — decoupled transient UI request routing. `VecDeque` queues drained by observers.
- `SettingsState` (`settings.rs`) — `AppSettings`, `SidebarSettings` loaded from `settings.json`.

## Key Files

| File | Purpose |
|------|---------|
| `state.rs` | `Workspace` GPUI entity + tests (data types live in `okena-state`) |
| `persistence.rs` | Load/save `workspace.json`. Validation, migration, layout normalization on load. |
| `settings.rs` | `AppSettings` schema, debounced auto-save. Re-exports `HooksConfig` from `okena-state`. |
| `hooks.rs` / `hook_monitor.rs` | Re-exports the hook execution surface from `okena-hooks`. |
| `sessions.rs` | Workspace export/import, named sessions. |
| `actions/` | Workspace mutations split by domain: project, folder, layout, terminal, focus. |

## Key Patterns

- **RequestBroker**: Decouples workspace actions from UI. Code that needs to show an overlay pushes a request; WindowView observer picks it up. Avoids circular entity dependencies.
- **Folder model**: Folder IDs go into `project_order` alongside project IDs. Projects inside a folder live in `folder.project_ids`, NOT duplicated in `project_order`.
- **`#[serde(default)]`**: Used on new fields for backward-compatible workspace.json migration.
- **LayoutNode tree**: Recursive tree navigated via `Vec<usize>` path. Actions in `actions/layout.rs` for split, close, move, reorder.
