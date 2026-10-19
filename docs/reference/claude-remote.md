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
- **Mobile**: the terminal gets a Terminal / Chat switch in the app bar; the chat replaces the terminal and its key toolbar. Terminal tabs and
  drawer rows show the agent state and the count of open dialogs, and the
  drawer's **Chat** pill opens a terminal straight in the chat. A project with
  an open dialog is listed under Needs attention.
- **Desktop**: settings only; dialogs are answered in the terminal.

## How a session is connected

1. The daemon writes a Bun preload script and gives new panes
   `BUN_OPTIONS=--preload=<file>`, `OKENA_BIN` and
   `OKENA_CLAUDE_REMOTE_CHANNEL`. The `claude` binary is Bun-compiled, so the
   preload runs inside it before Claude Code parses its arguments.
2. For an interactive run, the preload appends `--settings` with Okena's hooks
   and, with the channel on, `--mcp-config` plus
   `--dangerously-load-development-channels server:okena-remote`. Claude shows
   a confirmation dialog for the development channel at every start.
3. The hooks run `okena claude-remote hook permission|event`. The channel runs
   `okena claude-remote channel` (stdio MCP). Both talk to the daemon over
   loopback with the CLI token.

The preload's location, environment and the cases it leaves alone are in
[`integrations/claude-code/okena-remote/README.md`](../../integrations/claude-code/okena-remote/README.md).

## Prompts

`POST /v1/claude/terminals/{id}/prompt` returns how the prompt was delivered:

| `delivery` | When |
|---|---|
| `channel` | The channel server is polling. Claude queues the prompt after a running turn. The TUI shows it as `← okena-remote: …`. |
| `pty` | No channel, Claude is idle: the text is pasted into the pane and submitted. |
| `queued` | No channel, Claude is in a turn: the prompt is typed in when the `Stop` hook reports the turn ended. |

`409` means Claude is busy and no `Stop` hook will arrive (no hooks attached).
Prompts never go into a pane that runs a plain shell. Channel prompts skip
slash-command parsing and attachments. **Interrupt** sends Escape to the pane.

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
`assistant`, `tool_use` and `tool_result`. User entries carry `origin`
(`channel` for remote prompts). `reset: true` means the client must drop what
it has (new session or rewritten file).

## Limitations

- State lives in daemon memory. After a daemon restart the channel reconnects
  on its own, but hooks count as attached only after the next hook event, and a
  dialog that was waiting is answerable only in the terminal. Without the
  channel, an idle `claude` is not listed as connected until its next step.
- A `claude` that dies without a `SessionEnd` hook stays listed until the next
  state change.
- An interrupt (Escape) fires no `Stop` hook, so `queued` prompts wait for the
  next finished turn.
- Clients poll (1 s); there are no push notifications.
- The Claude Code JSONL format and channel API are undocumented and may change
  between versions. Verified on Claude Code 2.1.301.
