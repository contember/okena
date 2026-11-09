---
name: okena
description: Drive a running Okena terminal multiplexer from the `okena` CLI — run commands in and read output from the user's terminals, and inspect or manage projects, worktrees, their statuses, services and windows. Use whenever you need to act inside the user's Okena terminals instead of your own shell.
---

# Okena CLI

`okena <command>` controls the running Okena app over its local HTTP API
(auth is automatic on first use). This is the high-signal reference — run
`okena --help` and `okena <command> --help` for the full surface.

## Orient yourself first

- `okena ls` — overview of windows, projects and layout (`--json` for structured).
- `okena whoami` — which terminal/project YOU are in (reads `$OKENA_TERMINAL_ID`,
  set inside every Okena terminal). Use it to find your own terminal id.
- `okena term ls [project]` — terminals as `id<TAB>name<TAB>project`.
- `okena usage` — Claude Code / Codex rate-limit usage on the daemon's machine
  (`provider<TAB>status<TAB>window<TAB>percent<TAB>reset_epoch`; `--json` for raw).

## Addressing (how to name things)

- **Project**: id, case-insensitive name, or absolute path.
- **Terminal**: a bare id, `project/name`, or `project:index` (DFS order, 0-based).
  `project/name` also accepts a terminal id (the id `term ls` shows for unnamed ones).
- **Window** (`--window`): `main`, a full id, or a unique id prefix.

## Drive a terminal (the loop)

- `okena run <term> <cmd…>` — type a command + Enter, return immediately.
- `okena run --wait <term> <cmd…>` — run and BLOCK until it finishes; the CLI
  prints the output and exits with the command's status. Flags go BEFORE `<term>`.
- `okena send <term> <text…>` — type raw text, no Enter.
- `okena key <term> <key>` — enter, esc, tab, up/down/left/right, home, end,
  pageup, pagedown, backspace, delete, or `ctrl-<a-z>` (e.g. ctrl-c, ctrl-l).
- `okena read <term>` — the terminal's VISIBLE screen (not scrollback).

```bash
okena run --wait okena:0 cargo test   # run, wait, exit with its status
okena read okena:0                     # see the screen
okena key okena:0 ctrl-c               # interrupt
```

## Drive other Claude sessions

For terminals running `claude` with Claude Remote on (Settings → Terminal).

- `okena claude start <term> [--prompt-file f] [--trust] [--wait] [--timeout s] [--wait-timeout s] [--json] [-- <claude args>]`
  — run `claude` in a terminal at a shell prompt, accept the development-channels
  warning Okena causes, wait until it connects, send the first prompt. Prints
  `terminal`, `session_id`, `delivery`. The folder-trust dialog is accepted only
  with `--trust` (else exit 1; rerun with `--trust` to resume). `--wait` blocks
  until that turn ends: `state idle|dialog`, or `disconnected|timeout` with exit 1.
  Everything after `--` goes to `claude` itself, e.g.
  `okena claude start p:1 --trust -- --dangerously-skip-permissions --model opus`
  or `-- --resume <session-id>`. Quoted for sh/bash/zsh; in other shells (fish,
  cmd, PowerShell) only plain arguments pass. Control characters are refused
  (pass multi-line text via a file flag). Flags that keep Okena out (`--settings`,
  `-p`, `--tmux`, `--bg`, subcommands) end in a timeout that names them. With `--`
  arguments start refuses a pane where a claude already waits on a start-up
  dialog: rerun without them to resume it. Don't type into the start-up dialogs
  yourself.
- `okena claude ls` — `term<TAB>project<TAB>name<TAB>session<TAB>busy|idle|-<TAB>dialogs<TAB>queued<TAB>subagents`.
- `okena claude show <term>` — status lines (`startup_dialog trust|channels|-`:
  a start-up dialog `claude` waits on) plus `dialog<TAB>id<TAB>tool<TAB>tool_input-json` per open dialog.
- `okena claude log <term> [--cursor <c> | --before <o>] [--agent <id>]` — one
  line per entry (`user|assistant|tool_use|tool_result`, newlines escaped as
  `\n`), last line `cursor<TAB><c>`. Pass `<c>` back to get only newer entries;
  a `reset` line means drop what you had. Without a cursor you get the last 200
  entries; an `older<TAB><o>` line means more precede them: `--before <o>`
  prints that older page (with its own `older` line until the start). `--json`
  prints the page instead: `entries[]` (each with `timestamp`), `reset`,
  `next_cursor` to pass back as `--cursor`, and `older_cursor` for `--before`.
  `okena claude subagents <term>` lists `--agent` ids.
- `okena claude find <session-id> [--json]` — the terminal that runs that
  session (exit 1 when none does; known once a hook reported it).
- `okena claude prompt <term> [--session <id>] <text…>` — prints `channel`, `pty`
  or `queued`; exits 1 with the daemon's reason when it cannot be delivered.
  `--session` (before the text) refuses unless the pane still runs that session
  (`session_id` from `start`/`show`), so a prompt never reaches a session that
  replaced yours.
