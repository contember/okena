---
name: okena
description: Drive a running Okena terminal multiplexer from the `okena` CLI — run commands in and read output from the user's terminals, and inspect or manage projects, worktrees, services and windows. Use whenever you need to act inside the user's Okena terminals instead of your own shell.
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

## Manage the workspace

- Projects: `okena project add <path> | rm | rename | color | focus | show | hide`
- Layout: `okena term new | close | rename | split <h|v> | tab | focus | minimize | fullscreen`
  (`split h` = stacked top/bottom, `split v` = side by side left/right)
- Worktrees: `okena worktree add <project> <branch> [--new-branch] | rm`
- Services: `okena services [project]`, `okena service start|stop|restart <name> [project]`
- Settings: `okena settings show [key] | schema | set <key> <value>` (dotted keys, e.g. `sidebar.width`).
- Theme: `okena theme list | show [id] | set <id> | save <id> <json>`. To recolor
  ("make it lighter"): `theme show` the active theme, edit the colors, pipe the
  whole blob back via `theme save <id> -` (reads stdin) — it activates by default.
- Command palette: `okena command list`, `okena command run <Name>` invokes a GUI
  action (e.g. `ToggleSidebar`, `NewWindow`, `ZoomIn`) — things with no dedicated CLI verb.
- Raw: `okena state` (full JSON), `okena action '<json>'` (any ActionRequest).

Commands that create things (`term new/split/tab`, `project add`, `worktree add`,
`folder add`) print the new id to stdout.

## Gotchas

- **`read` is the visible screen only.** For long/full output, redirect to a file
  (`okena run --wait t 'cmd > /tmp/out'`, then read the file).
- **`run`/`send` take everything after `<term>` as literal text** — so `--wait`
  must come BEFORE the terminal, and a trailing `--window` is sent as text.
- **`run --wait` assumes a POSIX-ish shell** (bash/zsh/sh) and a non-interactive
  command (it appends a completion marker). Don't use it for vim/REPLs.
- **A bare `run` reports no completion or exit code** — only `run --wait` does.
- **`--window` is honored only by** `project add/show/hide/focus` and
  `term focus/fullscreen`, and must come AFTER the subcommand; others just warn.
- Default output is tab-separated (grep/awk friendly); add `--json` for structured.
  `okena ls --json` is a structured overview; `okena state` is the raw dump.
