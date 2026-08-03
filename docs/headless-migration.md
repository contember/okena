# Full Headless Mode — Migration Roadmap

Status doc for the migration from Okena's single-process "local + mirrored-remote"
architecture to a **two-process** model: a headless **daemon** that owns all state,
PTYs, logic and persistence, and **thin UI clients** (desktop, web, mobile, remote)
that speak a single protocol over a local socket. The in-process "local" branch is
deleted at the end.

This is the team-facing execution plan. The decision memo (why two processes, what
was rejected) lives in the plan-mode artifact; this doc is the *how* and *in what
order*.

---

## 1. Goal & end state

- **One architecture.** No more parallel "local" and "remote" code paths. There is
  the daemon (authoritative) and clients (views). Local projects and remote
  projects render through the **same** machinery.
- **Daemon is GPUI-free** (user-fixed decision — not merely windowless).
- **A standalone, GPUI-free daemon binary is a first-class shippable artifact.**
  `okena-daemon` must be buildable and runnable on its own, with **zero gpui in its
  dependency graph** — so it can run on a headless server / CI box / container that
  has no windowing stack at all, and a desktop/web/mobile client connects to it
  remotely. This is stricter than "the daemon process doesn't open a window": it
  means gpui must not be *linked* into the binary. The falsifiable gate is
  `cargo tree -i gpui -p okena-daemon` returning nothing.
- **Desktop runs as two processes**: the first UI spawns a local daemon and
  connects to it over loopback; the daemon dies with the last UI (UI-owned
  lifecycle, user-fixed decision).
- **First-class clients**: desktop, web, mobile and remote are all thin clients of
  the same protocol. View/focus state is client-owned; everything authoritative is
  daemon-owned.

The **same** `okena-daemon` binary serves two deployment modes:

| Mode | Invocation | Lifecycle | Transport |
|---|---|---|---|
| **Local UI-owned daemon** (desktop) | spawned by the first UI as `okena-daemon --listen 127.0.0.1` | dies with the last UI | loopback TCP, TLS off |
| **Standalone headless server** | run manually, e.g. `okena-daemon --listen 0.0.0.0` on a server/CI/container | long-running, independent | TLS on, paired clients connect remotely |

The standalone-server mode already *mostly works today* via `run_headless()` +
`--listen` + TLS (`src/main.rs:294`, `crates/okena-remote-server/src/tls.rs`). What's
missing is exactly the GPUI-free packaging — see Phases E/F.

End-state process split:

| Process | Owns |
|---|---|
| **Daemon** (`okena --headless` during strangler, then the standalone `okena-daemon` binary) | `Workspace` (authoritative), `PtyManager`, `execute_action`, `ServiceManager`, hooks, git watcher, persistence + instance lock, the HTTP/WS server. **No gpui linked.** |
| **GUI** | `WindowView` / `ProjectColumn` / layout views, a **mirror** `Workspace` (read-only projection via `apply_remote_snapshot`), per-window focus state, the remote-client state machine. No PTYs, no `execute_action`, no persistence. |

### 1b. The split rule: DATA vs PRESENTATION (not local vs remote)

The boundary between daemon and client is **data vs presentation**, not "local vs
remote":

- **Daemon owns DATA**: projects, layout *as data* (the tree, not pixels),
  terminals + PTYs, git status, services, and **persisted config including the
  theme *preference***.
- **Client owns PRESENTATION**: rendering, *applying* the theme (gpui colors,
  fonts), focus, window geometry, and — for the CLI — output formatting.

The protocol carries **data**; each client renders it its own way. Consequences:

- **Theme**: the daemon stores the *preference* (a string/enum in `settings.json`,
  broadcast to clients) but never renders — the GUI applies gpui colors, the CLI
  ignores it. (This is exactly why `okena-theme`'s data is gpui-free while the gpui
  conversions are behind the `gpui` feature.)
- **CLI**: just another thin protocol client — it gets a `StateResponse` (data) and
  formats plain text itself. No "UI-specific" thing crosses the wire pre-rendered.
- A client decides **per request**: a presentation concern (theme, focus, display)
  it handles locally; a workspace concern (create terminal, git diff) it sends to
  the daemon. No second "intercepting" server is needed.

### 1c. Remotes: Model A — the UI is the aggregation hub (chosen)

Okena aggregates local + multiple remote daemons in one sidebar (unlike VS Code,
where one window = one backend). So we must choose who aggregates. **Decision:
Model A — the UI is the hub.**

- The UI connects directly to its **local daemon** (loopback, for local projects)
  **and** to each **remote daemon** (for remote projects), all over the same
  protocol. "Local" is just *a connection to 127.0.0.1*. The UI's existing
  `RemoteConnectionManager` already does the multi-connect; the local-daemon
  connection is the only new piece.
- **The local daemon handles only its own machine** (local projects + their
  PTY/services/git). It does NOT connect to or proxy remotes — that keeps the
  daemon simple and remote PTY at one hop (remote→UI, no double-hop relay).
- Trade-off accepted: a mobile/web/CLI client connected to the local daemon sees
  only that machine's projects, not the remotes the desktop UI aggregates.

