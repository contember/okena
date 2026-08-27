# okena-daemon-core — GPUI-free daemon core

The authoritative half of the two-process architecture (ADR-0001). Runs the same
`okena-workspace` / `okena-services` code paths as the desktop, but driven by a
tokio reactor instead of GPUI's `Context`/`AsyncApp`.

## Critical invariants

- **NEVER add a `gpui` dependency to this crate or anything below it.** The gate
  is `cargo tree -i gpui -p okena-daemon` returning nothing. Enabling a gpui
  feature anywhere in the daemon graph breaks headless server/CI/container
  deployment, not just a build.
- **The daemon is the sole writer of `workspace.json`.** It takes
  `acquire_instance_lock()` as step 0 of `DaemonCore::new` — before binding a
  port or writing `remote.json` — and holds the `LockGuard` for its lifetime.
  Clients never write it.
- **Reactor tasks need the `LocalSet`.** Observers, `run_pty_loop`, service
  restarts, and the service arms of `daemon_command_loop` use `spawn_local`.
  They are spawned inside `LocalSet::block_on`; blocking subprocess work reaches
  the multi-thread pool via the held `Handle`. A `spawn_local` outside that scope
  panics at runtime.
- **Observer notifies can storm.** A services diff can bump `service_tick`,
  which re-runs the diff. Keep diffs idempotent (`sync_services`' `known` set,
  `sync_service_terminals`' equality check) and drop locks before re-entering —
  see the guards documented at the top of `observers.rs`.

## Architecture

The GUI reacts to GPUI entity `notify`; the daemon holds the same state behind
`Arc<parking_lot::Mutex<…>>` and turns each notify into a `watch` channel bump
that observer tasks await.

`DaemonReactor` (`reactor.rs`) owns three independent watch channels:

| Channel | Bumped by | Consumed by |
|---|---|---|
| `state_version` | anything persistent changing | autosave / snapshot observer |
| `workspace_tick` | `WorkspaceCx::notify` | workspace observers |
| `service_tick` | the service reactor's `notify` | service sync |

The two reactor trait families are what make the logic crates reactor-agnostic:

- `okena_workspace::context::WorkspaceCx` → impl in `workspace_cx.rs`
- `okena_services::manager`'s `ServiceCx` / `ServiceHandle` / `ServiceAsyncCx` →
  impl in `service_cx.rs`

## Files

| File | Purpose |
|------|---------|
| `daemon.rs` | `DaemonCore::{new,run}` — assembles managers, git watcher, bridge, `RemoteServer`; drives the reactor on a `LocalSet` until shutdown. |
| `command_loop.rs` | `daemon_command_loop` — the gpui-free port of `remote_command_loop`. Handles `GetState` (builds `StateResponse`) and every action arm. Largest file in the crate. |
| `reactor.rs` | `DaemonReactor` shared state + `spawn_observers`. |
| `observers.rs` | The observer tasks: autosave, state snapshot, service sync. Re-entrancy guards live here. |
| `pty_loop.rs` | PTY event loop — drains `PtyEvent::Data`, hook-terminal exits, OSC hook-exit titles, activity bumps. |
| `git_poll.rs` | Background git-status poller. |
| `workspace_cx.rs` / `service_cx.rs` | Tokio impls of the reactor traits. |
| `daemon_config.rs` | Gpui-free settings/theme handlers. |
| `soft_close.rs` | Soft-close deadline poll (grace window before killing a session). |
| `toast_poll.rs` | Drains `HookMonitor` toasts onto the stream. |
| `worktree_close_watchdog.rs` | Watches pending worktree closes. |
| `worktree_stale_sweep.rs` | Drops worktree projects whose checkout was deleted outside Okena. |

## Patterns

- **Mirror, don't fork.** When the GUI gains behaviour that belongs to DATA
  (per ADR-0001's split rule), port it here rather than leaving a client-side
  copy — a GUI-only path silently stops running once the client is thin.
- **`StateResponse` parity.** Fields the daemon legitimately does not own are
  per-window presentation (`focused_project_id`, `bounds`, `sidebar_open`,
  `folder_filter`) and are stubbed `None` on purpose. Everything else must be
  populated — a stub there is a real gap. See `docs/backlog/03-daemon-parity-follow-ups.md`.
- **`parse_window_id`** is a deliberate gpui-free copy of the GUI's version.
  Keep the two in sync.

Related: `crates/okena-workspace/CLAUDE.md` (the logic being driven),
`crates/okena-remote-server/src/CLAUDE.md` (the protocol surface).
