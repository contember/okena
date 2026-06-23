# Agent Status

Okena can show what an AI coding agent (Claude Code, Codex, …) is doing in each
terminal pane: a per-tab indicator, a dedicated **Agents** section in the sidebar
that lists every active agent across all projects, a matching field in the
[remote API](remote.md), and a desktop notification when an agent finishes or
gets blocked.

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
- **Notification** — entering `blocked` or `done` raises a desktop notification
  (+ sound), suppressed for the pane you're actively looking at. Gated by the
  normal notification settings.
- **Remote** — `GET /v1/state` includes `terminal_agent_status` per project, and
  a status change bumps `state_version` so subscribed clients re-fetch.

Agent status is **runtime-only** — it is never written to `workspace.json` and
does not survive a restart.

## The data model

| Field | Meaning |
|-------|---------|
| `lifecycle` | One of `working`, `blocked`, `done`, `idle`. Drives color / sort / notifications. |
| `custom` | Optional free-form text, e.g. `"running tests 3/5"`. Rendered verbatim. |
| `labels` | Optional flat `{ "key": "value" }` map of extras. |

## The wire format (OSC 9001)

An agent reports its state by writing this OSC sequence to its terminal:

```
ESC ] 9001 ; st=<state> [ ; msg=<base64> ] [ ; lbl=<base64-json> ] ST
```

- `ESC` is `\033` (0x1B); `ST` is the string terminator `ESC \` (`\033\\`). A
  `BEL` (`\007`) terminator is also accepted.
- `st=` — `working` | `blocked` | `done` | `idle`, or `clear` to remove any
  status. An unknown/missing `st` leaves the current status untouched.
- `msg=` — base64(UTF-8) of the free-form `custom` text. Base64 keeps the value
  `;`/`ST`-safe.
- `lbl=` — base64(UTF-8) of a flat JSON object, e.g. `{"stage":"verify"}`.
  Three keys are **reserved**: `agent` (harness id, e.g. `claude-code`),
  `session_id`, and `transcript_path`. When `agent` + a UUID-shaped `session_id`
  are present, Okena captures them into the pane's *agent session* — a sticky
  record (it survives `st=clear`) that is the basis for resuming the session and
  showing transcript stats. A non-UUID `session_id` is ignored (it's untrusted
  in-band data that may reach a resume command). All other keys are free-form.

For example, to report "done" with a message, from inside the pane:

```sh
printf '\033]9001;st=done;msg=%s\033\\' "$(printf 'all tests passed' | base64 | tr -d '\n')" > /dev/tty
```

This is the same family of in-band signals Okena already understands
(`OSC 9;4` progress, `OSC 133` shell integration); see the contract note in
`crates/okena-terminal/CLAUDE.md`.

## Session resume

The reserved `agent` + `session_id` (+ optional `transcript_path`) labels let
Okena remember which AI session a pane is running and bring it back after a
restart:

- **Captured** in-band from `OSC 9001` `lbl=` (see above), validated as a UUID,
  and kept on the pane as a *sticky* record that survives `st=clear`.
- **Persisted** per terminal in `workspace.json` (`project.agent_sessions`), so
  it outlives the process.
- **Resumed** on restore when the **`auto_resume_agent_sessions`** setting is on:
  Okena types the harness's resume command (for Claude Code, `claude --resume
  <id>`) into the reconnected pane after a short delay. Off by default — when
  off, the session is still captured, persisted, and shown, just not auto-run.

Which command resumes a session is **per-harness** (Claude Code, Codex, …),
selected by the `agent` id through the harness registry — adding a new agent is
additive, with no core change.

> **Requires a session backend.** A pane's `terminal_id` (the key the session is
> stored under) only survives a restart when a session backend (`tmux` / `dtach`
> / `screen`) is configured. With `session_backend = none`, terminal IDs are
> regenerated on load, so the persisted session no longer matches and auto-resume
> is a no-op.

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

Or enable it non-interactively in `~/.claude/settings.json`:

```json
{ "enabledPlugins": { "okena-lifecycle@okena": true } }
```

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
mines the hook's stdin event JSON for `session_id` / `transcript_path` (a small
`sed` extraction, no `jq` dependency) and forwards them in the reserved `lbl=`
keys above — that's how Okena learns the pane's Claude session.

It bundles `okena-lifecycle/scripts/okena-agent-status.sh`, invoked via
`${CLAUDE_PLUGIN_ROOT}`. Hooks run as subprocesses with **no controlling
terminal**, so `/dev/tty` is unavailable to them — Okena exports `OKENA_TTY`
(the pane's slave pty path) into the pane environment and the script writes
there instead (falling back to `/dev/tty` for interactive use). It's a silent
no-op when there's no device to write to.

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

…then add to `~/.claude/settings.json`:

```json
{
  "hooks": {
    "UserPromptSubmit": [
      { "hooks": [ { "type": "command", "command": "okena-agent-status working" } ] }
    ],
    "PreToolUse": [
      { "hooks": [ { "type": "command", "command": "okena-agent-status working" } ] }
    ],
    "PostToolUse": [
      { "hooks": [ { "type": "command", "command": "okena-agent-status working" } ] }
    ],
    "Notification": [
      { "hooks": [ { "type": "command", "command": "okena-agent-status blocked" } ] }
    ],
    "Stop": [
      { "hooks": [ { "type": "command", "command": "okena-agent-status done" } ] }
    ],
    "SessionStart": [
      { "hooks": [ { "type": "command", "command": "okena-agent-status clear" } ] }
    ],
    "SessionEnd": [
      { "hooks": [ { "type": "command", "command": "okena-agent-status clear" } ] }
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
  `OKENA_TTY`); no line at all means the hook didn't fire.

- **The Okena end** — `okena_terminal::terminal::osc_sidecar` logs every parsed
  `OSC 9001` at `debug` level (`agent-status[<terminal-id>]: <prev> -> <new>
  (changed=…, notify=…)`), including ignored/unknown/clear cases, so you can see
  what Okena received and decided. Raise the log filter to `debug` to see them.

### Other agents

The script is agent-agnostic: anything that can run a command (Codex, a
Makefile, your own tooling) can call
`okena-lifecycle/scripts/okena-agent-status.sh <state> [message]` to report into
Okena.