This is **not a one-way door**: because everything speaks the same protocol, the
remote-connection-manager can later move *into* the daemon (Model B — daemon as a
gateway/aggregator visible to all clients, remotes persisting across UI restarts)
behind the same protocol, if/when that property is wanted. Model A is chosen now
for least change + best remote-PTY performance; Model B is the eventual option, not
a prerequisite.

---

## 2. Why this is tractable: the seam already exists

The "remote mode" already *is* the daemon/UI split, fully built and battle-tested.
The migration is largely **pointing the existing remote-client machinery at a local
daemon** and then deleting the in-process shortcut.

What already exists and is reused unchanged:

- **Snapshot reconciliation** — `apply_remote_snapshot()`
  (`crates/okena-workspace/src/remote_apply.rs:55`) materializes a `StateResponse`
  into `WorkspaceData` (projects, layouts, git, terminals), merging client-owned
  visual state. Pure, no GPUI. **No change needed.**
- **Generic thin-client state machine** — `RemoteClient<ConnectionHandler>`
  (`crates/okena-transport/src/client/connection.rs:53`): auth, `GET /v1/state`,
  subscribe, binary frame reader, state-changed diffing. Parameterized by handler;
  desktop and mobile already use it.
- **Desktop thin-client handler** — `DesktopConnectionHandler`
  (`crates/okena-remote-client/src/connection.rs:16`) creates `Terminal` objects
  backed by `RemoteTransport` and feeds raw PTY bytes to the per-pane alacritty
  parser.
- **Remote action dispatch** — `ActionDispatcher::Remote`
  (`crates/okena-app/src/action_dispatch.rs:219`): visual-only actions stay
  client-side; everything else is `POST /v1/actions`.
- **Provider abstraction** — `GitProvider` with `LocalGitProvider` /
  `RemoteGitProvider` (`crates/okena-views-git/src/diff_viewer/provider.rs:57,151`);
  blame mirrors this. The daemon becomes the "remote".
- **Binary frame protocol** — `crates/okena-core/src/ws.rs:77` (`PROTO_VERSION=1`,
  `FRAME_TYPE_PTY=1` / `SNAPSHOT=2` / `INPUT=3`).
- **Reference zero-GPUI client** — `okena-mobile-ffi` proves the protocol is
  sufficient for a client with **zero** deps on gpui/workspace/PTY.
- **Headless host** — `run_headless()` (`src/main.rs:294`) + `HeadlessApp`
  (`crates/okena-app/src/app/headless.rs:34`) already run the whole stack windowless
  on `gpui_platform::current_platform(true)`. **This is the daemon, today.**

The migration's hard part is therefore **not** "build a daemon" — it's "make local
projects ride the remote rails" + "remove GPUI from the daemon" + "delete the old
rails".

---

## 3. Current status (done)

