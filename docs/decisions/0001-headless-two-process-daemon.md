---
id: 0001
title: Split Okena into a headless daemon and thin clients
status: accepted
date: 2026-06-29
---

# 0001 — Split Okena into a headless daemon and thin clients

## Context

Okena had two parallel code paths for the same thing. A "local" path where the
GPUI desktop owned the `Workspace`, the PTYs, persistence and the git watcher
in-process; and a "remote" path where the same desktop mirrored *another*
instance's state over HTTP/WS. Every feature had to be built twice, and the
remote path was structurally the weaker of the two — gaps only surfaced when
someone actually used a remote project.

The seam that made a split tractable already existed: `okena-transport`'s
`RemoteClient` spoke a complete protocol (`/v1/pair`, `/v1/state`, `/v1/actions`,
WS `/v1/stream`), and `run_headless()` already ran the full stack windowless. The
question was not *whether* a protocol boundary could carry the app — the mobile
and web clients already proved it — but whether the desktop could live behind it
too, and whether the server side could shed GPUI entirely.

Shedding GPUI mattered beyond tidiness: a daemon that merely doesn't open a
window still *links* GPUI, so it cannot run on a headless server, CI box, or
container with no windowing stack.

## Decision

We will run Okena as **two processes**: a headless daemon that owns all
authoritative state, and thin clients that own only presentation.

- **The daemon owns DATA** — projects, layout as data, terminals and PTYs, git
  status, services, hooks, persistence, and the instance lock. It is the single
  writer of `workspace.json`.
- **Clients own PRESENTATION** — windows, focus, bounds, sidebar state, folder
  filter, column widths. Desktop, web, mobile and remote are all thin clients of
  the same protocol; none is privileged.
- The split rule is **data vs presentation, not local vs remote.** Local projects
  and remote projects render through identical machinery.
- **The daemon is GPUI-free**, not merely windowless. The falsifiable gate is
  `cargo tree -i gpui -p okena-daemon` returning nothing.
- The desktop **spawns its own local daemon** over loopback and the daemon dies
  with the last UI (UI-owned lifecycle). The same binary also runs standalone as
  a long-lived TLS server.

Sequencing was deliberately inverted from the obvious order: reach the
two-process architecture first against a *headless-GPUI* daemon, flip the default,
delete the in-process path — and only then strip GPUI out as a pure internal
refactor. Because clients speak only the protocol, the daemon's internals are
swappable behind their back. That made the risky half a two-way door: if
GPUI-free had hit a wall, the headless architecture was already banked.

## Consequences

**Easier.** One architecture, one code path. A feature built for the daemon works
for every client. The daemon runs on machines with no windowing stack. Protocol
gaps become visible immediately rather than only to remote users.

**Harder.** Everything the GUI wants must be representable on the wire — anything
the protocol can't carry is now a real gap, not a local shortcut. The desktop
pays a process-spawn and loopback round-trip at startup. Presentation state the
client no longer persists locally needs a new home (see
[backlog 03](../backlog/03-daemon-parity-follow-ups.md)).

**Now binding.** The GUI must not acquire the instance lock, must not write
`workspace.json`, and must not own PTY machinery. The `cargo tree -i gpui` gate
on `okena-daemon` must stay empty. New authoritative state goes in the daemon;
new view state stays in the client.

## Alternatives considered

- **Keep the dual local/remote paths.** Rejected: it was the source of the
  problem — every feature built twice, with the remote half chronically behind.
- **Windowless-but-GPUI daemon as the end state.** Rejected as the *goal* (it
  cannot run without a windowing stack) but adopted as the intermediate step,
  which is what made the migration safe.
- **GPUI-free extraction before the client flip** (the original phase order).
  Rejected: it front-loads the hardest, least reversible work before the
  architecture has proven itself. Doing it after the flip turned it into an
  internal refactor behind a stable protocol seam.

Full execution record, phase-by-phase commit map, and verification numbers:
[`../archive/headless-migration.md`](../archive/headless-migration.md).
