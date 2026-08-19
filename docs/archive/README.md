# archive

Shipped sprints (each carrying its `OUTCOME` header — that's the record) and the
rare backlog/spec item with standalone reference value. Items arrive here by
`git mv`; they are **not** edited afterward.

**Default is delete, not archive.** The archive is not a graveyard for everything
that ships — only what genuinely helps a future reader. The git log holds the rest.

<!-- optional: group by date or theme as it grows; one line per entry -->

- [`headless-migration.md`](headless-migration.md) — the two-process daemon migration, shipped 2026-06-29. Kept for the phase-by-phase commit map and the data-vs-presentation split rationale; §4-§10 are the superseded original plan. The durable *why* is [ADR-0001](../decisions/0001-headless-two-process-daemon.md).
