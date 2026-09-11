# Knowledge stores

A **knowledge store** is a git repository holding an organisation's engineering
knowledge: how CI works, how the repositories are laid out, where service
boundaries are, how reviews and releases are done. It also holds the skills,
subagents and prompt templates the organisation shares. okena reads stores,
clones them, and keeps them up to date. It lists what is in them in the harness
Knowledge section.

The format belongs to okena ([ADR-0003](../decisions/0003-knowledge-stores.md)).
Skills and agents use the existing Claude formats, so the repository is useful
to an agent that has never heard of okena.

## Layout

```text
<store>/
├── .okena-knowledge/store.yaml   identity (see below)
├── docs/**/*.md                  principles, processes, architecture, runbooks
├── skills/**/SKILL.md            one skill per directory holding a SKILL.md
├── agents/**/*.md                one subagent per file
└── templates/**/*.md             prompt templates
```

Every folder is optional. A root needs at least one kind folder or an identity.
Anything outside the four kind folders (a `README.md`, CI config) is ignored.

| Kind | What counts as an entry | Name |
|---|---|---|
| Doc | every `.md` file under `docs/`, at any depth | path under `docs/` without `.md`, e.g. `ci/pipeline` |
| Skill | every directory below `skills/` that holds a `SKILL.md`; the other files in it are the skill's supporting files | frontmatter `name`, else the directory path, e.g. `release` |
| Agent | every `.md` file under `agents/` | frontmatter `name`, else the path without `.md` |
| Template | every `.md` file under `templates/` | path under `templates/` without `.md` |

A `SKILL.md` directly in `skills/` is not a skill. A skill's subdirectories
belong to that skill; okena does not look for more skills inside them.

Rules for walking the folders:

- Names starting with `.` are skipped.
- Symlinked directories are never followed.
- A kind folder that is itself a symlink is ignored.
- A symlinked file counts only when it resolves inside the root.

An entry is addressed by its **store id plus its path relative to the store
root**, e.g. `acme-eng` + `docs/ci/pipeline.md`. That address is the same on
every machine, whatever the checkout folder is called.

## Frontmatter

Frontmatter is optional YAML between a `---` on the first line and a closing
`---` or `...`.

| Field | Kinds | Meaning |
|---|---|---|
| `title` | all | Display title |
| `description` | all | One line saying what the entry is for; what a person or an agent picks an entry by |
| `tags` | all | A list, or one comma-separated string |
| `name` | skill, agent | The entry's name, as the Agent Skills and Claude subagent formats define it |
| `for` | template | The launch flows the template applies to: a list, or one comma-separated string |

When there is no `title`, okena uses the first `# ` heading outside a code
fence, then the entry's name.

A template lists the `{placeholder}` names its body uses. A placeholder is a
single identifier in braces (`{key}`, `{change_dir}`), so JSON such as
`{"a": 1}` does not count. Which flows exist, and which variables each one
fills, is not defined yet.

Frontmatter okena cannot parse does not hide the entry. The entry is still
listed, carrying a `frontmatter_invalid` warning. Fields other than those above
are ignored when listing, and are shown as they are when the entry is opened.

## Store identity

`.okena-knowledge/store.yaml`:

```yaml
version: 1
id: acme-eng                 # kebab-case: lowercase letters and digits, single hyphens
name: Acme Engineering       # optional; the id when unset
description: How we build    # optional
remote: git@github.com:acme/eng-knowledge.git   # optional canonical clone source
```

Unknown keys are ignored. A `version` newer than okena understands is refused.
Commit this file: it is what makes every clone agree on the store's id.

## Registry

okena lists the stores on a machine in
`<profile config dir>/knowledge/stores.yaml`. On macOS the profile config dir is
under `~/Library/Application Support/okena/`.

```yaml
version: 1
stores:
  acme-eng:
    path: /Users/me/knowledge/eng-knowledge
    remote: git@github.com:acme/eng-knowledge.git   # origin when registered
```

The rules for this file:

- **Writer:** only the daemon writes it.
- **One-to-one:** a store id has one checkout, and a checkout has one id.
- **Not settings:** checkout paths are machine state, so they are kept out of
  `settings.json`.
- **Corrupt file:** it is reported, never overwritten.

There are three ways to add a store:

