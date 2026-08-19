---
id: 03
title: Daemon/client parity follow-ups
blocked-by: []
---

# 03 — Daemon/client parity follow-ups

**Summary.** Last-mile parity gaps left over from the headless two-process
migration (see [ADR-0001](../decisions/0001-headless-two-process-daemon.md) and
the archived [migration record](../archive/headless-migration.md)). None block the
architecture; each needs a **running** app to validate, which is why they outlived
the migration.

## Problem

The migration's own follow-up list was written at the end of Phase D and never
re-checked against HEAD. Re-verified 2026-08-19 — status per item below.

### Still open (verified at HEAD)

1. **Per-window presentation restore across a client restart.**
   `sidebar_open` / `folder_filter` / `os_bounds` / panel heights are
   client-owned presentation, but the client no longer loads `workspace.json`
   itself, so nothing restores them. The daemon stubs them out:
   `crates/okena-daemon-core/src/command_loop.rs:3798` (`folder_filter: None`,
   `sidebar_open: None`). Decide client-side persistence vs. a daemon round-trip.

2. **Claude env live refresh.** `CLAUDE_CONFIG_DIR` is resolved once at daemon
   startup — `PtyManager::set_extra_env` is called exactly once
   (`crates/okena-daemon-core/src/daemon.rs:339`) and nothing re-calls it from
   the settings-update path. Changing `claude-code.config_dir` at runtime does
   not reach live PTYs.

### Flagged, not re-verified (need a live session)

3. **Terminal scrollback on (re)attach.** The SNAPSHOT frame replays the
   viewport, not history — a pre-existing remote-protocol limitation affecting
   every remote client. Matters on reconnect to an existing daemon session; the
   primary "create terminals live" flow is unaffected.

4. **`worktree_removed` hook on the daemon's removal path.** The daemon removes
   the worktree via `remove_worktree_project_off_reactor_with`
   (`crates/okena-daemon-core/src/command_loop.rs:1399`); confirm it fires
   `worktree_removed` and supports background removal, or add both so the daemon
   and `RemoveWorktreeProject` entry points share them.

### Closed since the list was written — do not re-do

- **`HookMonitor` run status into `StateResponse`** — done.
  `StateResponse.hooks: Vec<ApiHookExecution>` (`crates/okena-core/src/api.rs:43`)
  carries hook execution history to thin clients.
- **Soft-close-on-quit flush** — a graceful shutdown path exists
  (`POST /v1/shutdown` → `shutdown_requested` Notify, drains terminal kills before
  the final save; `crates/okena-daemon-core/src/daemon.rs`), plus a soft-close
  poll (`crates/okena-daemon-core/src/soft_close.rs`). Re-check only if a
  soft-closed session is observed outliving the daemon.

## Approach / acceptance

Each item is independent — take them one at a time in a run-capable session.
Acceptance is behavioural, not a unit test: restart the client and see the
sidebar/filter come back (1); change `claude-code.config_dir` and see a new PTY
pick it up (2); reconnect to a live daemon session and see history (3); remove a
worktree through the daemon and see the hook fire (4).

## Touch points

- `crates/okena-daemon-core/src/{command_loop,daemon}.rs`
- `crates/okena-workspace/src/claude_env.rs`
- `crates/okena-core/src/{api,ws}.rs`
- `crates/okena-app/src/views/window/` (client-side presentation state)
