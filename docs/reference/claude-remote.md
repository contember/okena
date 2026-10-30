# Claude remote

A chat view of `claude` sessions running in Okena panes. Web and mobile clients
can read the conversation, send prompts, interrupt a turn, and answer permission
dialogs, `AskUserQuestion` and plan approval. It works when Claude Code's own
Remote Control is off, for example behind the AI proxy (`ANTHROPIC_BASE_URL`
set). Unix only.

## Turning it on

Settings → Terminal → **Claude Remote** (`claude_remote`, default off). The
nested **Use MCP Channel for Prompts** (`claude_remote_channel`, default on)
selects how prompts are delivered.

Only panes created after the change are affected, and `claude` must be started
in them. Nothing is written to `~/.claude`.

## Where the chat is

The chat is offered for a terminal whose `claude` is connected (hooks or MCP
channel attached; `claude_sessions` in `GET /v1/state`) or has an open dialog
(`claude_dialogs`).

- **Web**: the **Claude** button in a terminal's header opens the chat panel: a
  drawer on wide screens, full screen on phones.
- **Mobile**: the terminal gets a Terminal / Chat switch in the app bar; the chat replaces the terminal and its key toolbar. The composer's **+** uploads a
  picked file to the daemon host (`POST /v1/files/upload`) and adds its path
  to the message; the terminal's compose sheet has the same **Attach**. Running
  subagents are listed under the status strip, and an `Agent` call links to its
  subagent's transcript. Terminal tabs and
  drawer rows show the agent state and the count of open dialogs, and the
  drawer's **Chat** pill opens a terminal straight in the chat. A project with
  an open dialog is listed under Needs attention.
