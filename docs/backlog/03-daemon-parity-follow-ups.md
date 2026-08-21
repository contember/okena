---
id: 03
title: Daemon/client parity follow-ups
blocked-by: []
---

# 03 — Daemon/client parity follow-ups

**Summary.** Two last-mile parity gaps remain from the headless two-process
migration (see [ADR-0001](../decisions/0001-headless-two-process-daemon.md) and
the archived [migration record](../archive/headless-migration.md)).

## Problem

The migration's own follow-up list was written at the end of Phase D and never
re-checked against HEAD. Re-verified 2026-08-21 — status per item below.

### Still open (verified at HEAD)

1. **Per-window presentation restore across a client restart.**
   `sidebar_open` / `folder_filter` / `os_bounds` / panel heights are
   client-owned presentation, but the client no longer loads `workspace.json`
   itself, so nothing restores them. The daemon stubs them out:
   `crates/okena-daemon-core/src/command_loop.rs:3798` (`folder_filter: None`,
   `sidebar_open: None`). Decide client-side persistence vs. a daemon round-trip.

### Flagged, not re-verified (need a live session)

2. **Terminal scrollback on (re)attach.** The SNAPSHOT frame replays the
   viewport, not history — a pre-existing remote-protocol limitation affecting
   every remote client. Matters on reconnect to an existing daemon session; the
   primary "create terminals live" flow is unaffected.

### Closed since the list was written — do not re-do

- **`HookMonitor` run status into `StateResponse`** — done.
  `StateResponse.hooks: Vec<ApiHookExecution>` (`crates/okena-core/src/api.rs:43`)
  carries hook execution history to thin clients.
- **Soft-close-on-quit flush** — a graceful shutdown path exists
  (`POST /v1/shutdown` → `shutdown_requested` Notify, drains terminal kills before
  the final save; `crates/okena-daemon-core/src/daemon.rs`), plus a soft-close
  poll (`crates/okena-daemon-core/src/soft_close.rs`). Re-check only if a
  soft-closed session is observed outliving the daemon.
- **Claude env live refresh** — done. A committed settings update recomputes the
  daemon backend's PTY environment, so newly created terminals use the current
  `claude-code.config_dir` without restarting the daemon.
- **`worktree_removed` hook on daemon removal paths** — done. Both direct and
  background removal converge on `Workspace::finish_worktree_removal`, which
  fires the hook after successful physical removal; regression tests cover both.

## Approach / acceptance

Each item is independent — take it in a run-capable session. Acceptance is
behavioural: restart the client and see the sidebar/filter come back (1);
reconnect to a live daemon session and see terminal history (2).

## Touch points

- `crates/okena-daemon-core/src/{command_loop,daemon}.rs`
- `crates/okena-workspace/src/claude_env.rs`
- `crates/okena-core/src/{api,ws}.rs`
- `crates/okena-app/src/views/window/` (client-side presentation state)
