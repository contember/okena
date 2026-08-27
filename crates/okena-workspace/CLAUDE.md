# okena-workspace — Workspace coordinator

Wires together persistent data (`okena-state`), layout algorithms
(`okena-layout`), hook execution (`okena-hooks`), and persistence.

**Reactor-agnostic.** `gpui` is an optional feature (`default = ["gpui"]`). The
action/state layer takes `&mut impl WorkspaceCx` (`context.rs`) rather than
`&mut Context<Workspace>`, so the same code runs under GPUI in the desktop and
under a plain tokio reactor in the daemon. `Context<'_, Workspace>: WorkspaceCx`,
so GUI callers are unaffected.

**IMPORTANT:** new code in `actions/` must take `&mut impl WorkspaceCx`, never a
GPUI `Context` directly — a `Context` parameter silently drops this crate out of
the daemon build. If you need something GPUI-only, put it behind the feature.

## Layered crates

| Crate | Role |
|-------|------|
| `okena-state` | Pure data: `WorkspaceData`, `ProjectData`, `FolderData`, `WindowState`, `WorktreeMetadata`, `HookTerminalEntry`, `HooksConfig`, `Toast`. No GPUI. |
| `okena-layout` | `LayoutNode` recursive tree + tree algorithms (split/normalize/merge_visual_state). |
| `okena-hooks` | `HookRunner` (PTY) + `HookMonitor`. Decoupled from `okena-workspace` — receives metadata in, returns `HookTerminalResult`. |
| `okena-workspace` | This crate — `Workspace` coordinator, `actions/`, persistence, settings, sessions. |

`crate::state::*` re-exports the moved types so existing `use crate::state::X`
imports keep working. Same for `crate::settings::HooksConfig`,
`crate::hooks::*`, `crate::hook_monitor::*`, and `crate::toast::Toast`.

## Key Types

- `Workspace` (`state.rs`) — coordinator over `WorkspaceData` from `okena-state`. Holds focus, lifecycle, remote-sync, access-history. A GPUI entity in the desktop; an `Arc<Mutex<_>>` in the daemon.
- `WorkspaceCx` (`context.rs`) — the reactor trait (`notify` / `refresh_views` + hook accessors) that keeps the action layer GPUI-free.
- `FocusManager` (`focus.rs`) — bounded stack for focus restoration. Tracks focused project + terminal path.
- `RequestBroker` (`request_broker.rs`) — decoupled transient UI request routing. `VecDeque` queues drained by observers.
- `SettingsState` (`settings.rs`) — `AppSettings`, `SidebarSettings` loaded from `settings.json`.

## Key Files

| File | Purpose |
|------|---------|
| `state.rs` | The `Workspace` coordinator + tests (data types live in `okena-state`) |
| `persistence.rs` | Load/save `workspace.json`. Validation, migration, layout normalization on load. |
| `settings.rs` | `AppSettings` schema, debounced auto-save. Re-exports `HooksConfig` from `okena-state`. |
| `hooks.rs` / `hook_monitor.rs` | Re-exports the hook execution surface from `okena-hooks`. |
| `sessions.rs` | Workspace export/import, named sessions. |
| `actions/` | Workspace mutations split by domain: project, folder, layout, terminal, focus. All take `&mut impl WorkspaceCx`. |
| `context.rs` | `WorkspaceCx` — the reactor abstraction. Read this before touching `actions/`. |
| `remote_apply.rs` | `apply_remote_snapshot` — pure, GPUI-free reconciliation of a daemon snapshot into `WorkspaceData`. Unit-tested. |
| `visibility.rs` | `compute_visible_projects(&WindowState, …)` — pure per-window filtering. |
| `claude_env.rs` | Shared gpui-free `CLAUDE_CONFIG_DIR` / Claude PTY env resolution. |
| `remote_sync.rs` | Remote reconciliation helpers. |

## Key Patterns

- **Windows are viewports**: per-window state (hidden set, folder filter, sizes, collapse, bounds) lives on `WindowState` in `okena-state`, not on this entity. See ADR-0002.
- **RequestBroker**: Decouples workspace actions from UI. Code that needs to show an overlay pushes a request; WindowView observer picks it up. Avoids circular entity dependencies.
- **Folder model**: Folder IDs go into `project_order` alongside project IDs. Projects inside a folder live in `folder.project_ids`, NOT duplicated in `project_order`.
- **`#[serde(default)]`**: Used on new fields for backward-compatible workspace.json migration.
- **LayoutNode tree**: Recursive tree navigated via `Vec<usize>` path. Actions in `actions/layout.rs` for split, close, move, reorder.
