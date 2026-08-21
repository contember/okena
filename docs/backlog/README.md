# backlog

Decided work items ("issues") not yet scheduled into a sprint. One self-contained
file per item: `NN-<slug>.md` (zero-padded, **folder-local** sequence — don't
renumber, gaps are fine). Copy [`_template.md`](_template.md).

**No `status:` field** — an item is alive because it lives here. It leaves by being
**deleted** on ship (default; git holds the record) or moved to `../archive/` if it
documents something a future reader needs. Dependencies go in frontmatter:
`blocked-by: [./NN-other.md]`.

Add scope sub-folders (`security/`, `perf/`, …) only once the flat list gets
unwieldy; numbers stay folder-local.

## Items

<!-- one line each: NN — short summary (link). Keep it short. -->

- 01 — [Markdown preview virtualization](01-markdown-preview-virtualization.md) — perf; blocked on a selection model that survives virtualization.
- 02 — [WindowView column/scroll extraction](02-window-view-column-extraction.md) — refactor; needs dependency threading + a new scroll anchor.
- 03 — [Daemon/client parity follow-ups](03-daemon-parity-follow-ups.md) — 1 verified open, 1 flagged; both need a live session.