- **Desktop**: settings only; dialogs are answered in the terminal.
- **CLI**: `okena claude start | ls | show | find | subagents | log | prompt |
  answer | interrupt` uses the same endpoints, for people and for agents driving other
  sessions. `log` prints a cursor (`<session_id>:<offset>`) to pass back with
  `--cursor` for only the newer entries. `start` runs `claude` in a pane, see
  [Starting a session](#starting-a-session). See `okena skill show`.

## How a session is connected

1. The daemon writes a Bun preload script and gives new panes
   `BUN_OPTIONS=--preload=<file>`, `OKENA_BIN`,
   `OKENA_CLAUDE_REMOTE_CHANNEL` and `OKENA_PROFILE`. The `claude` binary is Bun-compiled, so the
   preload runs inside it before Claude Code parses its arguments.
2. For an interactive run, the preload appends `--settings` with Okena's hooks
   and, with the channel on, `--mcp-config` plus
   `--dangerously-load-development-channels server:okena-remote`. Claude shows
   a confirmation dialog for the development channel at every start.
3. The hooks run `okena claude-remote hook permission|event`. The channel runs
   `okena claude-remote channel` (stdio MCP). Both talk to the daemon over
   its Unix socket.

The preload's location, environment and the cases it leaves alone are in
[`integrations/claude-code/okena-remote/README.md`](../../integrations/claude-code/okena-remote/README.md).

## Starting a session

Before it connects, `claude` may wait on a start-up dialog: the folder-trust
question, then the development-channels warning that Okena's own
`--dangerously-load-development-channels` causes. While neither the hooks nor
the channel are attached, a session read reads the pane's screen:
`startup_dialog` names the dialog (`trust`, `channels` or `null`) only while
`claude` is the foreground process, so a dialog left on screen by a `claude`
that exited does not count (a dialog on screen costs a foreground probe).
`?probe_foreground=true` also reports that process as `foreground_command`
(absent at a shell prompt).

`okena claude start <term>` needs a shell prompt in the pane: it refuses a pane
whose foreground is another program (or a `claude` started without Okena's
hooks) and one with a connected `claude`. It types `claude`, or resumes one
already waiting on a start-up dialog, and polls the session. It accepts the
channels warning; the trust dialog only with `--trust`, else it exits 1 and
leaves the dialog open, and a second run with `--trust` resumes from it. A key
is sent only when a fresh read of the screen still shows the dialog the session
named: an arrow towards the confirming option, or Enter on it, at most one key
per 700 ms. Once the channel is attached (with `claude_remote_channel` off:
once the hooks are), it sends `--prompt-file` as the first prompt and prints
`terminal`, `session_id` and `delivery` (`-` or `null` without a prompt). It
exits 1 when Claude Remote is off or `claude` does not connect within
`--timeout` (60 s).

`--wait` then blocks until the turn ends and prints `state`: `idle` once the
turn ran (the pane was seen busy, or the transcript shows a reply after the
prompt; the hooks run async, so an idle pane alone proves nothing) and the pane
is idle with nothing queued; `dialog` when a dialog waits for an answer;
`disconnected` (exit 1) when `claude` left; `timeout` (exit 1) after
`--wait-timeout` (3600 s).

## Client endpoints

All take the bearer token. Errors have a plain-text body.

| Endpoint | Purpose |
|---|---|
| `GET /v1/claude/terminals/{id}?probe_foreground=` | Session: `session_id`, `hooks_attached`, `busy`, open `dialogs`, `running_subagents`, `pr_links`, `startup_dialog`, `foreground_command` |
| `GET /v1/claude/sessions/{session_id}` | The session of the pane that runs it, same shape as above (`terminal_id` names the pane); `404` when no pane does |
| `GET /v1/claude/terminals/{id}/transcript?cursor=&session_id=&agent_id=` | Transcript page, see [Transcript](#transcript) |
| `GET /v1/claude/terminals/{id}/subagents` | The session's subagents, see [Subagents](#subagents) |
| `POST /v1/claude/terminals/{id}/prompt` | `{ "text", "expect_session_id"? }` → `{ "delivery" }`, see [Prompts](#prompts) |
| `POST /v1/claude/terminals/{id}/dialogs/{dialog_id}` | `{ "decision": "allow" \| "deny" \| "answer", "always"?, "message"?, "answers"? }`; `404` once the dialog is answered or gone |

Dialog ids are opaque strings, unique across daemon restarts. String values
over 16 KiB in a dialog's `tool_input` are cut for clients; the answer to
Claude uses the full input.

## Turns and liveness

Hooks keep `busy`: `UserPromptSubmit` and the tool hooks set it, `Stop` (or
`StopFailure`, when an API error ends the turn) clears it and types the next
`queued` prompt. An interrupt (Escape, also answering a permission dialog with
"No") fires neither, so the daemon also reads the end of each turn from the
transcript: Claude writes a `turn_duration` record, or a `[Request interrupted
by user…]` message when interrupted while generating. A recorded end at or
after the time the turn's first hook (`UserPromptSubmit`, or a tool hook for a
turn Claude started itself) reached the daemon ends it like `Stop`. The
transcript is read on session reads, so this lands on the next one. A `Stop`
that arrives after the transcript already ended the turn types nothing more.

A `claude` that is killed or crashes sends no `SessionEnd`. While its hooks are
attached and no channel polls, a session read probes the pane's foreground at
most every 5 s. When the probe finds the pane's shell running no process at
all, the session counts as gone: hooks detached, `session_id` and `busy`
`null`, dialogs cancelled, queued prompts dropped, and the pane leaves
`claude_sessions` and `GET /v1/claude/sessions/{session_id}`. A prompt whose
probe finds the same does the same. A probe that fails, an OS without process
probing, or a shell replaced by `exec claude` never counts as gone, and neither
does a hook arriving while the probe runs. A later hook of that session
attaches it again. With the channel on, this starts once the dead channel stops polling
(within its 25 s poll timeout).

A permission dialog whose hook stops waiting (Claude answered or dismissed it
in the terminal) is removed at once.

## Sessions by id

A session's `session_id` is the session `claude` runs in the pane now: the
hooks' session until its `SessionEnd`, after which it is `null` until a new one
(or `--resume`) starts. Before any hook has reported, as after a daemon restart,
it is the session the daemon recorded from the pane; that record is not cleared
when `claude` exits, so it is a weaker signal.

`GET /v1/claude/sessions/{session_id}` (`okena claude find`) finds the pane
whose hooks report that session and no `SessionEnd` (of two such panes, the
lowest terminal id). A session is known once a hook reported it, so after a
daemon restart only after its next hook event.

## Prompts

`POST /v1/claude/terminals/{id}/prompt` returns how the prompt was delivered:

| `delivery` | When |
|---|---|
| `channel` | The channel server is polling and the prompt is not a slash command. Claude queues the prompt after a running turn. The TUI shows it as `← okena-remote: …`. |
| `pty` | No channel or a slash command, Claude is idle: the text is pasted into the pane and submitted. |
| `queued` | No channel or a slash command, Claude is in a turn: the prompt is typed in when the turn ends (see [Turns and liveness](#turns-and-liveness)). |

`400` means an empty prompt or one over 64 KiB. `409` means the prompt cannot
be delivered: no `claude` runs in the pane, Claude is busy and no `Stop` hook
will arrive (no hooks attached), 32 prompts are already queued, or
`expect_session_id` names another session than the pane's `session_id` (see
[Sessions by id](#sessions-by-id)). `okena claude prompt --session <id>` sends
it. The check covers a session that changed before the prompt arrived, not one
that changes while it is being typed into the pane. A prompt is
typed into the pane only while its foreground process is `claude`; queued
prompts are dropped if `claude` is gone when the turn ends. Claude skips
slash-command parsing and attachments for channel prompts, so a prompt starting
with `/` always goes through the pane. A command that opens a picker (`/model`
without an argument) can be answered only in the terminal. **Interrupt** sends
Escape to the pane.

## Dialogs

The `PermissionRequest` hook waits for a remote answer while Claude shows its
own dialog in the terminal. The first answer wins: answering in the terminal
releases the waiting hook and removes the dialog from clients.

| Client decision | Hook output |
|---|---|
| Allow | `allow` with the unchanged input as `updatedInput` (Claude drops an allow without it for `ExitPlanMode`) |
| Always allow | the same, plus Claude's own suggestions with `destination: "session"` |
| Deny | `deny` with the message, default "Denied from Okena remote" |
| Answer (`AskUserQuestion`) | `allow` with `updatedInput` = original input + `answers` (multi-select labels joined with `, `) |

`ExitPlanMode` carries `plan` and `planFilePath` only when Claude wrote a plan
file; otherwise the plan is in the transcript.

## Transcript

`GET /v1/claude/terminals/{id}/transcript?cursor=&session_id=` reads the
session JSONL from `transcript_path` incrementally. Entries are `user`,
`assistant`, `tool_use` and `tool_result`. Each carries `timestamp`, when
Claude recorded it (RFC 3339 as Claude wrote it on its JSONL line; `null` when
the line has none or it does not parse); the entries of one line share it. User entries carry `origin`
(`channel` for remote prompts). `reset: true` means the client must drop what
it has (new session or rewritten file).

## Pull requests

Claude Code records each pull request a session creates or touches as a
`pr-link` line in the transcript. The session's `pr_links` lists them, newest
first, one per `url`, with `number`, `repository` (`owner/repo`) and
`last_seen_at` (the newest record's timestamp); at most 50, and only `https://`
URLs. `claude_pr_links` in `GET /v1/state` carries the same lists for the
terminals in `claude_sessions`.

The web chat panel's header and the mobile chat's status strip show each as a
`#<number>` chip that opens the PR.

The daemon reads only the lines appended since its last read, after the
`SessionStart`, `PostToolUse`, `PostToolUseFailure`, `Stop` and `StopFailure`
hooks and on every session read; `/v1/state` reads no files. A new PR bumps
`state_version`.

## Subagents

The `SubagentStart` and `SubagentStop` hooks keep the session's
`running_subagents` (`agent_id`, `agent_type`). A stop that arrives before its
start, as async hooks may, wins.

`GET /v1/claude/terminals/{id}/subagents` lists the subagents Claude Code
recorded next to the session transcript (`<session>/subagents/agent-<id>.jsonl`),
oldest first, at most 200. `tool_use_id`, `agent_type` and `description` come
from `agent-<id>.meta.json` and are missing when Claude wrote none; `running`
mirrors `running_subagents`. The transcript route with `agent_id` reads that
subagent's transcript; an id that is not ASCII letters, digits, `-` or `_` is a
`404`. Workflow agents (`subagents/workflows/`) are not listed.

## Limitations

- State lives in daemon memory. After a daemon restart the channel reconnects
  on its own, but hooks count as attached only after the next hook event, and a
  dialog that was waiting is answerable only in the terminal. Without the
  channel, an idle `claude` is not listed as connected until its next step.
- A `claude` that dies without a `SessionEnd` hook stays listed as connected
  until a session read finds the pane's shell running nothing (see
  [Turns and liveness](#turns-and-liveness)). Until its channel stops polling,
  a prompt is still accepted for the channel and lost.
- A prompt is typed into the pane only when the pane's foreground process is
  named `claude`. One started with `exec` or through a wrapper gets prompts
  only through the channel.
- Hook events run asynchronously. Events from an ended session, and events
  from another session than the current one before its `SessionStart`, are
  ignored.
- An interrupt ends `busy` only once a session read finds its end in the
  transcript. One that lands before the turn's first hook reached the daemon
  (hooks run async) is older than the turn and does not end it; the next
  `Stop` does.
- Clients poll (1 s); there are no push notifications.
- `running_subagents` lives in daemon memory like the rest; after a restart it
  fills again only with subagents started afterwards.
- The Claude Code JSONL format and channel API are undocumented and may change
  between versions. Verified on Claude Code 2.1.301.
