# okena-remote — Claude remote preload

Connects interactive Claude Code sessions in Okena panes to the Okena daemon, so a
phone or web client can follow the conversation, answer permission dialogs and
questions, and send prompts. Nothing is installed into `~/.claude`.

## How it is loaded

With the `claude_remote` setting on, the daemon writes `preload.js` (embedded in
the binary) to the first usable of:

1. `<profile config dir>/claude-remote/preload.js`
2. `$XDG_RUNTIME_DIR/okena/<profile id>/claude-remote/preload.js`
3. `<user cache dir>/okena/<profile id>/claude-remote/preload.js`
   (`~/.cache` on Linux, `~/Library/Caches` on macOS)
4. `$TMPDIR/okena-<profile id>/claude-remote/preload.js`

Bun splits `BUN_OPTIONS` on whitespace without quoting, so a path with
whitespace is skipped. Every directory Okena creates or reuses there must be
owned by the user and not writable by others. The daemon rewrites the file every 10 minutes if it was
removed or changed. It then gives every new pane:

| Variable | Value |
|---|---|
| `BUN_OPTIONS` | `--preload=<path to preload.js>` (plus any `BUN_OPTIONS` the daemon inherited) |
| `OKENA_BIN` | the `okena` binary the hooks and the channel server run |
| `OKENA_CLAUDE_REMOTE_CHANNEL` | `1` when the MCP channel is on, else `0` |
| `OKENA_PROFILE` | the daemon's profile, so the hooks reach this daemon even from a tmux server another profile started |

`claude` is a Bun-compiled binary, so Bun runs the preload before Claude Code
starts. Panes created before the setting was turned on are not affected.

## What it adds

For an interactive run of the `claude` binary only, the preload appends:

- `--settings <json>` with hooks that run `okena claude-remote hook permission`
  (`PermissionRequest`) and `okena claude-remote hook event` (session, prompt,
  tool, subagent and stop events);
- with the channel on: `--mcp-config <json>` for the `okena claude-remote channel`
  MCP server and `--dangerously-load-development-channels server:okena-remote`.
  Claude asks for confirmation of the development channel at every start.

It leaves argv unchanged for other Bun programs, for `claude` started by
`claude` (`CLAUDECODE` set), for subcommands (`claude mcp …`; any argument
before `--` that is a subcommand name counts), and for runs with
`-p`/`--print`, `--version`, `--help`, `--bg`, `--desktop`, `--cloud`,
`--environment`, `--tmux`, or the user's own `--settings`. A user's own
`--mcp-config` is kept and ours is added next to it.

With the setting off, new panes get `OKENA_BIN` and
`OKENA_CLAUDE_REMOTE_CHANNEL` removed and `BUN_OPTIONS` reset to the daemon's
own value, so a tmux server that kept the old environment is cleaned too.

## Debugging and tests

`OKENA_CLAUDE_REMOTE_DEBUG=1` prints the final argv to stderr when the preload
changes it.

```
bun test integrations/claude-code/okena-remote/
```