| Increment | Commit | What |
|---|---|---|
| **Phase 0 — spike + full action-layer migration** | `9ae348f4` | `WorkspaceCx` reactor trait (`notify`/`refresh_views`) in `crates/okena-workspace/src/context.rs`. Whole action/state layer of `okena-workspace` converted from `&mut Context<Workspace>` to `&mut impl WorkspaceCx` **except** the hook chain (which needs `&App` for `HookMonitor`/`HookRunner` globals — deferred to Phase E). Non-breaking: `Context<'_, Workspace>: WorkspaceCx`, so every existing caller still compiles. 294/294 tests green. No `as`/`unsafe`/downcast. |
| **Phase 1a — shared local toolkit** | `f6b1e812` | `okena_remote_server::local`: `discover()` / `running_daemon()` (parse `remote.json`), `is_process_alive()`, `mint_local_token()` (local-trust via `remote_secret`). CLI `register` DRYed onto it. |
| **Phase 1b — spawn/wait primitives** | `36d580b7` | `spawn_daemon()` (`--headless --listen 127.0.0.1`, caller owns the `Child`) + `wait_until_ready()` (poll `remote.json`, skip stale pid). Toolkit complete: discover + mint + spawn + wait. |
| **Phase 1c — ensure_local_daemon** | `0b954560` | `ensure_local_daemon_in()` orchestration (discover-or-spawn → mint → `notify_auth_reload`), `EnsuredDaemon` (only the spawner kills its `Child`). |
| **Phase E — gpui-optional crate track** | `dffc5244` `7dfece2c` `8f705eca` `73e474a4` `f723f8a0` `06e1d0f1` `260572fd` `10ae40d8` `470648a1` `b7f45ecd` `56b64515` | Made `gpui` an optional feature across the daemon dependency tree (hooks → workspace → services → app-core → theme → files → remote-server), extending `WorkspaceCx` with the hook accessors and adding the `ServiceCx`/`ServiceHandle`/`ServiceAsyncCx` trait family so the action + service layers run reactor-agnostic. Milestone: the entire daemon graph compiles **gpui-free** (`cargo tree -i gpui` empty for each crate). No `as`/`unsafe`/type-hacks. |
| **Phase E — `okena-daemon-core`** | `d3f36d25` `0817c5a5` `d6dc1841` `076d80de` `1333940a` `760265a6` | New gpui-free crate: `DaemonReactor` + tokio impls of the reactor traits; the observer reactor (autosave / `state_version` / service-sync, re-entrancy-guarded); the PTY event loop; the git-status poller; `daemon_config` (gpui-free settings/theme handlers); `daemon_command_loop` (gpui-free port of `remote_command_loop`); and `DaemonCore::{new,run}` wiring the `RemoteServer` + reactor + all loops inside a `LocalSet`. 24 tests; gpui gate empty. |
| **Phase F — `okena-daemon` binary** | `48e77591` | Standalone, 100% gpui-free headless server binary wrapping `okena-daemon-core` (mirrors `run_headless` without GPUI). `cargo tree -i gpui -p okena-daemon` is **empty** — the standalone-binary goal is met. Smoke-verified end-to-end (isolated `XDG_CONFIG_HOME`): boots → loads settings/workspace → dtach auto-detect → dual-stack TLS server + pairing code + `remote.json` → observer reactor fires on the `LocalSet` (`load_project_services`) → `/health` ok → after pairing, `/v1/state` returns a full `StateResponse` (the whole `RemoteServer → bridge → daemon_command_loop → GetState` path). |
| **Phase A — `--daemon-client` mode** | `d709c83a` | Desktop flag (OFF by default → classic behavior byte-for-byte unchanged). ON: `main.rs` skips `acquire_instance_lock()`, starts from `WorkspaceData::empty()`, and calls `ensure_local_daemon()`; `Okena::new` takes `Option<EnsuredDaemon>`, registers an implicit trusted loopback `RemoteConnectionConfig` (id `local-daemon`, tls off) via `add_connection`, holds the spawned `Child` (killed on `on_app_quit` — only the spawner kills), and makes the autosave observer inert (§5). Loopback connection never reaches persisted settings. Build + 405 tests green. |
| **Phase B — single-writer to the daemon** | `e2ee4e71` | `DaemonCore::new` acquires `acquire_instance_lock()` as step 0 (before binding a port / writing `remote.json`) and holds the `LockGuard` for the daemon's lifetime. Closes the §5 gap where the dedicated `okena-daemon` binary (preferred by `spawn_daemon`) never locked, so with the `--daemon-client` GUI also skipping the lock, no process owned the profile. Lock is gpui-free. |
| **Phase B/E — lifecycle hooks in the daemon** | `ea2ffc34` | `DaemonReactor` now gets a real `HookRunner::new(backend, terminals)` + `HookMonitor::new()` instead of `None`. All plumbing pre-existed (action layer → `WorkspaceCx::{hook_runner,hook_monitor}` → `DaemonWorkspaceCx`). Hook PTYs register in the shared registry + broadcast over `PtyBroadcaster`, so hook terminals reach clients via the normal remote terminal path. Daemon stays gpui-free. Follow-up: surface `HookMonitor` run status into `StateResponse`. |
| **Phase B — hide loopback connection** | `31af0b4e` | Shared const `LOCAL_DAEMON_CONNECTION_ID`; the implicit loopback connection is filtered out of the sidebar's REMOTE management section (its projects already render as ordinary flat-list projects via `apply_remote_snapshot`, which mirrors the daemon's project/folder structure 1:1). Confirmed: no separate "remote workspace" / chrome — Phase B's feared view-UX question was a non-issue. |
| **Phase D1 — flip default to daemon-client** | `662d311a` | The `--daemon-client` flag is gone: the GUI is **always** a thin client. Startup: the `--headless` branch (the daemon) takes the instance lock; the GUI path never locks, starts from `WorkspaceData::empty()`, and unconditionally `ensure_local_daemon()`s. |
| **Phase D2 — delete in-process local PROJECT path** | `a56c399b` | Every project is daemon-backed now, so the local path is dead. Deleted `ActionDispatcher::Local` (+ its dispatch/split/tab arms + the local branch of `dispatcher_for_project`), `create_local_column`, and the Local branches of `build_git_provider`/`build_project_fs`/`build_blame_provider`. The remote path is unconditional. −293 net. |
| **Phase D3 — delete the GUI's in-process infrastructure** | `885e0e76` | Removed from `Okena`: the self-hosted `RemoteServer` (11 fields + start/stop + bridge + command loop + resolvers — external clients connect to the daemon, Model A), the in-process `GitStatusWatcher`, `WorktreeSyncWatcher`, the GUI `HookRunner` global, the in-process `ServiceManager` field + sync observer, and the autosave observer (the daemon is the sole writer). `Okena::new` drops `listen_addr` and takes a non-Option `EnsuredDaemon`. KEPT: the `terminals` registry, `HookMonitor` (client renders hook UI from it), the daemon-shared `sync_services`/`observe_project_services`, and the whole daemon path (`run_headless`/`HeadlessApp` still starts its server + `remote_command_loop`). −705 net. |

Branch: `refactorx/full-headless`. My commits are atomic and listed above; the
unrelated `M` working-tree entries + untracked `profile.json.gz` are not ours —
leave them.

**Daemon-core is complete and the standalone gpui-free binary ships.** The
desktop now also runs as a thin client via `--daemon-client` (spawns a daemon,
mirrors its projects over loopback), with the daemon as the single writer and
lifecycle hooks live.

### StateResponse data parity — verified (static builder diff)

A field-by-field comparison of the GUI's `StateResponse` builder
(`app/remote_commands.rs` + `app/extras.rs::build_api_windows`) against the
daemon's (`daemon-core/command_loop.rs` `GetState` arm) confirms **the core data
is at full parity**: `ApiProject` (id/name/path/visibility/**layout+sizes**/
terminal_names/git_status/folder_color/**services**/**worktree_info**/worktree_ids),
`ApiFolder`, `ApiServiceInfo`, `ApiWorktreeMetadata`, `ApiGitStatus`,
`project_order`, and `visible_project_ids` are all populated identically.