- `okena claude answer <term> <dialog> allow [--always] | deny [--message <m>] | answer --answers '{"<question>":"<label>"}'`
- `okena claude interrupt <term>` — Escape, ends the running turn.

## Manage the workspace

- Projects: `okena project add <path> | clone <url> | rm | rename | color | focus | show | hide`
  (`clone` takes `--into <parent-dir>` (default CWD), `--dir <name>`, `--name <n>`)
- Layout: `okena term new | close | rename | split <h|v> | tab | focus | minimize | fullscreen`
  (`split h` = stacked top/bottom, `split v` = side by side left/right)
- Window layout: `okena window layout <columns|rows|grid> [--min-width px] [--min-height px]`
  (grid flows projects into tiles; the two minimums are app settings for every grid window)
- Worktrees: `okena worktree add <project> <branch> [--new-branch] [--wait] [--layout single|parent] [--name <label>] [--window <w>] | rm`
  (desktops show the new worktree in `--window` only, else in the first window showing the parent;
  `--layout single` = one terminal instead of the parent's layout, `--name` labels the first terminal)
- Statuses (a board column on a project / worktree, e.g. To do, Review, Done):
  `okena status ls [project]` (`name<TAB>column<TAB>column-id` in board order;
  with a project: just its family, parent + worktrees), `status get [project]`,
  `status set <column> [project]`, `status clear [project]`. The project defaults
  to the one YOUR terminal belongs to, so in a worktree `okena status set review`
  marks it. Columns: `status column ls | add <name> [--color c] | rename | color |
  move <column> <pos> | rm | no-status [color] [--position n]`; a column is an
  id or a name (`no-status` prints or sets the No status group's color and
  board-order position). `status
  enable|disable` is the board's switch (statuses only show while it is on);
  `status lanes on|off [project]` stacks the family's columns by status in a
  desktop window, `status lanes on|off --overview` does it for the overview.
- Services: `okena services [project]`, `okena service start|stop|restart <name> [project]`
- Settings: `okena settings show [key] | schema | set <key> <value>` (dotted keys, e.g. `sidebar.width`).
- Theme: `okena theme list | show [id] | set <id> | save <id> <json>`. To recolor
  ("make it lighter"): `theme show` the active theme, edit the colors, pipe the
  whole blob back via `theme save <id> -` (reads stdin) — it activates by default.
- Command palette: `okena command list`, `okena command run <Name>` invokes a GUI
  action (e.g. `ToggleSidebar`, `NewWindow`, `ZoomIn`) — things with no dedicated CLI verb.
- Versions: `okena update status | list | revert <version>`. Preview a revert with
  `--dry-run`; apply with `--yes`. Add `--restart` only when ending active terminal
  sessions is acceptable. The matching config checkpoint is restored by default;
  `--keep-config` accepts the risk of an incompatible newer config.
- Raw: `okena state` (full JSON), `okena action '<json>'` (any ActionRequest).

Commands that create things (`term new/split/tab`, `project add`, `project clone`,
`worktree add`, `folder add`) print the new id to stdout.

## Gotchas

- **`read` is the visible screen only.** For long/full output, redirect to a file
  (`okena run --wait t 'cmd > /tmp/out'`, then read the file).
- **`run`/`send` take everything after `<term>` as literal text** — so `--wait`
  must come BEFORE the terminal, and a trailing `--window` is sent as text.
- **`run --wait` assumes a POSIX-ish shell** (bash/zsh/sh) and a non-interactive
  command (it appends a completion marker). Don't use it for vim/REPLs.
- **A bare `run` reports no completion or exit code** — only `run --wait` does.
- **Exit 75 = retry**: `worktree rm` (and `action`) exits 75 when a terminal's
  processes could not be verified as stopped. The checkout is kept; retry after
  a few seconds.
- **`worktree add` and `project clone` are optimistic**: they print the id + path
  and return before the checkout exists on disk (the reply carries `pending: true`).
  Use `worktree add --wait`: it returns once the checkout and its terminal exist,
  prints the terminal id as a third line, and exits 1 with the reason if creation
  fails. An `on_worktree_create` hook may still be running then. Without it, poll `okena ls` until the terminal appears; a failed creation
  just removes the row.
- **Find a window**: `okena ls` (or `ls --json` → `windows[].id`, `visible_project_ids`)
  lists attached desktops' windows; pass a full id or a unique prefix to `--window`.
- **`--window` is honored only by** `project add/clone/show/hide/focus`, `worktree add`,
  `term focus/fullscreen`, `window layout` and `status lanes`, and must come AFTER
  the subcommand; others just warn.
- **`project show/hide`, `--hidden`, `term fullscreen`, `window layout` and `status
  lanes` reach attached desktops**: each applies them to the `--window` target, else
  its active window.
- Default output is tab-separated (grep/awk friendly); add `--json` for structured.
  `okena ls --json` is a structured overview; `okena state` is the raw dump.
