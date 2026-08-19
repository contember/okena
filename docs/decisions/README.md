# decisions (ADR)

One file per significant architectural/product decision: `NNNN-<slug>.md`
(monotonic, never reused). Copy [`_template.md`](_template.md).

**Immutable.** Once a decision is Accepted, don't rewrite it — to change course,
write a *new* ADR and set the old one's status to `Superseded by NNNN`.

Write one when the choice (a) constrains future work, (b) rejected a real
alternative, or (c) someone will later ask "why did we do it this way?". Otherwise
a commit message suffices.

## Log

<!-- newest last; one line each: NNNN — title — status (date) -->

- 0001 — [Split Okena into a headless daemon and thin clients](0001-headless-two-process-daemon.md) — accepted (2026-06-29)
- 0002 — [A window is a filtered viewport, not a partition](0002-window-as-viewport.md) — accepted (2026-05-12)
