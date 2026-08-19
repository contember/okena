# docs/ operating manual

How docs work in **Okena**. This file is self-contained: read it and you
know where every kind of document lives, how to create one, and how it dies.
(Structure maintained by the `agent-docs` skill — but you don't need the skill to
follow these rules.)

## Source-of-truth precedence

When documents disagree, higher wins:

1. `docs/invariants/` or hard rules in the root `CLAUDE.md` — binding.
2. The active sprint plan in `sprints/` — the contract for current work.
3. `decisions/` (ADR) — why the system is the way it is.
4. `reference/` — how the system currently works.
5. `archive/` — historical; informational only, may be stale.

Okena has no `docs/invariants/` folder. Level 1 is the **`CLAUDE.md` hierarchy**:
the root [`../CLAUDE.md`](../CLAUDE.md) plus the per-crate/per-module `CLAUDE.md`
files (`crates/*/CLAUDE.md`, `crates/okena-app/src/**/CLAUDE.md`, `web/`,
`mobile/rn/`). Those are binding for build commands, architecture rules and code
conventions, and they live **next to the code they govern** — not under `docs/`.
`docs/reference/` covers user-facing and cross-cutting subsystem knowledge; when
the two overlap, the `CLAUDE.md` next to the code wins.

## The folders — one purpose each

| Folder | Holds | Lifecycle |
|---|---|---|
| `reference/` | living knowledge: architecture, conventions, runbooks — "how it IS now" | edit in place when behaviour changes |
| `ideas/` | research, proposals, half-formed thoughts — **no commitment** | graduate → `backlog/`/sprint, or delete |
| `decisions/` | ADR — one significant decision each, the *why* | **immutable**; supersede, never rewrite |
| `backlog/` | decided work items ("issues") not yet scheduled | delete on ship (or archive if reference-worthy) |
| `sprints/` | active thematic work-plans being executed now | archive (with OUTCOME) on ship |
| `archive/` | shipped sprints + reference-worthy shipped items | append-only graveyard-lite |
| `specs/` *(optional)* | design docs for features big enough to design before building | freeze at accept; supersede |

**Do not mix purposes.** A backlog item is not an idea; a status update is not
reference. **The path is the status** — a doc moves between folders rather than
carrying a `status:` field.

## Where does a new document go?