The only daemon-stubbed fields are **per-window presentation state**:
`focused_project_id`, `focused_terminal_id`, `fullscreen`, `bounds`,
`sidebar_open`, `folder_filter`, and the multi-window `windows` list. These are
**client-owned** in the data/presentation split (§1b) — the desktop client has
real GPUI windows and tracks its own focus/bounds/sidebar locally, so the daemon
correctly does not dictate them. The single genuine (minor) gap is **restoring**
per-window UI state (sidebar_open / folder_filter / os_bounds / panel heights)
across a *client restart* in daemon-client mode, since the client no longer
loads `workspace.json` itself — a Phase C round-trip item, low priority.

## 3b. Architectural goal: MET

After Phases A→D the **migration's core goal is achieved**: there is one
architecture — the daemon (authoritative) and thin clients (views). The desktop
GUI is always a client of a local daemon it spawns; the in-process "local
project" path and the GUI's serving/owning infrastructure are deleted. The
daemon ships both as the standalone gpui-free `okena-daemon` binary and as the
GPUI-headless `run_headless`/`HeadlessApp` (the spawn fallback).

**Rollback:** annotated tag `pre-daemon-flip-2026-06-25` (commit `a45aacbb`) is
the return point before the irreversible Phase D deletions.

**Verification done blind:** every commit above is `cargo build`/`cargo test`
green (whole workspace builds with zero new warnings; okena-app 56, smoke 5,
daemon-core 24, workspace 294; daemon gpui-free gate empty). The **runtime**
behavior of the daemon-client desktop (opening a real window, rendering mirrored
projects, live terminals) was **not** verifiable headless — it needs a
run-capable session. Test with `cargo run`; if the daemon-client path misbehaves,
`git checkout pre-daemon-flip-2026-06-25`.

## 3c. Remaining follow-ups (runtime-validated — close in a live session)

These do not block the architecture; they are last-mile completeness/parity that
genuinely needs a running app to get right:

1. **Terminal scrollback on (re)attach.** Pre-existing remote-protocol limitation
   (the SNAPSHOT frame replays the viewport, not history) — affects all remote
   clients, not just daemon-client. Matters on reconnect to an existing daemon
   session; the primary "create terminals live" flow is unaffected.
2. **Per-window presentation restore** (sidebar_open / folder_filter / os_bounds /
   panel heights) across a client restart — client-owned presentation that the
   client no longer persists locally. Decide: client-side persistence vs. daemon
   round-trip.
3. **`HookMonitor` run status** into `StateResponse` for the client hooks panel.
4. **Soft-close-on-quit flush.** `DaemonCore` still has no shutdown/`Drop` hook,
   so a soft-closed terminal's persistent (dtach/tmux) session could outlive the
   daemon if the daemon exits during the grace window.
5. **`worktree_removed` hook + background removal.** The daemon removes the
   worktree synchronously and does **not** fire `worktree_removed` (matches the
   normal `RemoveWorktreeProject` action). If wanted, add both to
   `remove_worktree_project` so both entry points share them.
6. **Claude env live refresh.** `CLAUDE_CONFIG_DIR` is resolved once at daemon
   startup. Hook a `set_extra_env` re-call into the daemon's settings-update path
   if live re-sync is desired after `claude-code.config_dir` changes.

**Key spike conclusions carried forward:**
- The action layer needs only `notify`/`refresh_views` — **no `spawn` on the trait.**
- The only real residual GPUI coupling is `&App` for the global hook services
  (`HookMonitor`/`HookRunner`). That, plus the autosave/`state_version`/git/services
  observers, is the entire content of the GPUI-free extraction (Phase E).

## 3d. GUI PTY loop retired; terminal lifecycle ported to the daemon (2026-06-26)

