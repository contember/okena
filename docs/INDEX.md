# Okena docs — index

The map of everything under `docs/`. Read [`CLAUDE.md`](CLAUDE.md) for the rules.
When sources disagree, precedence is: the `CLAUDE.md` hierarchy (root + per-crate, binding) → active
sprint → decisions → reference → archive.

## Folders

- [`reference/`](reference/README.md) — how the system works now.
- [`ideas/`](ideas/README.md) — proposals, no commitment.
- [`decisions/`](decisions/README.md) — ADRs (the *why*), immutable.
- [`backlog/`](backlog/README.md) — decided work, not yet scheduled.
- [`sprints/`](sprints/README.md) — active thematic work-plans.
- [`archive/`](archive/README.md) — shipped sprints + reference-worthy records.

## Also binding, outside docs/

Architecture and build rules live next to the code, not here:

- [`../CLAUDE.md`](../CLAUDE.md) — repo hub: build commands, crate map, module pointers.
- `crates/*/CLAUDE.md` — per-crate detail (`okena-workspace`, `okena-terminal`, `okena-git`).
- `crates/okena-app/src/**/CLAUDE.md` — desktop app, app coordinator, keybindings.
- `crates/okena-remote-server/src/CLAUDE.md`, `crates/okena-cli/src/CLAUDE.md`.
- [`../web/CLAUDE.md`](../web/CLAUDE.md), [`../mobile/rn/CLAUDE.md`](../mobile/rn/CLAUDE.md) — the two non-Rust clients.

## Active sprints

<!-- list the sprint files currently in sprints/ ; empty between sprints -->
- [Knowledge stores in the harness](sprints/sprint-2026-09-10-knowledge-stores.md) (2026-09-10)

## What's hot

<!-- hand-maintained, keep short: the few things actually in motion + what's next.
     If everything is "hot", nothing is. -->
- **Knowledge stores** — the harness Knowledge view over git-backed org knowledge
  repos ([sprint](sprints/sprint-2026-09-10-knowledge-stores.md),
  [ADR-0003](decisions/0003-knowledge-stores.md)). Next: knowledge at agent launch
  ([backlog 04](backlog/04-knowledge-context-at-launch.md)) and template-driven
  prompts ([backlog 05](backlog/05-prompt-templates-from-knowledge.md)).