- **Clone:** clones a URL and registers the checkout.
  - With no destination, it clones into `harness.knowledge.clone_dir`
    (default `~/knowledge`), in the folder `git clone` would name.
  - If the clone fails, a folder okena created is removed.
  - If the repository turns out not to be a knowledge root, the checkout stays
    on disk and the error says where it is.
- **Existing folder:** registers a checkout already on disk. Its id comes from
  `store.yaml`. Without one, the id comes from the folder name, and the store
  carries a `store_identity_missing` warning until an identity is committed.
- **Create:** writes the identity and the four kind folders, each with a
  `.gitkeep`, and registers the store.
  - With git, it runs `git init` when needed and makes one commit,
    `Initialize knowledge store <id>`, of exactly those paths.
  - It refuses a file, a non-empty folder (a folder holding only `.git` is
    allowed), a folder inside another git repository, a taken id, and a
    missing git author identity.
  - If a step fails before the commit, everything it created is removed.

Unregistering removes only the registry entry. The checkout stays on disk.

## Projects

A project repository joins through `.okena/knowledge.yaml` at its root:

```yaml
stores: [acme-eng]           # stores this repository follows
root: docs/knowledge         # this repository's own kind folders (default .okena/knowledge)
```

Both keys and the file are optional.

- **Followed stores:** each id in `stores:` puts the project in that store's
  "used by" list. An id not registered on this machine is reported as
  `unknown_store`.
- **Project root:** the directory at `root:` becomes a project knowledge root
  when it exists. It holds the same kind folders as a store. `root: .` makes the
  repository itself the root.
- **Default root:** a repository with a `.okena/knowledge/` folder has a
  project root without any config file.
- **Stays inside:** `root:` must stay inside the repository, checked both as
  written and after symlinks resolve. Otherwise it is reported as
  `project_root_outside`.
- **Duplicate path:** a project root at the same path as a registered store is
  listed once, as the store.

Worktrees and agent sessions are never searched. Project discovery can be
turned off with `harness.knowledge.projects`.

## Sync

Sync state is read from local git and never touches the network. It is
therefore as fresh as the last fetch:

- the checked-out branch and its upstream
- commits ahead and behind
- whether there are uncommitted changes
- when the checkout last fetched (the time of `FETCH_HEAD`)

Only a folder with its own `.git` is treated as a checkout. A store folder
nested inside another repository has no sync state.

- **Fetch** runs `git fetch --all`.
- **Pull** fetches, then runs `git merge --ff-only @{upstream}`. It refuses a
  detached HEAD (`detached_head`), a branch without an upstream (`no_upstream`),
  and a branch that is both ahead and behind (`diverged`). The error names the
  checkout to fix it in.

Git runs non-interactively. Credentials must come from an SSH agent or a
credential helper, and a prompt fails instead of hanging. Only stores sync. A
project root is synced with its project's own git.

okena commits nothing to a store except the initial commit when creating one,
and never pushes. Changes are made in a terminal, or by an agent working in the
checkout.

## In okena

- **Harness → Knowledge** has these parts:
  - **Root list:** every root, with its health and a sync badge (`↑` commits to
    push, `↓` commits to pull, `•` uncommitted changes).
  - **Entry list:** the open root's entries grouped by kind, with docs nested
    by folder, and a filter over titles, names, paths, descriptions and tags.
    Markdown entries render formatted, and a skill lists its supporting files.
  - **Store overview:** the branch, the last fetch, and **Fetch** and **Pull**
    buttons. Pull is offered only when a fast-forward is possible.
  - **Unresolved stores:** projects that follow a store not on this machine are
    listed under "Followed, not here".
- **New with agent** opens an agent session in a root, briefed on this layout
  and on the frontmatter entries are picked by.
  - **In a store:** the agent is told to work on a `knowledge/<topic>` branch,
    commit, and not push unless asked.
  - **In a project root:** committing is left to you.
  - **Agent:** it starts the agent you pick, else `harness.agent_command`;
    without an agent it refuses.
- **Settings → Knowledge** clones, adds and creates stores. It removes a store
  from the registry while leaving the checkout on disk. It also switches
  project discovery on or off and sets the clone folder.

## Limits

| Limit | Value |
|---|---|
| Entries listed per root | 5 000 (`entry_limit` warning past it) |
| Supporting files listed per skill | 200 |
| Bytes read per file when listing | 256 KiB |
| Largest file opened | 2 MiB |

Reading is confined to discovered roots. A root key the daemon did not discover
is refused, and so is a path that resolves outside its root.
