# Sprint — Knowledge stores in the harness (2026-09-10)

**Goal.** Replace the harness Knowledge stub with a real view over git-backed
knowledge stores. A store is an organisation's shared repo of engineering docs,
skills, agents and prompt templates, cloned, synced and read from okena.

**Theme.** Everything that makes a knowledge store exist on a machine and
readable in okena: the on-disk format ([ADR-0003](../decisions/0003-knowledge-stores.md)),
a GPUI-free disk crate, git clone/fetch/pull, daemon actions, the view, the
settings page, and an agent that writes into a store.

The success condition is one walkthrough:
1. Clone `git@github.com:acme/eng-knowledge.git` from Settings → Knowledge.
2. See its docs, skills, agents and templates in the harness, and read one rendered.
3. See "↓3 behind" and pull.
4. Open an agent briefed to add a new doc.

Handing knowledge to agents at launch ([backlog 04](../backlog/04-knowledge-context-at-launch.md))
and template-driven prompts ([backlog 05](../backlog/05-prompt-templates-from-knowledge.md))
come next. This sprint fixes the format they build on.

## Design at a glance

Full rationale is in ADR-0003. This is the shape every WU implements.

```text
<store>/                              a git repo
├── .okena-knowledge/store.yaml       version: 1 · id · name? · description? · remote?
├── docs/**/*.md                      frontmatter: title? description? tags?
├── skills/**/SKILL.md                Agent Skills format (name, description) + supporting files
├── agents/**/*.md                    Claude subagent format (name, description, tools?, model?)
└── templates/**/*.md                 frontmatter for: [flow…]; body uses {placeholder}

<project repo>/.okena/knowledge.yaml  stores: [acme-eng]      # org stores this repo follows
                                      root: .okena/knowledge  # this repo's own kind folders (default)

<profile config dir>/knowledge/stores.yaml   okena's registry, daemon-written
  version: 1
  stores:
    acme-eng: { path: /Users/me/knowledge/eng-knowledge, remote: git@github.com:acme/eng-knowledge.git }
```

- **Roots.**
  - A *store* is a registered checkout, keyed `store:<id>`.
  - A *project root* is a project's own kind folders, keyed `path:<abs>`.
  - The daemon only accepts keys it discovered and only reads paths that resolve
    inside the root. This is the Specs guard, for the same reason: any paired
    client can reach these actions.
- **Entries.**
  - Titles fall back from frontmatter `title`, to the first `#` heading, to the
    file name.
  - Skills list their supporting files.
  - Templates list their `for:` flows and the `{placeholders}` found in the body.
  - Bad frontmatter still lists the entry, with a warning.
- **Git.**
  - Local sync state comes from `git status --porcelain=v2 --branch`: branch,
    upstream, ahead/behind, dirty count, and last fetch time from the
    `FETCH_HEAD` mtime.
  - Network commands are `clone`, `fetch`, and `merge --ff-only @{u}`.
  - Commit and push are left to a terminal or an agent.

## Refs re-verified at HEAD (2026-09-10)

`✔` = confirmed live · `⚠` = drift/nuance caught.

- ✔ `HarnessSection::Knowledge` exists with a blurb. The sidebar nav, the window and
  the tab strip all follow `all()`, so no nav work is needed —
  `crates/okena-core/src/harness.rs:22,62`.
- ⚠ Knowledge renders `render_stub`, and the section match ends in `other =>`, so a
  forgotten arm compiles and silently shows the stub. Make the match exhaustive
  when adding the view — `crates/okena-app/src/views/harness/sections.rs:173-201,207-211`.
- ✔ Specs has the pull-style action pattern to mirror (`SpecStores`, `SpecsTree`,
  `SpecRead`, store mutations, `SpecDraftChange`) — `crates/okena-core/src/api.rs`
  (OpenSpec block).
- ✔ Discovered-key plus path-guard pattern: `resolve_root`, `read_for`, and the
  2 MiB `MAX_DOC_BYTES` cap —
  `crates/okena-app-core/src/workspace/actions/execute/specs.rs:77-96,121,134-164`.
