# okena-lifecycle — Claude Code plugin

Reports Claude Code's lifecycle to [Okena](https://github.com/contember/okena) so
the pane's tab, the sidebar **Agents** section, and desktop notifications reflect
what the agent is doing. It does this by emitting Okena's agent-status escape
sequence (`OSC 9001`) to the terminal on lifecycle events — no network, no
config files written, works only inside an Okena pane (a silent no-op elsewhere).

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

`PreToolUse` / `PostToolUse` are the recovery edges: when you answer a blocked
agent (permission grant, or a question mid-turn) Claude Code does **not** fire
`UserPromptSubmit`, so without them the pane stays stuck on `blocked` while the
agent is actually busy again.

See [`docs/agent-status.md`](../../../docs/agent-status.md) for the full model,
the `OSC 9001` wire format, and debugging (the `OKENA_AGENT_STATUS_LOG` env var).
The bundled `scripts/okena-agent-status.sh` is agent-agnostic — anything that can
run a command can call it directly.
