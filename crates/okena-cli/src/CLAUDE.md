# cli/ — Command-line interface

`okena <subcommand>` controls a running instance over the remote HTTP API
(`src/remote/`). Entry point is `try_handle_cli()`, called early in `main.rs`
*before* GUI startup. The gate only engages clap when the first arg is a known
subcommand (or `--help`/`--version`), so a bare launch and the `--profile` /
`--list-profiles` / `--new-profile` flags pass straight through to GUI/profile
handling untouched.

## Files

| File | Purpose |
|------|---------|
| `lib.rs` | The gate + `dispatch(Cli)`. HTTP/token infra: `discover_server`, `ensure_token`, `api_get`/`api_post`, `CliConfig` load/save. |
| `parser.rs` | clap `Cli` parser + `Command`/`*Cmd` subcommand enums. `subcommand_names()` feeds the gate (a test asserts it covers the whole tree). |
| `resolve.rs` | Pure, unit-tested resolvers over a parsed `StateResponse`. No I/O. |
| `commands.rs` | Command implementations — build an `ActionRequest` JSON body, POST it, render the result. |
| `skill.md` | The agent skill, `include_str!`-embedded into `commands.rs` and emitted by `skill show` / written by `skill install`. Keep it concise — it's a reference, not a manual. |
| `register.rs` | First-use token registration (reads the local `remote_secret`). |

## Addressing (agent-friendly)

- **Projects**: exact id, case-insensitive name, or absolute path (canonicalized).
- **Terminals**: a bare terminal id, `<project>/<name>`, or `<project>:<index>` (DFS order). `<name>` matches a `terminal_names` entry first, then falls back to a terminal id scoped to that project (so the id `term ls` shows for unnamed terminals also works after the `/`).
- **Windows** (`--window`): `"main"`, a full id, or a unique id prefix → resolved to the exact id put in the action's `window` field. The flag is `global` but only `project add/show/hide/focus` and `term focus/fullscreen` honor it; `dispatch` warns when any other command receives it. It must come **after** the subcommand (`okena term focus X --window main`) — the gate only engages when `args[1]` is a subcommand, so `--window` *before* the subcommand falls through to GUI launch.
- **Layout `path`** for `term split`/`term tab` is resolved client-side from a terminal id (`resolve_terminal_path`), mirroring `okena_layout::LayoutNode::find_terminal_path` — agents never compute tree paths. `term tab` sends `in_group: false` (wrap-or-join, mirroring the UI), never `true` (which needs `path` to point at a Tabs node).

## Conventions

- Default output is tab-separated (grep/awk friendly); `--json` emits structured JSON. Commands that create things (`term new`, `term split`, `term tab`, `project add`, `worktree add`, `folder add`) print the new id(s) to stdout.
- `ls --json` emits a *structured overview* (windows with visible projects resolved to names + focus, and per-project hidden/git/terminals/layout) — not the raw state. Use `okena state` for the full raw dump.
- `key` accepts the named keys plus a generic `ctrl-<a-z>` chord (serialized as `{"Ctrl":"l"}` → `SpecialKey::Ctrl`). The named `ctrl-c/d/z` stay as dedicated variants for back-compat.
- `service start/stop/restart` validates the service name against the project up front (fail fast) instead of POSTing and polling for a status that never arrives.
- `run --wait` appends a completion marker (`OKENADONE_<pid>:<code>:END` via `printf`), polls `read_content` until it appears, prints the visible output (marker lines stripped) and exits with the command's status. The echoed command line carries the literal `%s`, so the digit-requiring match never false-positives on it. POSIX-shell + non-interactive only. Flags precede the terminal (trailing-var-arg).
- `skill show`/`install` are pure client-side (no server round-trip). `install` defaults to `~/.claude/skills/okena/SKILL.md` (`dirs::home_dir()`); `--project` writes `./.claude/skills/okena/SKILL.md`.
- `settings`, `theme`, and `command` are **app-scoped** actions (`get_settings`/`set_settings`, `get_themes`/`get_theme`/`set_theme`/`save_custom_theme`, `list_actions`/`invoke_action`). They touch globals (`GlobalSettings`/`GlobalTheme`) and windows, so they're handled in `app/remote_commands.rs` (delegating to `app/remote_config.rs`) — NOT `execute_action`, which returns an error for them (exhaustiveness only). `settings set` deep-merges a dotted-key patch and re-deserializes the whole `AppSettings` (validation). `command run` dispatches a named GUI action (`get_action_descriptions()` factory) into a window via `AnyWindowHandle::update` + `window.dispatch_action` — the only remote→GUI dispatch path; unavailable in headless mode.
- Every command maps to an `ActionRequest` snake_case tag (or `GET /v1/state`). Authentication is automatic on first use.