- ✔ Store mutations run off the command queue and the workspace lock on the
  blocking pool, with a settings snapshot —
  `crates/okena-daemon-core/src/command_loop.rs:2941-2976`.
- ✔ `SetSettings` runs in the main loop and needs `&mut daemon_config`, which an
  off-lock task cannot borrow. This is why the store registry is its own file, not
  `settings.json` — `crates/okena-daemon-core/src/command_loop.rs:3097-3123`.
- ✔ Timeout buckets: clone, fetch and pull need `LongMutation` —
  `crates/okena-transport/src/remote_action.rs:46-89`.
- ✔ Harness actions bypass `ActionDispatcher`'s id mapping through an explicit
  passthrough — `crates/okena-app/src/action_dispatch.rs:981-1010`.
- ✔ OpenSpec's git helpers never fetch, and they are private (`mod git;`) —
  `crates/okena-openspec/src/git.rs:1-2`, `src/lib.rs:19`.
- ✔ The non-interactive network git environment exists only as
  `pub(crate) network_command` in `crates/okena-git/src/repository/mod.rs:69`
  (used by `fetch --all` at `repository/branch.rs:217`).
- ✔ The settings schema to extend is `HarnessConfig` and its `SpecDiscoveryConfig`
  sibling — `crates/okena-workspace/src/settings.rs:68-160,164-212`.
- ✔ The per-profile config dir, which hosts the registry, is
  `crates/okena-workspace/src/persistence.rs:75`.
- ✔ `okena-markdown` already parses YAML frontmatter into a metadata card —
  `crates/okena-markdown/src/parser.rs:367-420`, `render.rs:601`. `MarkdownDocument`
  is used by `crates/okena-files/src/file_viewer/mod.rs:25` and re-exported at
  `crates/okena-app/src/views/overlays/markdown_renderer/mod.rs:3`.
- ⚠ The Specs view deliberately shows documents as plain text
  (`crates/okena-app/src/views/harness/specs_view.rs:9-12`). Knowledge renders
  Markdown, because people read a knowledge doc first; a spec is shown the way an
  agent will read it.
- ⚠ The OpenSpec project filter skips `is_spec_session() || is_agent_session()` but
  not custom sessions (`execute/specs.rs:41`). Knowledge discovery uses
  `is_any_agent_session()` (`crates/okena-state/src/workspace_data.rs:53`). The
  Specs gap is noted and not fixed here.
- ✔ Prompts are hardcoded, with no template engine: `substitute`
  (`execute/tasks.rs:213`), `brief` (`execute/specs.rs:262`), `custom_brief`
  (`execute/tasks.rs:995`). `prompt_args` (`execute/specs.rs:316`) and
  `agent_mcp::injection_args` are reusable for the draft agent.

## Work units

### WU1 — Wire types (effort S)

- **Problem.** No shapes exist for knowledge roots, trees or entries. The
  diagnostic type the Specs types use is named for OpenSpec.
- **Verify first.** Check that `SpecDiagnostic`/`SpecSeverity` have no
  OpenSpec-specific fields (`okena-core/src/specs.rs:109-160`), so they can move
  without changing the wire shape.
- **Scope.**
  1. Add `okena-core/src/knowledge.rs` with:
     - `KnowledgeKind { Doc, Skill, Agent, Template }`
     - `KnowledgeEntry { kind, path, name, title, description?, tags, files (skills), flows + variables (templates), status }`
     - `KnowledgeTree { root_key, root, store_id?, entries, status }`
     - `KnowledgeRootKind { Store, Project }`
     - `KnowledgeGitStatus { branch?, upstream?, ahead, behind, dirty, fetched_at? }`
     - `KnowledgeRoot { key, kind, name, path, store_id?, description?, remote?, healthy, git?, used_by, counts, status }`
     - `KnowledgePointer { project, path, store_id, root_key?, status }`
     - `KnowledgeStores { registry_path, roots, pointers, status }` with `root(key)` and `default_root()` (first healthy store, else first healthy root)
     - `KnowledgeRef { store, path }`
  2. Move the diagnostic type to a neutral `okena_core::diagnostic`, keeping
     `specs::SpecDiagnostic`/`SpecSeverity` as re-exports so no caller or JSON
     changes.
