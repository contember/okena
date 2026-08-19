# reference

How the system works **now** — architecture, conventions, runbooks. Flat files,
`kebab-case.md`.

Rules (see [`../CLAUDE.md`](../CLAUDE.md)): describe the current state only — no
status updates, no TODOs (file those in `../backlog/`), no design rationale (that's
a `../decisions/` ADR). Update reference in the **same change** that alters
behaviour.

<!-- index the reference docs here, one line each -->

- [`glossary.md`](glossary.md) — domain terms: workspace, project, worktree, folder, layout, window.
- [`configuration.md`](configuration.md) — settings file, keybindings, per-project config.
- [`hooks.md`](hooks.md) — lifecycle hooks: events, config shape, execution.
- [`services.md`](services.md) — Docker Compose integration and port detection.
- [`worktrees.md`](worktrees.md) — git worktree projects: create, close, parent linkage.
- [`remote.md`](remote.md) — remote control server: pairing, HTTP/WS API, TLS.
- [`mobile.md`](mobile.md) — React Native mobile client architecture (uniffi over `okena-mobile-ffi`).

For crate-level and module-level detail, read the `CLAUDE.md` next to the code
(`crates/*/CLAUDE.md`, `crates/okena-app/src/**/CLAUDE.md`, `web/`, `mobile/rn/`).
