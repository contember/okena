---
id: 0003
title: Separate durable missions from runtime and shared attention
status: accepted
date: 2026-10-02
---

# 0003 — Separate durable missions from runtime and shared attention

Accepted records design approval, not shipped implementation. The
[implementation sprint](../sprints/sprint-2026-10-02-missions-and-attention.md)
tracks delivery and verification.

## Context

One piece of work can span repositories, worktrees and several agent
conversations. A project, conversation or live terminal alone cannot identify
that work durably. A finished agent turn also does not mean that the work is
complete, or that a person has read its result.

[RFC #189](https://github.com/contember/okena/issues/189) proposed mission-centric
work. The approved sprint narrows its first delivery to desktop and daemon, with
shared read acknowledgment. It preserves the ownership boundary established by
[ADR-0001](0001-headless-two-process-daemon.md): the daemon owns authoritative
state, while clients own presentation.

## Decision

We will give a mission its own durable identity, independent of project,
conversation/session and PTY identity. Missions are optional groupings of work;
the attention inbox also serves agents outside missions. A title identifies the
mission to the user. Goal and home-project context are optional, and creation
requires no ticket, new branch, worktree or agent launch.

- **Daemon authority.** The daemon owns missions, membership, attention episodes
  and shared read state. Clients submit mutations through the existing daemon
  action pipeline and keep only selection and presentation locally.
- **Project terminal ownership.** Projects retain their layouts and PTYs within
  the daemon. Missions reference work rather than taking ownership of terminals.
  Durable conversation membership uses the existing harness/session identity.
  An explicit binding before conversation identity is known belongs on the layout
  leaf, not on a replaceable PTY ID.
- **Separate lifecycles.** Runtime agent lifecycle, attention/read state and
  mission lifecycle are separate. Agent `done` means a finished turn. Reading a
  completion acknowledges that episode; reading an input-needed item does not
  answer it. Mission `Active`, `Done` and `Archived` are explicit user choices and
  do not stop processes, remove worktrees or acknowledge attention.
- **Shared acknowledgment.** A completion becomes read after its real terminal
  is successfully revealed in the active window, or by explicit mark-read.
  Acknowledgment targets the observed episode ID/revision, is idempotent, and
  cannot consume a newer result. All clients of the owning daemon receive it.
  Unread completions and shared read state survive daemon restart; previously
  blocked sources remain unconfirmed until a fresh runtime report.
- **One daemon per mission.** Members must belong to the mission's daemon. The
  desktop may combine lists from several daemons, retaining source connection
  identity for navigation and mutations. It does not create cross-daemon
  membership or ownership.
- **Additive compatibility.** Extend workspace and wire schemas compatibly, with
  defaults for old persisted data and snapshots. Preserve existing lifecycle
  meanings and old-client snapshot loading. Unsupported or disconnected daemons
  do not get a client-local acknowledgment fallback. Older binaries may ignore
  new fields and drop them on save; compatibility does not promise downgrade
  round-trip preservation.

The first delivery uses the existing project-owned terminal surface and reveal
path. It adds no cross-project layout or duplicate live-terminal tiles.

## Consequences

Work can retain its identity across implementation and review conversations,
terminal replacement and checkout removal. The inbox remains useful without
missions, and shared read state prevents each client from maintaining a separate
unread queue for the same result.

This requires durable membership and episode identity rather than deriving them
from current panes. Restore/import must preserve or remap references correctly;
stale acknowledgments must not clear newer attention. A disconnected daemon's
records are unavailable for mutation, even when another daemon remains connected.

Attention depends on current explicit agent reporting. Input-needed means
**last reported blocked**, not independently verified process liveness. An agent
that crashes while its shell survives and emits no report remains undetected
until another report, pane exit or daemon restart. The first delivery adds no
screen scraping or implicit process detector. Unsupported agents still work as
ordinary terminals.

## Alternatives considered

- **Session as project.** Rejected because conversation identity and project
  ownership have different lifetimes. It would couple durable work grouping to
  terminal/layout ownership and obscure work that spans conversations or
  repositories.
- **Read state local to each client.** Rejected because reading a result should
  acknowledge it for all clients of that daemon. Independent queues would show
  the same completion as unread elsewhere and weaken reconnect consistency.
- **Focus resolves attention or completes work.** Rejected because viewing a
  blocked agent does not answer it, and a completed turn does not complete a
  mission. Read acknowledgment, runtime reports and explicit mission lifecycle
  changes serve different purposes.
- **Cross-project mission layouts in the first delivery.** Deferred because
  existing terminal navigation can validate mission grouping without changing
  terminal ownership or introducing competing live-terminal resize writers.
