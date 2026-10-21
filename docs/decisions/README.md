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
- 0004 — [Run the AI account proxy as its own process](0004-ai-proxy-process.md) — accepted (2026-10-07)
- 0005 — [Keep AI routing policies in the proxy, keyed by a project segment in the URL](0005-ai-routing-policies-in-the-proxy.md) — accepted (2026-10-08)
- 0006 — [Connect claude panes to remote clients through a Bun preload, hooks and an MCP channel](0006-claude-remote-via-bun-preload.md) — accepted (2026-10-16)
- 0007 — [Serve AI proxy management on a private socket, not the inference port](0007-ai-proxy-management-on-a-private-socket.md) — accepted (2026-10-21)
