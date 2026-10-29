# cli/ — Command-line interface

`okena <subcommand>` controls a running daemon over the remote HTTP API
(`crates/okena-remote-server/`). Entry point is `try_handle_cli()`, called early in `main.rs`
*before* GUI startup. The gate only engages clap when the first arg is a known
subcommand (or `--help`/`--version`), so a bare launch and the `--profile` /
`--list-profiles` / `--new-profile` flags pass straight through to GUI/profile
handling untouched.

## Files

| File | Purpose |
|------|---------|
| `lib.rs` | The gate + `dispatch(Cli)`. HTTP/token infra: `discover_server`, `ensure_token`, `api_get`/`api_post_json`/`api_action`, `CliConfig` load/save. |
| `parser.rs` | clap `Cli` parser + `Command`/`*Cmd` subcommand enums. `subcommand_names()` feeds the gate (a test asserts it covers the whole tree). |
| `resolve.rs` | Pure, unit-tested resolvers over a parsed `StateResponse`. No I/O. |
| `commands.rs` | Command implementations — build an `ActionRequest` JSON body, POST it, render the result. |
| `skill.md` | The agent skill, `include_str!`-embedded into `commands.rs` and emitted by `skill show` / written by `skill install`. Keep it concise — it's a reference, not a manual. |
| `register.rs` | First-use token registration (reads the local `remote_secret`). |
| `claude.rs` | `okena claude …`: follow and steer `claude` sessions over the Claude remote client routes (below). |
| `claude_remote.rs` | Hidden `claude-remote` subcommand run by `claude` itself (see below). |

## Addressing (agent-friendly)

- **Projects**: exact id, case-insensitive name, or absolute path (canonicalized).
- **Terminals**: a bare terminal id, `<project>/<name>`, or `<project>:<index>` (DFS order). `<name>` matches a `terminal_names` entry first, then falls back to a terminal id scoped to that project (so the id `term ls` shows for unnamed terminals also works after the `/`). `<project>` may contain `/` (branch-named worktrees, absolute paths): every `/` split point is tried, and a filter that matches terminals under more than one split is an ambiguity error.
- **Windows** (`--window`): `"main"`, a full id, or a unique id prefix → resolved to the exact id put in the action's `window` field. The flag is `global` but only `project add/clone/show/hide/focus`, `worktree add` and `term focus/fullscreen` honor it; `dispatch` warns when any other command receives it. It must come **after** the subcommand (`okena term focus X --window main`) — the gate only engages when `args[1]` is a subcommand, so `--window` *before* the subcommand falls through to GUI launch.
- **Layout `path`** for `term split`/`term tab` is resolved client-side from a terminal id (`resolve_terminal_path`), mirroring `okena_layout::LayoutNode::find_terminal_path` — agents never compute tree paths. `term tab` sends `in_group: false` (wrap-or-join, mirroring the UI), never `true` (which needs `path` to point at a Tabs node).

## Conventions

- Default output is tab-separated (grep/awk friendly); `--json` emits structured JSON. Commands that create things (`term new`, `term split`, `term tab`, `project add`, `worktree add`, `folder add`) print the new id(s) to stdout.
- `ls --json` emits a *structured overview* (windows with visible projects resolved to names + focus, and per-project hidden/git/terminals/layout) — not the raw state. Use `okena state` for the full raw dump.
- `key` accepts the named keys plus a generic `ctrl-<a-z>` chord (serialized as `{"Ctrl":"l"}` → `SpecialKey::Ctrl`). The named `ctrl-c/d/z` stay as dedicated variants for back-compat.
- `service start/stop/restart` validates the service name against the project up front (fail fast) instead of POSTing and polling for a status that never arrives.
- `run --wait` appends a completion marker (`OKENADONE_<pid>:<code>:END` via `printf`), polls `read_content` until it appears, prints the visible output (marker lines stripped) and exits with the command's status. The echoed command line carries the literal `%s`, so the digit-requiring match never false-positives on it. POSIX-shell + non-interactive only. Flags precede the terminal (trailing-var-arg).
- `skill show`/`install` are pure client-side (no server round-trip). `install` defaults to `~/.claude/skills/okena/SKILL.md` (`dirs::home_dir()`); `--project` writes `./.claude/skills/okena/SKILL.md`.
- `settings` and `theme` are **app-scoped** actions (`get_settings`/`set_settings`, `get_themes`/`get_theme`/`set_theme`/`save_custom_theme`). The daemon handles them in `okena-daemon-core/src/daemon_config.rs` (logic in `okena-app-core/src/remote_config.rs`) — NOT `execute_action`, which returns an error for them (exhaustiveness only). `settings set` deep-merges a dotted-key patch and re-deserializes the whole `AppSettings` (validation). `command list`/`command run` (`list_actions`/`invoke_action`) need the GUI action registry, which the daemon lacks: `list` is empty and `run` fails.
- **Desktop presentation goes through a push, not the daemon's state.** Each desktop owns its per-window hidden sets and fullscreen. `project show/hide` (and the `--hidden` follow-up of `project add/clone`), `term fullscreen` and `term focus`/`project focus` update the daemon's synthetic main window; after success the server pushes a one-shot request to every attached desktop, which applies it to the `--window` target or its active window (see `crates/okena-remote-server/src/CLAUDE.md`). `ls` shows the daemon's view, not a desktop's.
- `worktree add` lands the new worktree in one desktop window: the `--window` target (sent as `CreateWorktree.window`; the server pushes a `ProjectPlacementRequested` with the new id after success), else the first window showing its parent (main if none does) — `apply_initial_remote_project_visibility` in `okena-workspace/src/remote_apply.rs`.
- `update` uses the daemon-owned, loopback-only updater endpoints. `update revert` is non-interactive: `--dry-run` previews, `--yes` applies, `--keep-config` opts out of the exact config checkpoint, and `--restart` deliberately ends daemon-owned terminal sessions. Progress stays off stdout; `--json` results remain parseable.
- `claude ls/show/subagents/log/prompt/answer` call the REST routes `/v1/claude/terminals/{id}/...` (`api_get`/`api_post_json`), not `ActionRequest`s; `interrupt` is the `send_special_key` Escape action. `ls` lists `claude_sessions` ∪ `claude_dialogs` from state. Errors print the daemon's plain-text body and exit 1. The `log` cursor is `<session_id>:<offset>`, because the daemon honors an offset only with the session it came from; text-mode records escape `\`, newline and tab so each entry stays one line.
- Every command maps to an `ActionRequest` snake_case tag (or `GET /v1/state`). Authentication is automatic on first use.
- `claude-remote` is hidden from `--help`: Claude Code runs it, not people. `hook permission` long-polls `POST /v1/claude/hook/permission` and prints the daemon's answer when it is a non-empty object; `hook event` posts `/v1/claude/hook/event` (5 s timeout). Both exit 0 and print nothing to stdout on any failure — an error must never block or deny Claude. `channel` is an MCP stdio server: it always answers `protocolVersion` `2025-06-18` (the newer revision cannot send the unsolicited `notifications/claude/channel`), returns `-32601` for unknown requests (including Claude's `server/discover` probe), and after `notifications/initialized` long-polls `/v1/claude/channel/poll` for prompts. Empty polls that return in under 1 s back off like errors (two channel servers on one terminal must not hot-loop); the server exits 0 on stdin EOF or the first failed stdout write. Over the Unix socket no token is sent; over loopback TCP the cached `cli.json` token is sent without the `ensure_token()` pre-check (hooks run on every tool call), and `register()` runs only on a 401, followed by one retry. `main.rs` resolves the profile read-only for this subcommand and skips the start-up bookkeeping writes.
