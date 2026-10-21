# archive

Shipped sprints (each carrying its `OUTCOME` header — that's the record) and the
rare backlog/spec item with standalone reference value. Items arrive here by
`git mv`; they are **not** edited afterward.

**Default is delete, not archive.** The archive is not a graveyard for everything
that ships — only what genuinely helps a future reader. The git log holds the rest.

<!-- optional: group by date or theme as it grows; one line per entry -->

- [`headless-migration.md`](headless-migration.md) — the two-process daemon migration, shipped 2026-06-29. Kept for the phase-by-phase commit map and the data-vs-presentation split rationale; §4-§10 are the superseded original plan. The durable *why* is [ADR-0001](../decisions/0001-headless-two-process-daemon.md).
- [`sprint-2026-10-20-whats-new.md`](sprint-2026-10-20-whats-new.md) — in-app release notes from GitHub Releases with a one-time post-update toast, shipped 2026-10-21. Kept for the commit map and the review fixes (daemon-side cache, version skew between desktop and daemon).
- [`sprint-2026-10-20-terminal-status-board.md`](sprint-2026-10-20-terminal-status-board.md) — the status board, shipped 2026-10-21 as project / worktree statuses with status lanes in the main area. Kept for the run log of its two design pivots (per-terminal → per-project; modal kanban board → lanes). How it works now: [status-board.md](../reference/status-board.md).
