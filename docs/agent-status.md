# Agent Status

Okena can show what an AI coding agent is doing in each terminal pane: a per-tab
indicator, a dedicated **Agents** section in the sidebar that lists every active
agent across all projects, a matching field in the [remote API](remote.md), and a
desktop notification when an agent finishes or gets blocked.

Claude Code is supported out of the box via a [bundled plugin](#claude-code-integration).
The protocol is agent-agnostic and the resume logic is per-harness, so adding
another agent is additive — but today Claude Code is the only one that ships
working glue. Codex has a registered harness that declines to resume, and no
capture hooks yet.

> **Unix only.** The hook script is `#!/bin/sh` and depends on `/dev/tty`,
> `base64` and `sed`, and `$OKENA_TTY` is exported only on Unix. On Windows this
> works inside WSL panes; native `cmd`/PowerShell panes cannot report agent
> status, and the setup below is a silent no-op there.

The model is **push-based and open**: the agent reports its own state by writing
a small escape sequence to its terminal. Okena never scrapes the agent's output
or reads its private files — an agent (or a thin hook) tells Okena directly. A
small fixed set of lifecycle states drives color, sort order, and notifications;
a free-form message and optional labels carry whatever the agent wants and are
shown verbatim.

## What you see

- **Tab** — the pane's icon is recolored by lifecycle (blocked = red, working =
  yellow, done = green, idle = muted). Hovering the tab shows the agent's
  free-form status text.
- **Sidebar → AGENTS** — a flat, cross-project list of every pane currently
  reporting a status, sorted by attention (blocked → done → working → idle).
  Each row shows the lifecycle dot, the terminal name, its project, and the
  free-form text. Click a row to jump straight to that pane. The section hides
  itself when no agent is active.

  The list is deliberately cross-project, so a row can point at a project the
  current window isn't showing (hidden set, folder filter). Clicking then zooms
  to that project so the pane is actually reachable — the jump goes through the
  same window-aware `focus_terminal_by_id` as sidebar clicks, cursor navigation,
  notification jumps, and remote focus requests, which also keeps a jump made
  from a fullscreened pane in fullscreen.
  A pane that is *not* in a tab group (a new project, or any leaf of a plain
  split) has no tab bar, so its lifecycle shows as a tinted pane border instead
  — same colors, except `idle`, which is the resting state and draws nothing.
- **Notification** — entering `blocked` or `done` raises a desktop notification
  (+ sound), suppressed for the pane you're actively looking at. Gated by the
  normal notification settings.

  Raised by the client that **parsed the transition live**. A transition that
  happens while a client is disconnected is not replayed to it when it
  reconnects: the snapshot restores the *indicator*, but re-notifying on every
  reconnect would spam, so it stays silent. Mobile clients get no notification
  at all today.
- **Remote** — `GET /v1/state` includes `terminal_agent_status` per project, and
  a status change bumps `state_version` so subscribed clients re-fetch.

Agent status is **runtime-only** — it is never written to `workspace.json` and
does not survive a restart. (The *agent session* below is the part that is
persisted.)

### Stale status

A status is removed only by an explicit `st=clear` or by the pane going away.
Nothing ties it to the pane's process, so an agent that dies without sending
`clear` — a crash, `kill -9`, an `exit` that skips its `SessionEnd` hook —
leaves the tab tinted and the pane listed in AGENTS indefinitely. Closing the
pane, or having an agent report again in it, clears it.

## The data model

| Field | Meaning |
|-------|---------|
| `lifecycle` | One of `working`, `blocked`, `done`, `idle`. Drives color / sort / notifications. |
| `custom` | Optional free-form text, e.g. `"running tests 3/5"`. Shown in the tab tooltip and the AGENTS row, flattened to one line and clipped. |
| `labels` | Optional flat `{ "key": "value" }` map of extras. Carried on the wire; no UI renders them yet. The three reserved session keys are stripped out before this map is built, so session identity never leaves the machine. |

## The wire format (OSC 9001)

An agent reports its state by writing this OSC sequence to its terminal:

```
ESC ] 9001 ; st=<state> [ ; tid=<terminal-id> ] [ ; msg=<base64> ] [ ; lbl=<base64-json> ] ST
```

