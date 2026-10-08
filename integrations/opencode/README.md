# Okena integration for OpenCode V2

This local plugin supplies daemon-owned mission context to OpenCode V2 and reports
the selected session's lifecycle to its Okena pane.

## Install

1. Build or install an Okena version supporting OpenCode session IDs and
   `okena mission context`. Put `okena` on the OpenCode server's `PATH`.
2. Install this directory's dependencies with `npm ci`.
3. Add the absolute path to this directory to OpenCode's global or project
   `opencode.jsonc`:

   ```jsonc
   {
     "plugins": ["/path/to/okena/integrations/opencode"],
   }
   ```

4. Start OpenCode's full-screen TUI inside an Okena pane. Existing OpenCode clients
   may need a restart after changing the plugin configuration.

The directory contains both `index.ts` and `tui.ts`, which OpenCode V2 discovers
as server and TUI entrypoints. Configuring the directory loads both parts.
No global OpenCode configuration is changed by this repository.

## Session binding

The TUI reads `OKENA_TERMINAL_ID`, `OKENA_PROFILE`, and `XDG_CONFIG_HOME` from its
own process. It sends the selected session's explicit pane/profile binding through
the plugin's `okena.bind` RPC. The server never infers a pane from its environment,
working directory, desktop focus, or another client's active session.

Bindings persist in OpenCode's native plugin storage. They contain routing data,
not mission membership or a cached mission briefing. Selecting or resuming a
session rebinds it to the current pane. If several TUIs select the same session,
the last successful binding wins. `/okena-bind` repeats the binding explicitly.
Both processes must run on the same host and under the same user account.

Unbound subagents use their nearest bound ancestor's pane and conversation
assignment. Forks are independent and need their own binding. Sessions without a
binding or bound ancestor receive no Okena context. `opencode run`, `mini`, the
web client, and API clients do not run this TUI adapter; they can invoke the RPC
explicitly using the exported contract in `./rpc`.

## Context delivery

Native `context` and `compaction` hooks query `okena mission context` for the bound
pane and conversation. The current briefing is added to each outgoing model
request, including tool continuations. It is not appended repeatedly to durable
conversation history. Changes to the goal, membership, or detach state appear on
the next model request. An unassigned pane receives an explicit no-mission message.

The CLI supplies the bounded briefing, including goal, checkout paths, observed
branches, and retained conversation identities. Okena's daemon remains the sole
authority for membership, mission lifecycle, and attention. The plugin neither
reads transcripts nor changes mission membership.

Lookups have a three-second timeout. Failed lookups add an unavailable-context
message and log the error; they never substitute a cached briefing or stop model
execution. The plugin does not inject context into automatic title generation.

The injected context also includes the pane's routing environment and explicit
conversation ID for on-demand CLI calls. Apply that environment when running
`okena` from a shell tool: shell tools run on OpenCode's shared server and do not
automatically inherit the selected TUI's Okena profile or terminal.

## Lifecycle

The TUI reports the selected session through Okena's existing OSC 9001 protocol:

| OpenCode event                              | Okena state                            |
| ------------------------------------------- | -------------------------------------- |
| Execution starts                            | `working`                              |
| Permission or form requested                | `blocked`                              |
| All input requests resolved                 | `working` if running, otherwise `idle` |
| Execution succeeds                          | `done`                                 |
| Execution fails or is interrupted           | `idle`                                 |
| Session selection changes or plugin unloads | `clear`                                |

Each report contains `agent=opencode`, its native `ses_…` session ID, and the
terminal ID. Writes use the current TTY pointer (`OKENA_TTY_FILE`) when available,
so reattaching a surviving pane does not reuse its old device. Session IDs are
bounded and validated by Okena on capture, load, and mission API requests. Claude
Code and Codex keep their UUID validation.
Resuming checks the session's permission and form queues to preserve pending input.

## Development

```sh
npm ci
npm run check
npm test
npm run lint
```

The plugin uses the V2 API from `@opencode/plugin`, not V1 hook exports. The
integration is tested against `opencode2 v0.0.0-beta-19242`. That runtime supports
native plugin storage but does not expose the session metadata update method
described by V2 documentation and the type package.
