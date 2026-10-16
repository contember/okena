# Claude remote control over channels and permission hooks

A chat-shaped remote for `claude` sessions running in Okena panes, so a phone can
follow a session, send prompts and answer its dialogs without a terminal view.

## Why

Claude Code's own Remote Control refuses to start when `ANTHROPIC_BASE_URL` does
not point at `api.anthropic.com`. The check is client-side and has no opt-out
(`_CLAUDE_CODE_ASSUME_FIRST_PARTY_BASE_URL` explicitly does not apply). Every pane
behind the AI proxy (`okena --ai-proxy`) therefore loses Remote Control. Pointing
the bridge at our own server is not possible either: `CLAUDE_BRIDGE_BASE_URL` is
honoured only for internal builds.

A terminal on a phone is a poor interface, so mirroring the PTY alone is not the
answer.

## Building blocks (verified on Claude Code 2.1.301, behind the AI proxy)

| Need | Mechanism | Result |
|---|---|---|
| Push a prompt into a running interactive session | MCP server with `experimental["claude/channel"]`, sending `notifications/claude/channel` `{ content, meta }` | Works. Enqueued as the next prompt; queued after a running turn. Shown in the TUI as `← <server>: …`. |
| Answer permission dialogs | `PermissionRequest` hook returning `decision` | Works. The hook runs **concurrently** with the TUI dialog; the first answer wins, a late answer is ignored. `updatedPermissions` with `destination: "session"` gives "always allow". |
| Answer `AskUserQuestion` | Same hook, `decision.updatedInput = { questions, answers: { "<question text>": "<label>" } }` | Works. `updatedInput` must still pass the tool schema (keep the original `questions`). |
| Read the conversation | Session JSONL (`transcript_path` from any hook input) | Channel prompts are `user` records with `isMeta: true` and `origin: { kind: "channel", server }`. |

Things that do **not** work through a channel:

- Slash commands. The content is wrapped in `<channel>` and skips slash-command
  parsing; the model just explains it cannot run them.
- Attachments.
- Interrupting a turn.
- Answering `AskUserQuestion` / `ExitPlanMode` through the channel's own
  permission relay (`claude/channel/permission`): it skips tools that require
  user interaction and only carries allow/deny. The hook covers all of it, so the
  relay is not needed.

Registration constraints:

- Our server is not on Anthropic's channel allowlist, so it loads only through
  `--dangerously-load-development-channels server:<name>`. Passing the same
  server in `--channels` as well makes the allowlist check reject it.
- That flag shows a confirmation dialog on **every** start.
- A remote kill switch (`tengu_harbor`) and, for Team/Enterprise orgs, the
  `channelsEnabled` managed setting can disable channels.
- Hooks and the MCP server can both come from flags (`--settings`,
  `--mcp-config`), so nothing has to be written to `~/.claude`.

## Proposed shape

- **`okena mcp-channel`** (stdio, in `okena-cli`): connects to the daemon,
  identifies the pane by `OKENA_TERMINAL_ID`, forwards phone prompts as channel
  notifications. Optionally exposes a `notify` tool so Claude can push a message
  to the phone on purpose.
- **`PermissionRequest` hook** in the `okena-lifecycle` plugin: long-polls the
  daemon with a long `timeout`; the phone answers permissions, `AskUserQuestion`
  and plan approval.
  - The hook input carries no `tool_use_id`. The daemon must cancel a pending
    request when the terminal answers first, by matching `session_id` +
    `tool_name` + `tool_input` against `PostToolUse`. Otherwise the hook
    process waits until its timeout.
- **Transcript reader** in the daemon: tails the JSONL, maps known record types
  into a small wire model, skips unknown ones. No token streaming; the phone
  sees whole blocks.
- **PTY** only for Esc (interrupt) and slash commands when the session is idle.
- **Mobile**: a chat screen next to the terminal view in `mobile/rn`.

## Open questions

- Launching `claude` with the flags: auto-resume can add them; a `claude` typed by
  hand needs a wrapper on the pane's `PATH`. The per-start confirmation dialog
  of the dev flag also blocks an unattended resume.
- Push notifications while the phone is locked need a relay (APNs/FCM or
  something like ntfy). Okena has no cloud component today.
- The JSONL format is undocumented and changes between Claude Code versions.