This closes §3c follow-up #1: the GUI's in-process `pty_manager` +
`start_pty_event_loop` are **deleted**. The desktop client now owns **no** local
PTY machinery. The dead loop turned out to be the *sole* home of several
terminal-lifecycle behaviors (it never ran in daemon-client mode — nothing fed
the GUI's `pty_manager` — so they were silently inert). Each was ported to its
correct owner **first**, then the loop was removed.

| Behavior | Owner (data vs presentation) | Where it landed |
|---|---|---|
| OSC 52 clipboard *reads* | **client** (the clipboard is on the client machine) | wired into the remote activity pump (`RemoteManagerEvent::TerminalActivity` → `process_clipboard_reads`); the reply rides the terminal's `RemoteTransport` back to the daemon PTY |
| hook-terminal exits (status + pending worktree close) | **daemon** | `daemon-core/pty_loop.rs` `handle_hook_terminal_exits` via `DaemonWorkspaceCx` |
| `terminal.on_close` hooks | **daemon** | `fire_terminal_on_close_with_services` (new gpui-free split of `fire_terminal_on_close`) |
| OSC `__okena_hook_exit:<code>` title detection | **daemon** | `daemon-core/pty_loop.rs` data path |
| command-finished activity (OSC 133 ;D → `last_activity_at`) | **daemon** | `bump_activity` on the dirty terminals' owning projects |
| `CLAUDE_CONFIG_DIR` per-PTY env | **daemon** | shared gpui-free `okena-workspace::claude_env`; the daemon `set_extra_env`s its own `PtyManager` |
| worktree removal on hook-close (`git worktree remove` + hooks) | **daemon** | converged on the canonical `remove_worktree_project` (same path as `RemoveWorktreeProject`) |

Also done this session: deleted the dead `Local{Git,Blame}Provider` /
`LocalProjectFs` (no instantiation sites remained — every project rides the
`Remote*` providers), and pointed detached-terminal windows at the project's
`RemoteTransport` instead of the GUI's empty `pty_manager`.

**Residual daemon-side gaps (flagged, *not* regressions — all were already inert
in the dead GUI loop):**
1. **Soft-close-on-quit flush.** Still needs a daemon shutdown hook.
2. **`worktree_removed` hook + background removal.** If wanted, add both to
   `remove_worktree_project` so both entry points share them.
3. **Claude env live refresh.** Re-run `PtyManager::set_extra_env` from the
   daemon settings-update path if live re-sync is desired.

Closed since the original gap list: queued terminal kills are drained after each
daemon workspace action, daemon-originated toasts are forwarded over the stream,
and the GUI no longer owns local PTY machinery.

These need a running app to validate end-to-end (the GPUI client + daemon can't
be exercised headless).

---

## 4. The key sequencing insight (a refinement of the original phase order)

**The daemon can ship as a headless-GPUI process first.** `run_headless` already
runs the full stack windowless. That means we can reach the *architectural goal*
(two processes, one protocol, in-process local path deleted from the GUI) **before**
doing the hardest piece (removing GPUI from the daemon).

This reorders the work versus the original plan (which put GPUI-free extraction
before the flip), and it is strictly safer:

1. Get the desktop running as a thin client of a **headless-GPUI** daemon and make
   it the default (Phases A→D). The user-visible architecture is now "two
   processes, one protocol." Local in-process path is gone from the GUI.
2. **Then** strip GPUI out of the daemon (Phase E) as a pure internal refactor.
   Because clients only speak the protocol, the daemon's internals are swappable
   behind their back — this is the **two-way door** the whole plan was designed to
   create. If GPUI-free hits a wall, we still shipped the headless architecture.

So: **functional two-process split is decoupled from GPUI-free.** We bank the
architecture win early and de-risk the irreversible-feeling part by doing it last,
behind the seam.

Phase letters below (A–F) map to the original Fáze numbers in parentheses.

---

## 5. Strangler invariants (must hold from Phase A until the Phase D flip)

Both paths coexist during the transition. The desktop must support, switchable at
runtime, **(i)** classic in-process local projects and **(ii)** daemon-client local
projects. To keep that honest:

- **Single writer.** Exactly one process owns persistence + the instance lock
  (`crates/okena-workspace/src/persistence.rs::acquire_instance_lock`). In
  daemon-client mode the **daemon** holds it; the GUI's `Workspace` is a pure mirror
  and must **not** autosave (`app/mod.rs:243` autosave observer must be inert in
  client mode).
- **Single PTY owner.** In daemon-client mode the GUI must **not** run
  `start_pty_event_loop` for local projects, **not** instantiate `LocalBackend`,
  **not** run `ServiceManager`/hooks. The daemon does all of it.
- **Single server.** In daemon-client mode the GUI must **not** start its own remote
  server (`app/mod.rs:571`); the daemon is the server. External remote clients
  (mobile, etc.) connect to the **daemon**, which unifies remote access for free.
- **Flag, not fork.** The classic path stays the default until parity (§ Phase D).
  Selection is a single runtime switch, not duplicated call sites.

---

## 6. Phases

### Phase A — Daemon lifecycle + loopback attach  *(Fáze 1c; additive, testable headless)*

**Goal:** desktop startup can discover-or-spawn a local daemon and establish a
loopback client connection to it, using the local-trust token. No rendering change
yet — this only proves the plumbing.

**Steps:**
1. `okena_remote_server::local::ensure_local_daemon()` — orchestrate the toolkit:
   `running_daemon()` → if absent `spawn_daemon()` + `wait_until_ready()` →
   `mint_local_token()` → notify `/v1/auth/reload` (via `okena-transport`
   blocking-http). Returns `{ LocalDaemon, token }`. UI-owned: return the `Child`
   (or a guard) to the caller so the last UI can kill it.
2. Wire `src/main.rs` GUI startup to call `ensure_local_daemon()` and register the
   daemon as a loopback **remote connection** through the existing
   `RemoteConnectionManager` (`crates/okena-remote-client`), TLS off on loopback.
3. Lifecycle guard: hold the spawned `Child` in the `Okena` coordinator
   (`crates/okena-app/src/app/mod.rs`); on last-window-close, terminate it. Don't
   kill a daemon we merely attached to (only the one we spawned), to avoid killing a
   daemon shared with other UIs in future.

**Gate:** desktop boots, spawns/attaches a daemon, the loopback connection reaches
`AuthOk` and pulls a `StateResponse`. Verified by logs + `okena ls` against the
loopback port. Classic local rendering still in force — zero regression.

**Risk/reversibility:** additive, fully reversible. Two-way door.

---

### Phase B — Local projects render via the daemon  *(Fáze 3; behind a dev flag)*

**Goal:** make a **local** project's actions and terminal rendering go through the
daemon over loopback, exactly as a remote project does today. This is where
protocol gaps surface, so it goes behind a `--daemon-client` (or settings) dev flag
first.

**Prerequisite — move the single-writer to the daemon (do this FIRST).** Today the
GUI acquires `persistence::acquire_instance_lock()` at startup (`src/main.rs:497`,
impl `crates/okena-workspace/src/persistence.rs:75`) and owns `workspace.json`
writes. In daemon-client mode the **daemon** must be the sole writer of the profile
(lock + `workspace.json` + autosave); the desktop client must NOT acquire the lock
or write the workspace, or the two clobber each other. Concretely:
1. `DaemonCore::new` acquires the instance lock (hold the `LockGuard` for the
   daemon's lifetime). The daemon already owns autosave (the observer reactor).
2. In daemon-client mode the desktop skips `acquire_instance_lock()` and treats its
   `Workspace` as **mirror-only** (no autosave observer). In classic mode it still
   acquires it. This is gated by the same dev flag as the rest of Phase B.
3. `ensure_local_daemon` (already built, `okena_remote_server::local`) is the
   desktop's entry: discover-or-spawn the daemon, mint a loopback token, connect.
This is the Phase-A2 / instance-lock conflict folded into Phase B — it is the one
hard ordering constraint and must land before the desktop attaches as a client.

**Exact seams (from the desktop architecture map):** the local-vs-remote split is
cleanly keyed on `project.is_remote` + the `ActionDispatcher` enum, so "local
project via daemon" reuses the remote path wholesale. The touch points:
- **Dispatch:** `ActionDispatcher::{Local,Remote}` + `dispatcher_for_project`
  (`crates/okena-app/src/action_dispatch.rs:34,78,94,110,224`). Local projects in
  daemon-client mode take the `Remote` branch (visual actions stay client-side; the
  rest go over the wire).
- **Column/render:** `create_local_column` vs `create_remote_column`
  (`crates/okena-app/src/views/window/mod.rs:744`,`:677`); snapshot apply
  `apply_remote_snapshot` (`crates/okena-workspace/src/remote_apply.rs:55`, via
  `state.rs:892`).
- **Providers (per-project `is_remote` branch):** `build_git_provider` /
  `build_project_fs` / `build_blame_provider`
  (`crates/okena-app/src/views/window/handlers.rs:59,124,171`).
- **In-process pieces that become daemon-only:** `Okena::start_pty_event_loop`
  (`app/mod.rs:660`), the in-process `ServiceManager` (`app/mod.rs:315`) and
  `GitStatusWatcher` (`app/mod.rs:356`) wiring, the conditional self-hosted server
  (`app/mod.rs:571`).

**Mechanism (reuse, don't rebuild):**
- The daemon owns the local project (it added it / loaded it from disk). The GUI
  receives it through `apply_remote_snapshot` like any mirrored project — same
  prefixing (`remote:{connid}:{id}`), same layout/terminal materialization.
- Route the GUI's project actions through `ActionDispatcher::Remote`
  (`action_dispatch.rs:219`) instead of `::Local`. The dispatcher selection
  (`dispatcher_for_project`, `action_dispatch.rs:34`) keys off `is_remote`; in
  daemon-client mode local projects are effectively remote-from-a-local-daemon.
- Terminals render via `DesktopConnectionHandler` + `RemoteTransport` + the per-pane
  alacritty parser — the existing remote terminal path.
- Git/blame use `RemoteGitProvider` against the daemon.

**Key design question to resolve here:** *how does a local project on the daemon get
surfaced to the GUI as a connection-scoped project without the user "pairing"?* The
loopback connection from Phase A is implicit and trusted; local projects on the
daemon should appear in the GUI's default window automatically. Likely: a
"local-daemon connection" that is auto-subscribed and whose projects render in the
main window rather than as a separate remote workspace. This is the main new view-
wiring (`crates/okena-app/src/views/window/mod.rs` snapshot sync,
`create_local_column` → mirror path).

**Gate:** with the flag on, open a local folder → it runs entirely through the
daemon: new terminal, split, type/echo, resize, close, git diff, services all work.
Compare side-by-side with classic mode. Enumerate every gap found (feeds Phase C).
**This phase needs a run-capable session** — the GPUI desktop cannot be fully
verified headless.

**Risk/reversibility:** flagged, default-off → reversible. The flag is the strangler.

---

### Phase C — Protocol parity  *(Fáze 2; iterate with Phase B until no gaps)*

**Goal:** close every gap Phase B surfaces, so daemon-client mode is
indistinguishable from classic mode. Driven by the Phase B gap list, but the known
suspects:

- **Toasts** — forward over WS; UI renders. (`crates/okena-core/src/{api,ws}.rs`,
  `crates/okena-app/src/app/notifications.rs`.)
- **OS notifications** — daemon emits an event; the UI fires the OS notification.
- **Scrollback** — a fetch action / frame so a freshly-attached client can pull
  history, not just live output. (`crates/okena-terminal/src/terminal/*`.)
- **Soft-close & command-palette `InvokeAction`** — today these return errors in
  headless (`app/remote_commands.rs`). Model window/focus in **data**, not GPUI
  windows, so they work without a GUI.
- **Typed schemas** — replace untyped `serde_json::Value` for git/files/settings in
  the wire types with typed structs.
- **Unsynced persistent fields** — promote fields the client needs but that aren't
  mirrored today: `hooks`, `default_shell`, `pinned`, panel heights, per-window
  bounds/widths. (`crates/okena-state/src/workspace_data.rs`, `StateResponse`
  builder in `app/remote_commands.rs`.)

**Gate:** the Phase B gap list is empty; daemon-client mode passes the same manual
checklist as classic for projects/layout/git/services/terminals/scrollback/toasts/
notifications/soft-close/command-palette.

**Risk/reversibility:** additive protocol growth; reversible.

---

### Phase D — Flip the default + delete the in-process local path from the GUI  *(Fáze 5a)*

**Goal:** daemon-client becomes the desktop default (honoring the "flip hned"
decision — default the moment parity holds, not a permanent opt-in). The GUI process
loses its in-process local machinery. **The daemon is still headless-GPUI at this
point** — that's fine and intended.

**Pre-flip:** run the §7 benchmark suite; require no perceptible interactive
regression. Tag/branch as the rollback point (one-way-ish door).

**Delete from the GUI process:**
- `ActionDispatcher::Local` and the `dispatcher_for_project` local branch
  (`action_dispatch.rs:34,110`).
- `create_local_column`'s in-process wiring → only the mirror path remains
  (`crates/okena-app/src/views/window/*`, `views/panels/project_column.rs`).
- The GUI's in-process PTY loop, `LocalBackend` instantiation, `ServiceManager`,
  hooks, git watcher, autosave, and self-hosted remote server
  (`crates/okena-app/src/app/mod.rs` — these stay only in the daemon's `HeadlessApp`).
- The GUI's `Workspace` becomes mirror-only.

**What is *not* deleted:** the shared code itself (`execute_action`, `PtyManager`,
`LocalBackend`, services, hooks) — it now lives and runs **in the daemon**
(`HeadlessApp`, which is `run_headless`). "Deleting the local branch" means removing
the GUI's *ownership* of it, not the code.

**Gate:** desktop runs daemon-client by default; smoke tests (`src/smoke_tests.rs`)
green; classic path removed; one daemon owns persistence/PTYs/server.

**Risk/reversibility:** the deletion is the one-way door → gated on the benchmark and
a parallel-run soak period, with a tagged rollback point. After this, the
architectural goal is **met**: two processes, one protocol.

---

### Phase E — GPUI-free daemon extraction  *(Fáze 4; internal, behind the protocol seam)*

**Goal:** make the daemon's entire dependency tree build with **gpui absent**, not
merely unused. Now safe and reversible because clients only see the protocol — the
daemon's internals are invisible to them. This is the work that turns "headless-GPUI
daemon" into "standalone GPUI-free binary."

**The coupling to remove (grounded inventory, measured on `refactorx/full-headless`):**

| Crate | gpui coupling today | Action |
|---|---|---|
| `okena-remote-server` | 1 file, only `gpui::Global` for the `GlobalRemoteInfo` wrapper | Move/feature-gate the `Global` wrapper out; core server is already gpui-free. Trivial. |
| `okena-hooks` | `HookMonitor` / `HookRunner` exposed as gpui globals (`impl Global`, accessed via `&App`) | Replace global access with a plain accessor owned by the reactor. Low. |
| `okena-services` | `ServiceManager` is a gpui `Entity` that `cx.observe`s the workspace (17× `Context<`, 8× `Entity<`) | Re-host as a plain struct driven by reactor callbacks (no `Entity`/`observe`). Medium — the real work. |
| `okena-workspace` | residual after Phase 0: deferred hook chain (`Entity`/`Context`), `GlobalWorkspace` wrapper, and **`gpui::Point`/`gpui::Pixels` embedded in persistent data** (window bounds, panel widths). (133 `gpui::test` refs are test-only.) | Finish the `WorkspaceCx` migration; replace gpui geometry types in persisted data with plain types in `okena-state`. |
| `okena-app` | irreducibly gpui (it holds the views). `HeadlessApp` currently lives here. | The daemon host must **not** depend on `okena-app` — see step 4 (new crate). |

**Steps:**
1. **Finish the `WorkspaceCx` migration.** The deferred hook-chain methods
   (`project.rs::{add_project,delete_project}`, `worktree.rs` registration chain)
   need `&App` for `HookMonitor`/`HookRunner` globals. Route them through a plain
   service accessor on the reactor instead of a GPUI global.
2. **De-GPUI the observers.** Replace `cx.observe`-driven autosave + `state_version`
   bump + git status + service sync with a plain reactor (tokio + `watch` +
   callbacks). These live in the app/daemon layer, not the action layer (already
   GPUI-free after Phase 0).
3. **Purge gpui types from data.** Replace `gpui::Point`/`gpui::Pixels` in persisted
   `okena-state` types with plain types (the wire schema in `okena-core` already
   avoids gpui — align on those). Move `GitStatusWatcher` out of the
   `okena-views-git` views crate (`watcher.rs`) so the daemon never links a views
   crate.
4. **Make gpui an optional feature** in `okena-workspace`, `okena-services`,
   `okena-hooks`, `okena-remote-server`. The GPUI-backed impls — `Entity`/`Global`
   wrappers, `impl WorkspaceCx for Context<'_, Workspace>`, any `gpui::*` geometry —
   go behind `#[cfg(feature = "gpui")]`. The GUI builds these crates *with* the
   feature; the daemon builds them with `default-features = false`.
5. **New gpui-free host crate** — `crates/okena-daemon-core` holding `WorkspaceCore`:
   the reactor + the already-generic action layer + `PtyManager` + (de-GPUI'd)
   services + hooks + the server. It depends on the daemon-tree crates with gpui
   off, and **not** on `okena-app`. (`HeadlessApp`'s logic moves here; `okena-app`
   keeps only GUI-client code.)

**Gate (this is what makes the standalone binary real):**
- `cargo build -p okena-daemon-core --no-default-features` (or with gpui off)
  succeeds, and `cargo tree -e features -i gpui -p okena-daemon-core` returns
  **nothing**.
- Desktop (still a thin client) sees no change. `cargo test -p okena-workspace`
  green throughout (the Phase 0 gate). Each feature-gated crate builds **both** with
  and without the `gpui` feature (CI matrix).

**Risk/reversibility:** two-way door by construction. If one piece resists de-GPUI,
that crate keeps its gpui-backed impl and the daemon temporarily keeps it gpui-on —
the standalone-binary gate just stays red for that crate until resolved, with no
user-visible impact on the already-shipped two-process architecture.

---

### Phase F — `okena-daemon` binary + final cleanup  *(Fáze 5b)*

**Goal:** ship the standalone, GPUI-free daemon binary — smaller, faster to start,
no windowing libraries linked, runnable on a headless server.

**Steps:**
- New `crates/okena-daemon` **binary** wrapping `okena-daemon-core` (gpui off).
  Supports both deployment modes from § 1 (loopback UI-owned + standalone server).
  `spawn_daemon()` switches from `current_exe --headless` to launching `okena-daemon`.
- Remove the now-dead headless-GPUI scaffolding from the main binary
  (`run_headless`/`HeadlessApp` in the gpui app become unnecessary).
- Final pass: delete leftover dual-path conditionals, dead `cfg`s, unused wiring.

**Gate:**
- `okena-daemon` is the spawned/served process; `cargo tree -i gpui -p okena-daemon`
  returns nothing (the shippable-artifact gate).
- The standalone-server mode is verified end-to-end: run `okena-daemon` on a box
  with no display, connect a desktop/mobile client remotely, exercise
  projects/terminals/git/services.
- Main GUI binary links gpui only for the client; full `cargo build`/`cargo test`;
  benchmark suite re-run.

---

## 7. Performance plan (benchmark-gated, before the Phase D flip)

The user de-prioritized performance as a *driver*, but the flip is gated on no
perceptible regression.

- **Measure:** echo latency (keystroke→render) vs in-process; throughput flood
  (`yes`, `cat 100MB`); snapshot churn under bursts; CPU under multi-MB/s streams.
- **Bar:** no perceptible regression for interactive use.
- **Escalation ladder if it fails:**
  1. Push-not-poll instead of the 8 ms remote-dirty loop
     (`crates/okena-views-terminal/src/layout/terminal_pane/mod.rs:~202`).
  2. State deltas/coalescing instead of full snapshot on every `state_version` bump.
  3. UDS / named-pipe transport (TLS off on loopback) instead of loopback TCP.
  4. Lossless backpressure on the local socket instead of lossy resync.

---

## 8. Risks & mitigations

| Risk | Mitigation |
|---|---|
| GPUI-free depth (the dominant unknown) | Resolved by Phase 0 spike (done, GO). Action layer is already generic; only services/observers remain, and they're done **after** the flip behind the seam. |
| Protocol gaps hidden until desktop-as-client | Phase B (flagged) surfaces them **before** anything irreversible; Phase C closes them. |
| Two writers / two PTY owners / two servers during strangler | §5 invariants: in client mode the GUI is inert for persistence/PTY/services/server. |
| Auto-surfacing local-daemon projects without "pairing" | Implicit trusted loopback connection from Phase A; resolve the view-wiring in Phase B. |
| Cross-platform local socket | Loopback TCP for MVP; UDS/named-pipe only if the benchmark demands it. |
| Killing a shared daemon | Only the **spawner** kills; attachers never do. |

---

## 9. Falsifiability — what would change the plan

- **Phase B reveals the protocol can't represent something essential** without a
  major schema redesign → reconsider whether *all* local state belongs on the wire,
  or keep a narrow in-process fast-path for that one concern.
- **Benchmark shows insurmountable interactive latency** even after push-not-poll +
  deltas → reconsider "two processes always" for the single-machine desktop (the
  decision memo's stated fallback).
- **GPUI-free (Phase E) hits a hard executor-ordering dependency** → daemon ships
  headless-GPUI permanently; we still have the two-process architecture (this is why
  E is last and behind the seam).

---

## 10. Recommended next step

**Phase A (Fáze 1c)** — `ensure_local_daemon()` orchestration + loopback attach at
desktop startup. It is additive, reversible, and unblocks Phase B. Phase B is the
first step that needs a **run-capable session** (the GPUI desktop can't be fully
verified headless), so it's the natural point to switch from "build" to "build +
manually verify in a running app."
