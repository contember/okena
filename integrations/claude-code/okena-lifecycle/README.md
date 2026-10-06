# okena-lifecycle — Claude Code plugin

Reports Claude Code's lifecycle to [Okena](https://github.com/contember/okena) so
the pane's tab, the sidebar **Agents** section, and desktop notifications reflect
what the agent is doing. It does this by emitting Okena's agent-status escape
sequence (`OSC 9001`) to the terminal on lifecycle events. Inside an Okena pane,
it also reads mission context from the owning daemon through the `okena` CLI and
delivers a short briefing directly to Claude's context. Outside Okena it is a
silent no-op.

## Install

From a clone of the okena repo (the `integrations/claude-code` dir is the
marketplace):

```
/plugin marketplace add ./integrations/claude-code
/plugin install okena-lifecycle@okena
```

Or enable it non-interactively in `~/.claude/settings.json`:

```json
{
  "enabledPlugins": { "okena-lifecycle@okena": true }
}
```

## What it maps

| Claude Code hook | Reported state |
|------------------|----------------|
| `UserPromptSubmit` | `working` |
| `PreToolUse` | `working` (about to run a tool) |
| `PostToolUse` | `working` (tool finished — work resumes) |
| `Notification` | `blocked` (needs permission / input) |
| `Stop` | `done` |
| `SessionStart` | `clear` (reset stale status) |
| `SessionEnd` | `clear` (agent exited) |

## Mission context

Use plugin version **0.3.0** with an Okena binary and daemon that support
`okena mission context`. The binary must be on `PATH`. Older binaries continue
to receive lifecycle reports; the plugin checks command support through `help`
before querying context.

- `SessionStart` supplies the current briefing on startup, resume and compaction.
- `UserPromptSubmit` checks for changes before each prompt.
- `PreToolUse` and `PostToolUse` check during long turns, at most once per second
  for each pane/session/subagent. Only changed briefings are injected.
- `SubagentStart` supplies a fresh briefing to the child, without replacing the
  pane's parent-agent lifecycle identity.
- `SessionEnd` removes that session's delivery cache.

The hook output uses Claude Code's `hookSpecificOutput.additionalContext`; it
does not type into the terminal or create a user prompt. The briefing contains
the mission title, goal, lifecycle, home context, current checkout, participating
checkouts and retained conversation identities. Attachments mean last-reported
panes, not verified running agents. It does not read transcript contents.

Assignment changes and detach are delivered on the next hook check. Resume and
compaction always refresh, including an explicit no-assignment message that
invalidates a previously supplied mission. The hook passes its own session ID
so a new agent does not inherit the pane's previous conversation assignment.

The text briefing is capped at 6000 bytes. Structured context includes up to 12
projects and 12 conversations, with omission counts; `okena state` exposes the
full inventory. The private, profile-local `mission-context/` directory stores
the last delivered briefing for deduplication, not authoritative mission state.

Context lookup, cache or CLI failures never block Claude and never reuse a
cached briefing as fresh context. Diagnostics go to stderr. Lifecycle reporting
still works when the context CLI is missing or the daemon is unavailable.

To inspect context manually:

```sh
okena mission context
okena mission context --json
okena mission context --agent claude-code --session-id <UUID>
```

Verified locally with Claude Code **2.1.291**: a print-mode session loaded this
plugin and received a unique mission title through the real daemon and CLI.
An isolated daemon/CLI smoke check also exercised startup, unchanged-hook
deduplication, attach, edit, move, detach, resume/compact `SessionStart` events,
subagent start and session-end cache cleanup. Resume/compaction were supplied
hook events in that smoke check, not an interactive compaction run.

`PreToolUse` / `PostToolUse` are the recovery edges: when you answer a blocked
agent (permission grant, or a question mid-turn) Claude Code does **not** fire
`UserPromptSubmit`, so without them the pane stays stuck on `blocked` while the
agent is actually busy again.

See [`docs/reference/agent-status.md`](../../../docs/reference/agent-status.md) for the full model,
the `OSC 9001` wire format, and debugging (the `OKENA_AGENT_STATUS_LOG` env var).
The bundled `scripts/okena-agent-status.sh` is agent-agnostic — anything that can
run a command can call it directly.