- `ESC` is `\033` (0x1B); `ST` is the string terminator `ESC \` (`\033\\`). A
  `BEL` (`\007`) terminator is also accepted.
- `st=` — `working` | `blocked` | `done` | `idle`, or `clear` to remove any
  status. An unknown/missing `st` leaves the current status untouched.
- `tid=` — the pane the status is *for*, from `$OKENA_TERMINAL_ID`. A pane drops
  a sequence addressed to a different id (see "Choosing the output device").
  Optional: omit it and the receiving pane accepts the status.
- `msg=` — base64(UTF-8) of the free-form `custom` text. Base64 keeps the value
  `;`/`ST`-safe.
- `lbl=` — base64(UTF-8) of a flat JSON object, e.g. `{"stage":"verify"}`.
  Three keys are **reserved**: `agent` (harness id, e.g. `claude-code`),
  `session_id`, and `transcript_path`. When `agent` + a UUID-shaped `session_id`
  are present, Okena captures them into the pane's *agent session* — a sticky
  record (it survives, and is captured on, `st=clear`) that is the basis for
  resuming the session. All other keys are free-form.

  The reserved keys are read from the raw label map and then **removed** from
  it, so they never reach `labels` on the wire. They are also the only part of
  an agent status that gets written to disk, so they are validated rather than
  trusted: `session_id` must be a canonical UUID, `agent` must be ≤64 chars of
  `[A-Za-z0-9._-]`, and `transcript_path` must be absolute and free of `..` and
  ≤4096 bytes. Anything else is dropped — a bad `transcript_path` alone doesn't
  discard the session, a bad `agent` or `session_id` does.

Everything on this sequence is **untrusted**. Any process that can write to a
pane — including a `cat` of a hostile file, or a remote host over SSH — can emit
it. That is why the values are bounded and validated, why the resume argv must
be shell-neutral, and why `tid=` exists.

For example, to report "done" with a message, from inside the pane:

```sh
printf '\033]9001;st=done;msg=%s\033\\' "$(printf 'all tests passed' | base64 | tr -d '\n')" > /dev/tty
```

This is the same family of in-band signals Okena already understands
(`OSC 9;4` progress, `OSC 133` shell integration); see the contract note in
`crates/okena-terminal/CLAUDE.md`.

### Choosing the output device

Prefer **`$OKENA_TTY`** — the pane's own slave pty, which Okena exports into
every pane's environment — and fall back to the controlling terminal:

```sh
if [ -n "${OKENA_TTY:-}" ] && (: >"$OKENA_TTY") 2>/dev/null; then
    dev="$OKENA_TTY"
elif (: >/dev/tty) 2>/dev/null; then
    dev=/dev/tty