| You have… | Put it in… |
|---|---|
| A research note / RFC / half-formed idea | `ideas/<slug>.md` |
| A decided, not-yet-scheduled work item | `backlog/NN-<slug>.md` |
| A significant decision + its rationale | `decisions/NNNN-<slug>.md` (copy `decisions/_template.md`) |
| A plan for a chunk of work to do now | `sprints/sprint-YYYY-MM-DD-<theme>.md` (copy `sprints/_template.md`) |
| A change to how the system works | update the relevant `reference/*.md` |
| A pre-build design for a large feature | `specs/<slug>.md` (only if it's genuinely big) |

After adding anything, update the relevant folder `README.md` (the index) and, if
it's a new top-level item, `docs/INDEX.md`.

## Sprints — the unit of work

A sprint is a **thematic batch of work executed now**. Running it unattended
("do everything autonomously") is just a sprint with no human watching — same
file, same rules.

**Create** — copy `sprints/_template.md` to
`sprints/sprint-YYYY-MM-DD-<theme>.md`. A good plan states:
a single **theme/goal**; the load-bearing facts **re-verified at HEAD** (read the
actual code, cite `file:line`, mark ✔ confirmed / ⚠ drifted before planning on
them); **work units (WU)** each with effort, the problem, a cheap *verify-first*
check, scope, an **acceptance/witness** (the test that proves it), and touch
points (file paths); **out of scope** (what's deferred + why); resolved
**decisions**; and **sequencing** (what's parallel). Multiple sprints may be
active at once.

**Run** — work the WUs. Append to the sprint's **`## Run log`** as you go
(discoveries, deviations, blockers). The run log is ephemeral scratch — see
graduation below.

**Close** — when shipped: stamp an **`OUTCOME`** header at the top (commit map +
verification numbers + what was deferred), `git mv` the file to
`archive/sprint-YYYY-MM-DD-<theme>.md`, delete or re-scope the backlog items it
consumed, and refresh any `reference/*` whose behaviour changed — in the same
change. Then update `INDEX.md` and the relevant `README.md`.

## The run log + graduation

The sprint's `## Run log` is where an agent records deviations, surprises, and
blockers found during a run (this replaces a separate "drift/trouble" tracker).
Each entry **graduates** out of the scratch log:

- It changed *why* the system is built a certain way → write a **decision**
  (`decisions/NNNN-*.md`).
- It's new work to do later → file a **backlog item** (`backlog/NN-*.md`).
- It was a transient hiccup with no future value → leave it in the log; it dies
  with the sprint when the sprint is archived (git holds the record).

After graduating an entry, trim it to a one-line pointer ("→ ADR-0007") so the log
doesn't duplicate the durable doc.

## Decisions (ADR)

One file per significant decision: `decisions/NNNN-<slug>.md` (copy
`decisions/_template.md`). Sections: Status · Context · Decision · Consequences ·
(Alternatives). **Immutable**: once Accepted, never rewrite — to change a decision,
write a *new* ADR and set the old one's status to `Superseded by NNNN`. Numbers
are monotonic and never reused.

**When to write one:** the choice (a) constrains future work, (b) rejected a real
alternative, or (c) someone will later ask "why did we do it this way?". Otherwise
a commit message is enough — don't manufacture ADRs.

## Backlog items

A backlog item is a self-contained file: `backlog/NN-<slug>.md` (copy
`backlog/_template.md`). `NN` is a zero-padded, **folder-local** sequence — don't
renumber, gaps are fine. **No `status:` field** — it's alive because it's here.
Use frontmatter `blocked-by: [./NN-other.md]` for dependencies. When the work
ships, **delete** the file (default) or move it to `archive/` if it documents a
non-obvious decision a future reader needs. Keep `backlog/README.md` as a short
index. Add scope sub-folders (`backlog/security/`, `backlog/perf/`) only once the
flat list gets unwieldy.

## Reference

`reference/*.md` is for understanding the **current** system. Flat — no Diátaxis
quadrants. Rules: no status updates ("recently we changed…"), no TODOs (file them
in `backlog/`), no design rationale (that's a decision or archive). When behaviour
changes, update reference in the same change. If you spot drift you can't resolve,
flag it — don't guess.

## Ideas

`ideas/<slug>.md` — anything exploratory, zero commitment. An idea either
graduates (becomes a backlog item or gets pulled into a sprint) or is deleted. It
is never a place for decided work or status.

## Archive

`archive/` keeps shipped sprints (the OUTCOME header is the record) and the rare
backlog/spec item with standalone reference value. **Default is delete, not
archive** — the archive is not a graveyard for everything. Done items move here by
path (`git mv`); they are not edited afterward.

## Conventions

- File names: `kebab-case.md`. English, regardless of surrounding language.
- Backlog: `NN-` folder-local. ADR: `NNNN-` monotonic. Sprints:
  `sprint-YYYY-MM-DD-<theme>.md`. Templates: `_template.md` (ignored by tooling).
- Only `README.md` and this `CLAUDE.md` live in `docs/` root — everything else in
  a subfolder.
- **Cross-references use relative paths.** When you move a file, `grep -r` its old
  path under `docs/` and fix every reference before committing.
- One navigation entry point: `docs/INDEX.md` maps everything. Keep it current.

## Things NOT to do

- Don't add a `status:` field or a `todo/` folder — the folder is the status.
- Don't add top-level meta files (`STATUS.md`, `PROGRESS.md`) — the structure
  encodes status. (One living `STATUS.md` *inside* a long-running subsystem folder
  is the only exception.)
- Don't rewrite a decided ADR — supersede it.
- Don't let the archive become a dump — delete by default.
- Don't duplicate content across folders — link to the one canonical copy.
