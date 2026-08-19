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
src/                        # Thin `okena` binary entry point (main.rs, assets.rs, smoke_tests.rs)
crates/                     # All logic — 34 crates, see below
docs/                       # Project docs (see "Docs" at the bottom)
mobile/                     # Mobile app — React Native UI (mobile/rn) over the Rust core via uniffi (crates/okena-mobile-ffi)
web/                        # Web client (React + TypeScript + xterm.js)
assets/                     # Fonts, icons (assets/icons/*.svg referenced as icons/*.svg)
scripts/                    # Build & utility scripts
```

### Architecture in one paragraph

Okena runs as **two processes**: a GPUI-free daemon that owns all authoritative
state (projects, layouts, PTYs, git, services, hooks, persistence) and thin
clients that own only presentation. The desktop spawns its own local daemon over
loopback; web, mobile and remote clients speak the same protocol. Local and
remote projects render through identical machinery. See
[ADR-0001](docs/decisions/0001-headless-two-process-daemon.md).

### Crate layout

Everything lives in `crates/`; `src/` is only the binary entry point.

| Crate | Purpose |
|-------|---------|
| `okena-state` | Pure data types: `WorkspaceData`, `ProjectData`, `FolderData`, `WindowState`, `HooksConfig`, `Toast`. No GPUI. |
| `okena-layout` | `LayoutNode` recursive tree + algorithms (split/normalize/merge_visual_state) |
| `okena-hooks` | Lifecycle hook execution (`HookRunner`, `HookMonitor`). Decoupled from `okena-workspace`. |
| `okena-workspace` | `Workspace` entity, persistence, settings, sessions, action methods. Reactor-agnostic via `WorkspaceCx` (gpui optional). |
| `okena-terminal` | PTY management, shell config, session backends |
| `okena-git` | Git status, diff parsing, worktree operations |
| `okena-theme` | Theming system (built-in + custom themes) |
| `okena-ui` | Design tokens, shared UI utilities |
| `okena-files` | File search, file viewer, syntax highlighting |
| `okena-highlight` | syntect/tree-sitter syntax highlighting shared by the file viewer, diff viewer and markdown code blocks |
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
| `okena-ext-github` | GitHub status extension |
| `okena-ext-updater` | Self-update system |
| `okena-core` | Shared data types only (no networking): wire schema (`api`), WS message types (`ws`), profiles, theme colors, process bus, key handling. Depended on by every crate. |
| `okena-transport` | Networking/transport over the `okena-core` schema: async client engine (WS connection + TLS pinning, `client` feature) and blocking HTTP + `remote_action` (`blocking-http` feature). Holds the heavy optional deps (tokio/reqwest/tungstenite/rustls) split out of `okena-core`. |
| `okena-mobile-ffi` | uniffi FFI surface for the React Native mobile app (`mobile/rn`); self-contained ConnectionManager / TerminalHolder engine over `okena-core` |
| `okena-app` | Desktop UI/app layer: GPUI views, app coordinator, keybindings, action dispatch. The `okena` binary is a thin shell over this. |
| `okena-app-core` | Headless app-logic layer: global observable settings + action-execution glue over the workspace. No views. |
| `okena-daemon-core` | GPUI-free daemon core: tokio reactor impls, observer reactor, PTY loop, git poller, command loop, `DaemonCore`. |
| `okena-daemon` | Standalone GPUI-free daemon binary. Gate: `cargo tree -i gpui -p okena-daemon` must stay empty. |
| `okena-remote-server` | The HTTP/WS server itself: routes, auth/pairing, PTY broadcaster, daemon bridge, local-daemon discovery. |
| `okena-cli` | `okena <subcommand>` — controls a running instance over the remote HTTP API. Gated before GUI startup in `main.rs`. |
| `okena-tui` | Proof-of-concept terminal UI client for a running daemon. |
| `okena-usage` | Shared usage-bar UI + working-days logic behind the Claude and Codex usage widgets. |

## Module-Specific Context

Read these when working in the corresponding areas:

- `crates/okena-app/src/CLAUDE.md` — Desktop app architecture, event flow, GPUI entity model
- `crates/okena-app/src/app/CLAUDE.md` — Main app entity, daemon connection, detached windows
- `crates/okena-app/src/keybindings/CLAUDE.md` — Keyboard actions, bindings config
- `crates/okena-daemon-core/CLAUDE.md` — The daemon: reactor traits, LocalSet, gpui-free invariants
- `crates/okena-remote-server/src/CLAUDE.md` — Remote control server (HTTP/WS API)
- `crates/okena-cli/src/CLAUDE.md` — CLI subcommands over the remote API
- `crates/okena-workspace/CLAUDE.md` — State management, `WorkspaceCx`, LayoutNode tree, persistence
- `crates/okena-terminal/CLAUDE.md` — PTY threading model, shell detection
- `crates/okena-git/CLAUDE.md` — Diff parsing, worktree operations
- `mobile/rn/CLAUDE.md` — React Native mobile app (uniffi over `okena-mobile-ffi`)
- `web/CLAUDE.md` — React web client
- `docs/reference/testing.md` — Test-selection rules + GPUI test harness setup

<!-- AGENT-DOCS:POINTER (managed by the agent-docs skill — edit the body freely,
     keep the markers) -->
## Docs

Project docs live in [`docs/`](./docs/) and follow a fixed structure — start at
[`docs/CLAUDE.md`](./docs/CLAUDE.md) (the operating manual) and
[`docs/INDEX.md`](./docs/INDEX.md) (the map). In short:

- `docs/reference/` — how the system works now.
- `docs/decisions/` — ADRs (the *why*), immutable.
- `docs/backlog/` — decided work not yet scheduled · `docs/sprints/` — active
  work-plans · `docs/archive/` — shipped.
- `docs/ideas/` — proposals, no commitment.

Path is the status (no `status:` fields); when you finish or supersede something,
move/delete it per `docs/CLAUDE.md`. This file and the per-crate `CLAUDE.md`s
outrank `docs/reference/` where they overlap.
<!-- /AGENT-DOCS:POINTER -->
