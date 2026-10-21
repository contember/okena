# remote/ — Remote Control Server

HTTP/WebSocket API for controlling the application from external clients (CLI, mobile, web).

## Architecture

```
Client → HTTP/WS request
  → axum router (tokio runtime)
    → async_channel → daemon reactor (LocalSet)
      → execute command
      → oneshot reply → response
```

This server runs **only inside the daemon** (`okena-daemon-core`), never in the
desktop process — the GUI stopped self-hosting it when it became a thin client
(ADR-0001). The crate compiles gpui-free with `default-features = false`; the
`gpui` feature only forwards to `okena-workspace` for the GUI-side consumer.

Commands cross from the axum tokio runtime to the daemon's reactor via
`async_channel`, with results returned over `oneshot`. Client-side connection
logic lives in `crates/okena-remote-client/` (desktop) and `okena-transport`
(shared engine).

## Files

| File | Purpose |
|------|---------|
| `server.rs` | `RemoteServer` — starts tokio runtime, axum HTTP server. Port range 19100–19200 (auto-selects first available). Writes `remote.json` discovery file. |
| `serve.rs` | Server bootstrap/run loop. |
| `local.rs` | Local-daemon toolkit: `discover()`, `running_daemon()`, `is_process_alive()`, `mint_local_token()`. |
| `tls.rs` | TLS setup + cert generation for standalone (non-loopback) deployments. |
| `auth.rs` | `AuthStore` — HMAC-SHA256 token auth, 6-digit pairing codes, rate limiting. |
| `bridge.rs` | `RemoteCommand` enum, `BridgeMessage` — channel factory connecting axum handlers to the daemon reactor. |
| `pty_broadcaster.rs` | tokio broadcast channel for PTY output fan-out to WebSocket clients. |
| `types.rs` | API request/response DTOs. |
| `claude_remote/` | `ClaudeRemoteHub`: open permission dialogs (hook long-polls), prompt delivery (MCP channel / PTY / queue until `Stop`), session state from hook events, incremental JSONL transcript reader. Routes in `routes/claude.rs`; see `docs/reference/claude-remote.md`. |
| `usage.rs` | `UsageHub` — agent usage `watch` snapshot + refresh-request channel shared with the daemon's usage poller; merges the AI proxy pool (`set_pool`) into the Claude entry. |
| `routes/` | axum route handlers: health, pair, state, actions, stream, refresh, tokens, usage. |

## Key Patterns

- **Thread boundary**: All mutable state access happens on the daemon reactor's `LocalSet`. The tokio server only serializes/deserializes and forwards via channels.
- **Discovery file**: `remote.json` (in config dir) contains the port and auth info so clients can auto-discover the running instance.
- **PTY fan-out**: `PtyBroadcaster` uses tokio's `broadcast` channel so multiple WebSocket clients can subscribe to the same terminal's output independently.
- **Window model**: `GET /v1/state` returns `windows` (`ApiWindow[]`). The daemon serves a **single synthetic main window** (`command_loop.rs`, `GetState` arm) with `visible_project_ids` populated and the presentation fields (`focused_project_id`, `focused_terminal_id`, `fullscreen`, `bounds`, `folder_filter`, `sidebar_open`) left `None` — those are client-owned by design, not missing. The flat `focused_project_id`/`fullscreen_terminal` stay for backward compatibility.
- **Per-window action targeting**: `FocusTerminal`, `SetProjectShowInOverview`, and `SetFullscreen` accept an optional `window` field (`"main"` | extra UUID), parsed by `WindowId::from_wire` (`okena-state`). On the daemon, `None` and `"main"` target its synthetic main window; an extra UUID yields "window not found" (except for `FocusTerminal`, which passes it through to desktops).
- **Desktop presentation is a one-shot push, never snapshot state.** Desktops own per-window focus, hidden sets, and fullscreen; the daemon only owns its synthetic main window. After a REST `FocusTerminal`, `SetProjectShowInOverview`, or `SetFullscreen` succeeds, `routes/actions.rs` sends a `ClientPresentationRequest` (`okena-core/src/ws.rs`) on `presentation_tx`; every WS stream forwards it as `WsOutbound::TerminalFocusRequested` / `ProjectVisibilityRequested` / `FullscreenRequested`. A failed action pushes nothing. Desktop clients prefix the ids and apply it to the requested window, else their active window (`okena-app/src/app/presentation_requests.rs`, `Okena::jump_to_terminal`); show/hide sets the requested state (idempotent), and a project not synced yet waits in `RemoteSyncState` until it materializes. Do not put these into state snapshots: replaying them on reconnect would steal focus and, for visibility, clobber the desktop's own hidden sets (see ed6137c9).

- **Agent usage** (`GET /v1/usage`, `POST /v1/usage/refresh`, `GET /v1/usage/pool/{account_id}`, WS `usage_changed`): the daemon's `usage_poll` (okena-daemon-core) fetches Claude Code / Codex / Grok usage and publishes full `ApiUsage` snapshots into `UsageHub`; the daemon's `ai_proxy` accounts poll adds the account pool with `UsageHub::set_pool` and each account's windows with history with `UsageHub::set_pool_histories` (served by the pool route, never pushed), and every published change gets a `generated_at` no older than the snapshot it replaces. Routes read the latest snapshot; refresh enqueues a request with a oneshot ack (the poller throttles to 60 s per provider and respects a 429 `retry-after`; an error backoff only spaces the automatic poll) and returns the snapshot once the poller has applied and published it (2 s cap), so `fetching` answers "did a fetch start". Every WS stream registers a `UsageClient` (both polls pause their cadence while none is registered), sends the snapshot right after `auth_ok`, then on each change (`send_if_modified` ignores `generated_at`-only changes). Wire types and their docs: `okena-core/src/usage.rs`; user-facing reference: `docs/reference/remote.md`.

## Clients

The `okena <subcommand>` CLI (`crates/okena-cli/`, see its CLAUDE.md) is the primary agent-facing client and talks this same API. Mobile/web clients use `crates/okena-remote-client/`.