- **Acceptance / witness.** A serde round-trip test of a nested `KnowledgeStores`
  (roots with git state, pointers, entries of every kind). A `default_root`
  test: an unhealthy store is skipped for a healthy project root. The existing
  specs tests pass untouched.
- **Touch points.** `crates/okena-core/src/{knowledge.rs,diagnostic.rs,specs.rs,lib.rs}`.

### WU2 — `okena-knowledge` crate: format, tree, discovery, registry (effort L)

- **Problem.** Nothing reads the ADR-0003 layout.
- **Verify first.**
  - Check whether `okena-markdown` is GPUI-free. If it is, make its
    `split_frontmatter` public and depend on it. If not, keep a small local
    splitter with a comment naming its twin.
  - Confirm `okena-openspec`'s `tree::resolve_document` guard handles symlink
    escapes. Copy its approach rather than depending on the OpenSpec crate.
- **Scope.** A new GPUI-free crate depending on `okena-core`, `serde_yaml_ng`
  and `dirs`.
  - `identity.rs`: parse `.okena-knowledge/store.yaml` (version 1, kebab id via
    `okena_core::specs::is_kebab_id`).
  - `tree.rs`:
    - `read_tree(root)` walks the four kind folders without following symlinked
      dirs and skips dot-dirs.
    - Entries are capped (5 000) with a diagnostic.
    - Handles the title fallback chain, `tags` as a string or a list, a skill's
      supporting files, and template `for:` plus `{placeholder}` scan.
    - `resolve_document(root, rel)` is the path guard.
  - `project.rs`: parse `.okena/knowledge.yaml`. Refuse a `root:` that resolves
    outside the repo.
  - `registry.rs`: `stores.yaml` read, write, register and unregister.
    - Atomic temp+rename write under a process-wide mutex (the daemon is the
      only writer).
    - Never overwrite a corrupt file.
    - One checkout per id, one id per path.
  - `discover.rs`: `discover(&Sources { registry_path, projects, include_projects })`
    returns `KnowledgeStores`.
    - Registered stores are checked for a missing path, not-a-root, and a
      missing identity (warning, id from folder).
    - Project pointers become `used_by`; an unknown id becomes a pointer
      diagnostic.
    - Project roots come from the default or `root:` path.
    - Duplicates are removed by canonical path.
- **Acceptance / witness.** Tempdir sandbox tests (pattern:
  `okena-openspec/src/lib.rs` `testutil`):
  - Identity: valid, missing, bad id, unknown version.
  - Tree:
    - title from frontmatter, from heading, and from file name
    - `tags` string vs. list
    - nested skill with supporting files
    - template flows and variables
    - invalid frontmatter still listed with a warning
    - dot-dir and symlinked dir skipped
    - entry cap reported
  - Guard: `..`, absolute paths and a symlink escaping the root are all refused.
  - Registry: round-trip, corrupt file refuses write, duplicate id refused,
    unregister leaves the checkout.
  - Discovery:
    - missing checkout is unhealthy
    - pointer becomes `used_by`
    - unknown pointer id is diagnosed
    - default and overridden project root work
    - `root: ../x` refused
- **Touch points.** `crates/okena-knowledge/**`, workspace `Cargo.toml`, root
  `CLAUDE.md` crate table (34 → 35 crates).

### WU3 — Store git: status, clone, fetch, pull, setup (effort M)

- **Problem.** No git code exists that clones or fast-forwards, and the
  non-interactive network environment is private to `okena-git`.
- **Verify first.**
  - Move `network_command` and its env test to `okena_core::process` next to
    `command`/`safe_output`. It only sets env vars, so `okena-core` stays free of
    networking. `okena-git` re-exports it, so its fetch/push sites don't change.
  - Confirm `git status --porcelain=v2 --branch` output is stable on the minimum
    git okena supports.
