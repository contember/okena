---
id: 0003
title: Knowledge stores are kind-folder git repos with an okena-owned registry
status: accepted
date: 2026-09-10
---

# 0003 — Knowledge stores are kind-folder git repos with an okena-owned registry

## Context

The harness has a Knowledge section (`HarnessSection::Knowledge`) that renders a
stub. It is meant to hold the engineering knowledge that sits above any single
repo: how CI works, how directories and service boundaries are laid out,
review and release principles, plus the skills, subagents and prompt templates
an organisation shares. Most organisations will keep this in one shared git
repo; individual repos may add their own.

Two later features build on it and fix what the format must support:

- **Context selection at launch.** Starting an agent (on a task, a spec, a
  free-form goal) should let the user pick knowledge entries to hand it. That
  needs a stable address for an entry and a description to choose by.
- **Template-driven prompts.** The briefs okena sends today are hardcoded
  `format!` strings (`execute/specs.rs` `brief`, `execute/tasks.rs`
  `custom_brief`, `harness.agent_args` substitution). They should come from
  templates an organisation can edit.

OpenSpec already solved "a git repo of planning documents, registered on a
machine, referenced from projects" (`okena-openspec`), so the shape is proven —
but there is no external CLI to stay compatible with here, so okena owns the
format.

## Decision

**A knowledge root is a directory with kind folders.** A store is a knowledge
root at the top of a git repo with a committed identity:

```text
<store>/
├── .okena-knowledge/store.yaml   version: 1, id: <kebab-id>, name?, description?, remote?
├── docs/**/*.md                  principles, processes, architecture, runbooks
├── skills/**/SKILL.md            Agent Skills format; the SKILL.md's directory is the skill
├── agents/**/*.md                Claude Code subagent format
└── templates/**/*.md             prompt templates: `for:` flows, `{placeholder}` body
```

- Every folder is optional; a root needs at least one of them or an identity.
- Frontmatter is optional everywhere. `title`, `description` and `tags` are
  read for docs; skills and agents use their formats' own `name` and
  `description`; templates add `for:` (the launch flows they apply to).
  Missing titles fall back to the first `#` heading, then the file name.
- Skills and agents use the existing Claude formats unchanged, so the repo is
  useful to an agent that has never heard of okena.
- An entry's address is `(store id, path relative to the root)`, e.g.
  `acme-eng` + `docs/ci/pipeline.md`.

**okena reads checkouts and never writes to them**, except when creating a new
store (scaffold plus one initial commit). Registering a checkout without
`store.yaml` derives its id from the folder name and reports a warning rather
than leaving an uncommitted file in someone's shared repo.

**Stores are registered in an okena-owned registry file,**
`<profile config dir>/knowledge/stores.yaml` (`id → path, observed remote`),
written only by the daemon. Settings hold only discovery preferences
(`harness.knowledge.projects`, `harness.knowledge.clone_dir`).

**Projects join through `.okena/knowledge.yaml`** at the repo root:
`stores: [<id>…]` names the org stores the repo follows (shown as "used by"),
and `root:` (default `.okena/knowledge`) is where the repo's own kind folders
live.

**Git goes through the `git` binary** with the non-interactive network
environment `okena-git` already uses: `status --porcelain=v2 --branch` for sync
state, `clone`, `fetch`, and fast-forward-only pull. Commit and push are left
to the user or an agent in a terminal.

## Consequences

- Organisations get one repo layout that works with and without okena; the
  reference for store authors is `docs/reference/knowledge.md`.
- Context selection and templates have what they need: a stable entry address,
  descriptions, and a `templates/` kind with `for:` — without this ADR deciding
  their variable sets, which are defined when those flows are wired.
- The registry is per profile and per machine. A dotfile-synced
  `settings.json` never carries checkout paths that don't exist elsewhere.
- Because the registry is not `settings.json`, store mutations can run on the
  daemon's blocking pool. `SetSettings` needs the command loop's
  `&mut daemon_config`, which an off-lock task cannot borrow.
- A repo that isn't a store (no identity) still works, with a warning. That
  makes adoption cheap, but ids can then change if the folder is renamed.
- Merge conflicts, rebases and pushes stay out of okena's UI. "Pull" refuses
  anything that isn't a fast-forward and says why.

## Alternatives considered

- **Claude Code plugin layout** (`.claude-plugin/plugin.json`, `skills/`,
  `agents/`, `commands/`). This would make the repo installable as a plugin, but
  it ties the format to one vendor's manifest and has no place for docs or
  templates. The kind folders keep the same `skills/` and `agents/` shapes, so a
  plugin manifest can be added to a store later without moving files.
- **Freeform markdown with `kind:` frontmatter.** This is the most flexible,
  but every file needs frontmatter before it shows up correctly, and nothing
  about the tree tells a human reader where things go.
- **Stores listed in `settings.json`.** Users can already see and edit
  settings, but checkout paths are machine state rather than preferences, and
  the daemon's settings writer can't be reached from the blocking pool where
  clone and pull run.
- **Reusing OpenSpec stores** (a `knowledge/` folder inside an OpenSpec store).
  This would couple two lifecycles. Knowledge is read by every agent; specs
  belong to one change.
