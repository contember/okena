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
| `mod.rs` | The gate + `dispatch(Cli)`. HTTP/token infra: `discover_server`, `ensure_token`, `api_get`/`api_post`, `CliConfig` load/save. |
| `parser.rs` | clap `Cli` parser + `Command`/`*Cmd` subcommand enums. `subcommand_names()` feeds the gate (a test asserts it covers the whole tree). |
| `resolve.rs` | Pure, unit-tested resolvers over a parsed `StateResponse`. No I/O. |
| `commands.rs` | Command implementations — build an `ActionRequest` JSON body, POST it, render the result. |
| `register.rs` | First-use token registration (reads the local `remote_secret`). |

## Addressing (agent-friendly)

- **Projects**: exact id, case-insensitive name, or absolute path (canonicalized).
- **Terminals**: a bare terminal id, `<project>/<name>`, or `<project>:<index>` (DFS order).
- **Windows** (`--window`): `"main"`, a full id, or a unique id prefix → resolved to the exact id put in the action's `window` field.
- **Layout `path`** for `term split`/`term tab` is resolved client-side from a terminal id (`resolve_terminal_path`), mirroring `okena_layout::LayoutNode::find_terminal_path` — agents never compute tree paths.

## Conventions

- Default output is tab-separated (grep/awk friendly); `--json` emits structured JSON. Commands that create things (`term new`, `term split`, `project add`, `worktree add`, `folder add`) print the new id(s) to stdout.
- Every command maps to an `ActionRequest` snake_case tag (or `GET /v1/state`). Authentication is automatic on first use.