fi
```

The intuitive order is the other way round, since `/dev/tty` always names the
pane's *current* pty. But under a session backend it names the **wrong** one:
with `session_backend = tmux` the pane process is tmux, so a hook's controlling
terminal is tmux's pty — and tmux forwards only a fixed allowlist of OSC numbers,
which 9001 is not on. The sequence is dropped before it ever reaches Okena.
Screen behaves the same way. Writing to `$OKENA_TTY` reaches Okena's own reader
and bypasses the nested pty entirely. It also covers hooks that have no
controlling terminal at all.

> Keep the probes in a subshell. POSIX makes a redirection error on a special
> built-in (`:`) exit the shell, so a bare `: >/dev/tty` aborts the script in
> exactly the no-tty case the fallback is for.

`$OKENA_TTY` is captured once, so after a reattach it may name a pty that now
belongs to a *different* pane. That is what `tid=` defends against: stamp
`$OKENA_TERMINAL_ID` on the sequence and a pane that isn't the addressee drops
it instead of showing another agent's status.

> **`$OKENA_TTY` is a capability, not just a hint.** Unlike an inherited file
> descriptor, a *path* can be reopened, and Linux recycles `/dev/pts/N` numbers.
> A process that outlives its pane (a daemonized dev server, a forking
> postinstall) keeps a way to write into whatever pane later inherits that
> number — and writes to a slave arrive as terminal *output*, i.e. they are
> parsed as escape sequences. It is same-user, so not a privilege boundary, but
> `tid=` guards OSC 9001 only; nothing else on that channel is addressed.

## Session resume

The reserved `agent` + `session_id` (+ optional `transcript_path`) labels let
Okena remember which AI session a pane is running and bring it back after a
restart:

- **Captured** in-band from `OSC 9001` `lbl=` (see above), validated as a UUID,
  and kept on the pane as a *sticky* record that survives `st=clear`.
- **Persisted** per terminal in `workspace.json` (`project.agent_sessions`), so
  it outlives the process.
- **Re-keyed** on load. Without a session backend a restore clears every
  terminal id — exactly the keys the sessions are stored under. Before dropping
  them, `validate_workspace_data` moves each surviving session onto its pane's
  *layout path* (`project.pending_agent_resumes`, never persisted), which is the
  pane identity that does survive that load.
- **Resumed** by the daemon when the **`auto_resume_agent_sessions`** setting is
  on (see [configuration](configuration.md#session-backend)): 
  `spawn_uninitialized_terminals` consumes the queued session as it gives
  the pane its new terminal id, and runs the harness's resume command (for
  Claude Code, `claude --resume <id>`) as the pane's **startup command**,
  chained after any `on_create` hook so the pane still ends up at an interactive
  shell. Off by default — when off, the session is still re-attached to the
  restored pane and shown, just not auto-run.

Consuming the queued entry makes this **exactly-once**: a pane respawned later
in the same session does not re-resume. And because it hangs off the *spawn*
path, a pane that re-attaches to a live backend session (tmux/dtach — where the
agent is still running) is never touched.

Opening a saved session or importing a workspace also re-keys each pane's
session onto its new terminal id, so the identity survives — but nothing is
auto-resumed there. Opening a session is a request to open panes, not to re-run
whatever agent last lived in each of them.

> **What the resume actually resumes.** The record is "the last agent session
> this pane ever ran", not "what was running at shutdown". There is no
> `session ended` signal in the protocol — `st=clear` deliberately keeps the
> session — so a pane where you finished with Claude an hour ago and then used
> for something else will still relaunch `claude --resume <that id>` on the next
> start with auto-resume on. Close the pane to drop the record.

Which command resumes a session is **per-harness** (Claude Code, Codex, …),
selected by the `agent` id through the harness registry — adding a new agent is
additive, with no core change.

> **Resume argv must be shell-neutral.** The command is composed into the pane's
> shell line, so `okena_core::agent_harness::resume_command_line` refuses any
> argv element containing whitespace, quotes, or metacharacters rather than
> quote per dialect. A harness needing more should ship a launcher script.

The session follows its pane: it moves with a cross-project drag, survives the
undo window of a soft close, and is dropped on a hard close, a finalized soft
close, or a shell switch. Sessions whose pane is gone — or whose stored
`session_id` no longer looks like a UUID — are pruned on load.

## Claude Code integration

The easiest way is the bundled **Claude Code plugin**, which wires up the
lifecycle hooks for you — no editing of `settings.json`, versioned and cleanly
uninstallable. The [`integrations/claude-code/`](../integrations/claude-code/)
directory is a Claude Code plugin marketplace.

From a clone of this repo:

```
/plugin marketplace add ./integrations/claude-code
/plugin install okena-lifecycle@okena
```

Once the marketplace is registered, you can enable the plugin non-interactively
instead of the second step, in `~/.claude/settings.json`:

```json
{ "enabledPlugins": { "okena-lifecycle@okena": true } }
```

This is *not* an alternative to the first step: `@okena` names the marketplace,
which has to exist before the plugin can resolve.

Then run `claude` inside an Okena pane and watch the tab + AGENTS section react.

The plugin maps Claude Code's lifecycle hooks to agent states:

| Claude Code hook | State | When |
|------------------|-------|------|
| `UserPromptSubmit` | `working` | You submit a prompt — the agent starts working. |
| `PreToolUse` | `working` | The agent is about to run a tool — work resumes. |
| `PostToolUse` | `working` | A tool finished — work continues. |
| `Notification` | `blocked` | Claude needs permission or input. |
| `Stop` | `done` | The agent finished its turn. |
| `SessionStart` | `clear` | A new/resumed session — reset any stale status. |
| `SessionEnd` | `clear` | The agent exited — drop it from the Agents list. |

`PreToolUse` / `PostToolUse` are the recovery edges that the obvious four-hook
mapping is missing: when Claude is `blocked` waiting on you and you answer
(e.g. a permission grant, or answering a question mid-turn), **no
`UserPromptSubmit` fires** — that only fires for a fresh prompt. Without a
"work resumed" signal the pane stays stuck on `blocked` even though the agent is
busy again. Running a tool fires `PreToolUse` (and later `PostToolUse`), which
flips it back to `working`. Ordering is safe: for a permission-gated tool the
sequence is `PreToolUse` (working) → `Notification` (blocked) → you approve →
tool runs → `PostToolUse` (working), and hooks are awaited so the writes never
race — the pane correctly shows `blocked` while you're being asked.

The plugin sets `OKENA_AGENT=claude-code` on each hook command, and the script
mines the hook's stdin event JSON for `session_id` / `transcript_path` and
forwards them in the reserved `lbl=` keys above — that's how Okena learns the
pane's Claude session. It uses `jq` when available and falls back to a `sed`
extraction otherwise, so there is no hard dependency. Only **top-level** keys are
read: `PreToolUse` / `PostToolUse` payloads embed `tool_input` as nested JSON,
and a tool argument called `session_id` must not be able to hijack the pane's
identity.

It bundles `okena-lifecycle/scripts/okena-agent-status.sh`, invoked via
`${CLAUDE_PLUGIN_ROOT}`. The script writes to `$OKENA_TTY` (the pane's own slave
pty), falling back to `/dev/tty` — see [Choosing the output
device](#choosing-the-output-device) for why that order. It's a silent no-op
when there's no device to write to.

> **Caveat — persistent sessions.** `OKENA_TTY` is captured into the shell's
> environment when the pane is **first launched**, not refreshed per-attach. If
> you *reattach* to a pre-existing `dtach`/`tmux`/`screen` session, the
> already-running shell keeps the original value while Okena has opened a new
> pty, so `$OKENA_TTY` points at the old device and the indicator can go silent
> until the session is restarted. (Known limitation — the env isn't yet
> refreshed on reattach.)

### Manual (without the plugin)

If you'd rather not use the plugin, register the hooks yourself. Copy the script
somewhere on your `PATH`:

```sh
install -m 0755 integrations/claude-code/okena-lifecycle/scripts/okena-agent-status.sh ~/.local/bin/okena-agent-status
```

…then add to `~/.claude/settings.json`. Note the `OKENA_AGENT=` prefix on every
command: without it Okena has no harness id, so it drops the session entirely —
you get the indicator, but no persistence and no resume, with no error.

```json
{
  "hooks": {
    "UserPromptSubmit": [
      { "hooks": [ { "type": "command", "command": "OKENA_AGENT=claude-code okena-agent-status working" } ] }
    ],
    "PreToolUse": [
      { "hooks": [ { "type": "command", "command": "OKENA_AGENT=claude-code okena-agent-status working" } ] }
    ],
    "PostToolUse": [
      { "hooks": [ { "type": "command", "command": "OKENA_AGENT=claude-code okena-agent-status working" } ] }
    ],
    "Notification": [
      { "hooks": [ { "type": "command", "command": "OKENA_AGENT=claude-code okena-agent-status blocked" } ] }
    ],
    "Stop": [
      { "hooks": [ { "type": "command", "command": "OKENA_AGENT=claude-code okena-agent-status done" } ] }
    ],
    "SessionStart": [
      { "hooks": [ { "type": "command", "command": "OKENA_AGENT=claude-code okena-agent-status clear" } ] }
    ],
    "SessionEnd": [
      { "hooks": [ { "type": "command", "command": "OKENA_AGENT=claude-code okena-agent-status clear" } ] }
    ]
  }
}
```

### Debugging

If a pane's status looks wrong (stale `blocked`, nothing showing), make the
whole path observable from both ends:

- **The hook end** — set `OKENA_AGENT_STATUS_LOG` to a writable file in the
  pane's environment. The script then appends one line per invocation recording
  the state, the target device (`$OKENA_TTY`), and whether the write actually
  succeeded:

  ```
  2026-06-23T11:30:01+0200 pid=12345 state=working tty=/dev/pts/7 msglen=0 write=ok
  ```

  A `write=failed` line means the OSC never reached Okena (wrong/missing
  `OKENA_TTY`); `skip=bad-state` means the state argument wasn't one Okena
  knows; `skip=no-agent` means a `session_id` was found but `OKENA_AGENT` is
  unset, so the session is being dropped; no line at all means the hook didn't
  fire — or, on native Windows, that the script can't run at all.

- **The Okena end** — `okena_terminal::terminal::osc_sidecar` logs every parsed
  `OSC 9001` at `debug` level (`agent-status[<terminal-id>]: <prev> -> <new>
  (changed=…, notify=…)`), including ignored/unknown/clear cases, so you can see
  what Okena received and decided. Raise the log filter to `debug` to see them.

### Other agents

The script is agent-agnostic: anything that can run a command (Codex, a
Makefile, your own tooling) can call
`okena-lifecycle/scripts/okena-agent-status.sh <state> [message]` to report into
Okena. Set `OKENA_AGENT` to your harness id if you also want session capture.

Resuming a captured session needs a harness registered in
`okena-agent-harnesses`, which maps an `agent` id to the argv that resumes it.
Claude Code is implemented; Codex is registered but declines to resume until its
CLI invocation is confirmed, so a Codex session is captured and shown but not
auto-resumed.