- **Scope.** `okena-knowledge/src/git.rs`:
  - `is_repository_at_root` (same nested-repo reason as OpenSpec's).
  - `status(root) -> KnowledgeGitStatus` (a pure parser over the porcelain
    output, plus the `FETCH_HEAD` mtime).
  - `clone(url, dest)`:
    - refuse an empty url, a leading `-`, or a non-empty dest
    - use `--` before the url
    - remove dest if the clone fails
  - `fetch(root)`.
  - `pull_ff(root)`: fetch, then refuse without an upstream, refuse when
    diverged (message names ahead/behind and the path to resolve in), else
    `merge --ff-only @{u}`.
  - `setup.rs`:
    - create identity plus kind folders with `.gitkeep`
    - refuse inside another repo
    - optional `git init` and one commit scoped to the created paths
    - roll back on failure (mirror `okena-openspec/src/setup.rs`)
- **Acceptance / witness.**
  - Porcelain parser fixtures: no upstream, ahead/behind, detached, dirty count.
  - Integration tests against a local bare repo (no network):
    - clone registers a checkout
    - clone into a non-empty dest is refused
    - a failed clone leaves no dest
    - pull fast-forwards after a new upstream commit
    - diverged pull errors with both counts
    - no-upstream pull is diagnosed
    - setup layout plus commit; setup inside a repo refused
  - The moved env test passes. Tests set a git identity the way
    `d7dd0587` did for branch fixtures.
- **Touch points.** `crates/okena-knowledge/src/{git.rs,setup.rs}`,
  `crates/okena-core/src/process.rs`, `crates/okena-git/src/repository/mod.rs`.

### WU4 — Settings, actions, daemon wiring (effort M)

- **Problem.** The daemon exposes nothing.
- **Verify first.**
  - Find how an off-lock lane gets a quick project snapshot. Take
    `workspace.lock()` only to map projects to `(name, path)` pairs, then release
    it before any git.
  - Confirm `custom_session` is shown as a plain agent session, so WU7 can reuse
    it (`okena-state/src/workspace_data.rs:45`).
- **Scope.**
  - Add `HarnessConfig.knowledge: KnowledgeConfig { projects: bool = true, clone_dir: Option<String> }`.
    The default clone dir is `~/knowledge`, mirroring OpenSpec's `~/openspec/<id>`
    convention.
  - Add client setters in `okena-app-core/src/settings.rs`.
  - Add `ActionRequest` variants:
    - `KnowledgeStores`
    - `KnowledgeTree { root }`
    - `KnowledgeRead { root, path }`
    - `KnowledgeStoreClone { url, path? }`
    - `KnowledgeStoreRegister { path }`
    - `KnowledgeStoreUnregister { id }`
    - `KnowledgeStoreSetup { id, path, remote?, init_git }`
    - `KnowledgeStoreFetch { root }`
    - `KnowledgeStorePull { root }`
  - Add `execute/knowledge.rs` and `execute_knowledge_action`.
  - Run every knowledge action except the draft in one off-lock lane in
    `command_loop.rs`. Even a read runs `git status` per store.
  - Buckets: reads `Fast`; clone, fetch, pull, setup and register
    `LongMutation`.
  - Add `action_dispatch.rs` passthrough arms.
- **Acceptance / witness.** Executor tests in `execute/knowledge.rs` (pattern:
  `execute/specs.rs:547-640`):
  - an undiscovered key is refused
  - a read outside the root is refused through the action
  - a document over the cap is refused
  - worktrees and every session kind are skipped as project sources
  - the default root is the first healthy store
  - register → stores lists it → unregister → gone, with the checkout still
    on disk
- **Touch points.** `crates/okena-core/src/api.rs`,
  `crates/okena-workspace/src/settings.rs`,
  `crates/okena-app-core/src/{settings.rs,workspace/actions/execute/{mod.rs,knowledge.rs}}`,
  `crates/okena-daemon-core/src/command_loop.rs`,
  `crates/okena-transport/src/remote_action.rs`,
  `crates/okena-app/src/action_dispatch.rs`.

### WU5 — Knowledge view (effort L)

- **Problem.** The section is a stub.
- **Verify first.**
  - Find the smallest reusable element that renders a `MarkdownDocument` into a
    scrollable div, from `markdown_renderer/` or `okena-files`'s viewer. If only
    the full file viewer can, fall back to plain text like Specs and log the
    deviation.
  - Read `specs_view.rs` for the `load_generation` stale-reply guard and the
    `daemon_id()` rule (`sections.rs:24`).
- **Scope.** `views/harness/knowledge_view.rs` plus `KnowledgeState` on
  `HarnessPane`, loaded in `new()`. The section match becomes exhaustive (drop
  `other =>`).
  - **Left column (280 px).**
    - Root picker: health dot, kind, and a `↑a ↓b •dirty` sync badge.
    - A filter input matching title, name, description and tags.
    - Grouped lists: Docs as a folder tree, then Skills, Agents and Templates,
      with counts; empty groups hidden.
  - **Right pane.**
    - Entry header: title, kind chip, path, tags.
    - Rendered Markdown with its frontmatter card.
    - For a skill, its supporting files, clickable; non-Markdown files open as
      plain text.
  - **Root header.**
    - Name, remote, branch and "fetched 2h ago".
    - **Fetch** and **Pull** buttons (Pull disabled at behind = 0).
    - **New with agent** (WU7) and **Settings**.
    - Errors show inline.
  - **Empty state.** "Add your team's knowledge repo" opens Settings → Knowledge.
- **Acceptance / witness.**
  - Pure unit tests for `group_entries` (docs nest by folder; kinds in fixed
    order; stable sort) and `entry_matches` (case-insensitive over
    title/name/description/tags; empty filter matches all).
  - Nothing else in rendering gets tests (UI wiring, per
    `docs/reference/testing.md`).
  - Manual: the Goal's walkthrough runs against a real repo with `cargo run`.
- **Touch points.**
  `crates/okena-app/src/views/harness/{mod.rs,sections.rs,knowledge_view.rs}`,
  `crates/okena-core/src/harness.rs` (blurb only, if the wording changes).

### WU6 — Settings → Knowledge page (effort M)

- **Problem.** There is nowhere to add a store.
- **Verify first.** Read how `render_specs.rs` posts store actions and refreshes,
  and how `open_settings("specs")` resolves a slug (`categories.rs`).
- **Scope.**
  - Add a `SettingsCategory::Knowledge` variant (label, `all`, slug `knowledge`).
  - `render_knowledge.rs`:
    - **Stores list:** id, path, remote, health, sync, and Unregister (the
      checkout stays; the page says so).
    - **Clone from URL:** url, plus a destination defaulting to
      `<clone_dir>/<repo name>`.
    - **Add existing folder.**
    - **Create new store:** id, path, remote, init git.
    - **Discovery:** a projects toggle and a clone dir input.
- **Acceptance / witness.** No automated tests. This is UI wiring over WU4
  actions that are already covered there. Manual: clone, register, create and
  unregister each show up in the harness view after a refresh.
- **Touch points.** `crates/okena-app/src/views/overlays/settings_panel/{categories.rs,mod.rs,render_knowledge.rs}`.

### WU7 — Write into a store with an agent (effort S/M)

- **Problem.** Authoring is out of the UI by decision, but a store needs a way
  in besides a hand-opened terminal.
- **Verify first.** Check how `draft_change` creates its session project, whether
  it runs under the workspace lock, and whether its marker and project-creation
  hooks path can be reused as-is (`execute/specs.rs:383`).
- **Scope.**
  - `ActionRequest::KnowledgeDraft { root, request, agent_command? }` on the
    normal (locked) path, `LongMutation` bucket.
  - Refuse an unhealthy root and an empty agent command. Nothing gets
    scaffolded, so a session with no agent has no purpose.
  - Create a session project at the root, marked `custom_session`.
  - Launch through `prompt_args` plus `agent_mcp::injection_args`.
  - The brief states the ADR-0003 layout and frontmatter rules inline and tells
    the agent to read the existing entries first. It also says to work on a
    `knowledge/<slug>` branch, commit when done, and not push unless asked.
  - Add a "New with agent" button in the view.
- **Acceptance / witness.** Executor tests:
  - the brief contains the layout conventions, the branch rule and the request
  - an unhealthy root is refused
  - an empty agent command is refused with a message saying to configure one
  - the session project is created at the root path with the marker set
- **Touch points.** `crates/okena-core/src/api.rs`, `execute/knowledge.rs`,
  `okena-transport/src/remote_action.rs`, `action_dispatch.rs`,
  `views/harness/knowledge_view.rs`.

### WU8 — Docs (effort S)

- **Problem.** Store authors need a format reference that doesn't require reading
  Rust.
- **Verify first.** None.
- **Scope.**
  - New `docs/reference/knowledge.md`: the layout, frontmatter per kind, the
    entry address, `.okena/knowledge.yaml`, the registry, sync behaviour, and
    what okena never writes. Written for someone setting up an org repo.
  - A Knowledge section in `docs/reference/configuration.md`
    (`harness.knowledge.*`).
  - A crate row in the root `CLAUDE.md`.
  - `reference/README.md` index.
- **Acceptance / witness.** Every key in `KnowledgeConfig` appears in
  `configuration.md`, and every kind and frontmatter field `tree.rs` reads
  appears in `knowledge.md`.
- **Touch points.** `docs/reference/{knowledge.md,configuration.md,README.md}`,
  `CLAUDE.md`.

## Out of scope (explicit)

- **Handing knowledge to agents at launch**, and MCP tools to list and read it:
  [backlog 04](../backlog/04-knowledge-context-at-launch.md). It needs this
  sprint's entry addresses and discovery first.
- **Template-driven prompts:** [backlog 05](../backlog/05-prompt-templates-from-knowledge.md).
  This sprint only lists templates and their `for:` flows; flow ids and variable
  sets are decided when the flows are wired.
- **In-app editing, commit, push, branches or PRs.** Decided for this sprint:
  read plus sync, with authoring through an agent (WU7).
- **Background or periodic fetch.** Manual Fetch first. Add one if "behind"
  proves stale in practice.
- **Full-text search across stores.** The filter covers
  title/name/description/tags. Content search can reuse `SearchPathContent`
  later.
- **Web and mobile clients.** The wire types are client-neutral, so they can
  follow without daemon changes.
- **A keybinding** to open Knowledge directly (`ShowHarness` always opens Tasks).
- **Exporting a store as a Claude Code plugin manifest.** ADR-0003 keeps that
  possible without moving files.

## Decisions

- Store format, registry location, project config, and git-through-binary:
  [ADR-0003](../decisions/0003-knowledge-stores.md).
- Every knowledge action except the draft runs off the workspace lock. A read
  still runs `git status` per store, and a subprocess must not stall every other
  action.
- The registry is its own daemon-written file, not `settings.json`, so store
  mutations can run on the blocking pool (see the `SetSettings` ref above).
- Knowledge docs render as Markdown, unlike Specs' plain text.
- The draft agent reuses the `custom_session` marker. A dedicated marker waits
  until something needs to list knowledge sessions apart from other sessions.
- ~~`network_command` moves to `okena_core::process`~~ — superseded in WU3:
  `okena-knowledge` depends on `okena-git` and uses its existing clone, fetch,
  dirty-check and upstream helpers, so `network_command` stays private there
  (see Run log).

## Sequencing

| Order | WUs | Notes |
|---|---|---|
| 1 | WU1 | Everything else speaks these types |
| 2 | WU2 ∥ WU3 | Disjoint files; WU3's `network_command` move touches `okena-core`/`okena-git` only |
| 3 | WU4 | Needs WU2 + WU3 |
| 4 | WU5 ∥ WU6 ∥ WU7 | Disjoint UI files; WU7's action lands in `execute/knowledge.rs` after WU4 |
| any | WU8 | `knowledge.md` can be written alongside WU2 since the format is fixed; config section after WU4 |

## Run log

<!-- Append as you work: discoveries, deviations, blockers. Graduate each entry:
     changed the *why* → ../decisions/NNNN ; new future work → ../backlog/NN ;
     transient → leave it (dies with the sprint on archive). After graduating,
     trim to a one-line pointer ("→ ADR-0007"). -->

- **WU1** — `KnowledgeGitStatus.dirty` is a `bool`, not a count: okena-git's
  dirty check answers yes/no, and a count would cost a second status walk for
  what is only a badge.
- **WU2** — Added `okena_core::fs` (`canonical`, `expand_home`,
  `write_atomically`) and moved `okena-openspec` onto it instead of giving
  `okena-knowledge` a fourth private copy. Private copies remain elsewhere
  (`~` expansion in `okena-workspace`, `okena-ext-claude`,
  `okena-views-sidebar`, `execute/tasks.rs`; atomic writes in
  `okena-workspace/src/sessions.rs`, `okena-remote-server/src/tls.rs`) —
  transient, dedupe when next touched.
- **WU2** — Discovery counts entries without opening files
  (`tree::count_entries`); only `KnowledgeTree` reads file heads, so a stores
  refresh stays cheap with many stores.
- **WU3** — Deviation: `okena-knowledge` depends on `okena-git` and reuses
  `clone_repository`, `validate_clone_url`, `clone_dir_name`, `fetch_all`,
  `has_uncommitted_changes` and `get_repo_common_dir`. New in okena-git:
  `repository/upstream.rs` (`current_upstream`, `fast_forward_to_upstream`,
  `origin_url`) and `repository/init.rs` (`is_repository_at_root`,
  `has_commit_identity`, `init_repository`, `commit_paths`). No porcelain-v2
  parser, no `network_command` move.
- **WU3** — Cloning a repository that is not a knowledge store keeps the
  checkout and fails with "Cloned into …, but it can't be added", instead of
  registering an unhealthy store.
- **Pre-existing** — `okena-transport`'s `action_url` is dead code under the
  `blocking-http`-only feature set okena-git uses, so
  `cargo clippy -p okena-git -- -D warnings` fails at HEAD. Lint with
  `--no-deps`. Not touched here.
- **WU4** — `KnowledgeStores` and `KnowledgeTree` use the `Search` timeout
  bucket, not `Fast`: a listing runs `git status` in every store and a tree
  reads every entry's head.
- **WU4** — `okena-remote-server` embeds `web/dist` through `RustEmbed`, so
  linting `okena-app` or `okena-daemon-core` locally needs that folder. An empty,
  gitignored `web/dist` was created for the lint; CI builds the real one with
  bun. Transient.
- **WU5** — The document pane renders Markdown block by block through
  `MarkdownDocument::render_node`, without selection: the file viewer's
  selectable wrapper is internal to `okena-files`. Selecting and copying text in
  a knowledge doc is not supported yet.
- **WU5** — The harness section dispatch is exhaustive now; `render_stub` is
  gone, and `HarnessSection::blurb()` is only used by its own test.
- **WU6** — The Specs settings helpers (`text_input`, `badge`, `muted`,
  `muted_row`, `path_line`, `banner`, `diagnostic`, `labeled_input`) are
  `pub(super)` and shared with the Knowledge page instead of being copied.
- **WU7** — The draft form lives on `HarnessPane::knowledge_draft`
  (`views/harness/knowledge_draft.rs`) and reuses the Specs form's
  `choice_chip`, `field_label`, `field_hint` and `AGENT_CHOICES`, now
  `pub(super)`. The executor's brief and its no-agent refusal are unit-tested;
  creating the session project is not, since it needs a terminal backend — the
  same gap `SpecDraftChange` has.
