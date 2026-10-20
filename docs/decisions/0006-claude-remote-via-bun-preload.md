---
id: 0006
title: Connect claude panes to remote clients through a Bun preload, hooks and an MCP channel
status: accepted
date: 2026-10-16
---

# 0006 — Connect claude panes to remote clients through a Bun preload, hooks and an MCP channel

## Context

Claude Code's Remote Control refuses to start when `ANTHROPIC_BASE_URL` does not
point at `api.anthropic.com`. The check is client-side and has no opt-out, and
the bridge URL can be changed only in internal builds. Every pane behind the AI
proxy therefore loses Remote Control. A terminal mirror on a phone is a poor
substitute.

Claude Code 2.1.301 offers public pieces that cover the need:

- an MCP server that declares `experimental["claude/channel"]` can push prompts
  into a running interactive session;
- a `PermissionRequest` hook runs concurrently with the terminal dialog and can
  answer it, including `AskUserQuestion` (via `updatedInput`);
- the session JSONL holds the conversation.

Both the hooks and the MCP server must be passed as flags to every interactive
`claude` start, without editing the user's `~/.claude`.

## Decision

We will make Claude remote an opt-in setting. When it is on, the daemon gives
new panes `BUN_OPTIONS=--preload <okena preload.js>`. The preload runs inside the
Bun-compiled `claude` binary and appends `--settings` (Okena's hooks) and, unless
disabled, `--mcp-config` plus `--dangerously-load-development-channels
server:okena-remote` to interactive runs. The hooks and the channel are
subcommands of the `okena` binary that talk to the daemon over loopback.

## Consequences

- A `claude` started any way in a new pane is connected, including a resumed
  session, with nothing installed outside Okena's config.
- It depends on `claude` being Bun-compiled and on `BUN_OPTIONS`. The npm
  (Node) distribution is not covered. Bun has no quoting in `BUN_OPTIONS`, so the
  preload path must not contain whitespace.
- Every Bun program in the pane loads the preload; it must stay a no-op for
  anything that is not an interactive `claude` run and must never throw.
- The development-channel flag shows a confirmation dialog at every start. The
  setting `claude_remote_channel` turns the channel off; prompts are then typed
  into the PTY when Claude is idle.
- The channel API and JSONL format are undocumented and may change between
  Claude Code versions.

## Alternatives considered

- **A `claude` shim on the pane's `PATH`.** Shell init files commonly prepend
  `~/.local/bin` (where the native installer puts `claude`) after the PTY
  environment is applied, so the shim is shadowed.
- **Writing hooks and the MCP server into `~/.claude/settings.json`.** Affects
  every Claude session on the machine, not only Okena panes, and edits user
  files.
- **`CLAUDE_CODE_MANAGED_SETTINGS_PATH`.** Replaces the managed-settings layer
  meant for organisations; not ours to take over.
- **Channel permission relay (`claude/channel/permission`).** Carries only yes/no
  and skips tools that need user interaction (`AskUserQuestion`,
  `ExitPlanMode`).
- **PTY only.** Cannot answer dialogs reliably or read the conversation.
