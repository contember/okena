# Okena

Cross-platform terminal multiplexer built with Rust and GPUI (from Zed editor).

## Git Rules

- **Never revert or discard changes you didn't make.** If you see unexpected modifications in the working tree (e.g. from worktrees, other branches, or manual edits), leave them alone. Only stage and commit your own work.

## Build Commands

```bash
cargo build
cargo run
cargo test
```

On Windows, build from **x64 Native Tools Command Prompt for VS 2022** to avoid link.exe PATH conflicts with Git for Windows.

## Project Structure

```
src/                        # Desktop app — main binary, GPUI views, app coordinator
crates/                     # Library crates (20 crates, see below)
mobile/                     # Mobile app (Flutter + Rust FFI)
web/                        # Web client (React + TypeScript + xterm.js)
assets/                     # Fonts, icons (assets/icons/*.svg referenced as icons/*.svg)
scripts/                    # Build & utility scripts
```

### Crate layout

Most logic lives in `crates/`. The `src/` modules are thin re-exports (`pub use okena_*::*`) so existing `use crate::` imports keep working.

| Crate | Purpose |
|-------|---------|
| `okena-state` | Pure data types: `WorkspaceData`, `ProjectData`, `FolderData`, `HooksConfig`, `Toast`. No GPUI. |
| `okena-layout` | `LayoutNode` recursive tree + algorithms (split/normalize/merge_visual_state) |
| `okena-hooks` | Lifecycle hook execution (`HookRunner`, `HookMonitor`). Decoupled from `okena-workspace`. |
| `okena-workspace` | `Workspace` GPUI entity, persistence, settings, sessions, action methods |
| `okena-terminal` | PTY management, shell config, session backends |
| `okena-git` | Git status, diff parsing, worktree operations |
| `okena-theme` | Theming system (built-in + custom themes) |
| `okena-ui` | Design tokens, shared UI utilities |
| `okena-files` | File search, file viewer, syntax highlighting |
| `okena-markdown` | Markdown parsing and rendering |
| `okena-views-terminal` | Terminal pane, layout container, split/tabs views |
| `okena-views-sidebar` | Sidebar, project list, folder list, drag-and-drop |
| `okena-views-git` | Diff viewer, worktree dialog, git status UI |
| `okena-views-remote` | Remote connection dialogs |
| `okena-views-services` | Service panel views |
| `okena-remote-client` | Remote client connection manager |
| `okena-services` | Docker Compose, port detection |
| `okena-extensions` | Extension system |
| `okena-ext-claude` | Claude AI extension |
| `okena-ext-codex` | Codex extension |
| `okena-ext-updater` | Self-update system |
| `okena-core` | Shared types, API client, key handling |

## Module-Specific Context

Read these when working in the corresponding areas:

- `src/CLAUDE.md` — Desktop app architecture, event flow, GPUI entity model, testing rules
- `src/app/CLAUDE.md` — Main app entity, PTY event loop, remote bridge
- `src/remote/CLAUDE.md` — Remote control server (HTTP/WS API)
- `src/keybindings/CLAUDE.md` — Keyboard actions, bindings config
- `crates/okena-workspace/CLAUDE.md` — State management, LayoutNode tree, persistence
- `crates/okena-terminal/CLAUDE.md` — PTY threading model, shell detection
- `crates/okena-git/CLAUDE.md` — Diff parsing, worktree operations
- `mobile/CLAUDE.md` — Flutter + Rust FFI mobile app
- `web/CLAUDE.md` — React web client
