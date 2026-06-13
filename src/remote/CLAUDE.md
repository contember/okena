# remote/ — Remote Control Server

HTTP/WebSocket API for controlling the application from external clients (CLI, mobile, web).

## Architecture

```
Client → HTTP/WS request
  → axum router (tokio runtime)
    → async_channel → GPUI thread
      → execute command
      → oneshot reply → response
```

The remote server runs on a separate tokio runtime. Commands cross the thread boundary via `async_channel` to execute on the GPUI thread, with results returned via `oneshot` channels.

Note: the client-side connection logic lives in `crates/okena-remote-client/`.

## Files

| File | Purpose |
|------|---------|
| `server.rs` | `RemoteServer` — starts tokio runtime, axum HTTP server. Port range 19100–19200 (auto-selects first available). Writes `remote.json` discovery file. |
| `auth.rs` | `AuthStore` — HMAC-SHA256 token auth, 6-digit pairing codes, rate limiting. |
| `bridge.rs` | `RemoteCommand` enum, `BridgeMessage` — channel factory connecting axum handlers to the GPUI thread. |
| `pty_broadcaster.rs` | tokio broadcast channel for PTY output fan-out to WebSocket clients. |
| `types.rs` | API request/response DTOs. |
| `routes/` | axum route handlers: health, pair, state, actions, stream, refresh, tokens. |

## Key Patterns

- **Thread boundary**: All mutable state access happens on the GPUI thread. The tokio server only serializes/deserializes and forwards via channels.
- **Discovery file**: `remote.json` (in config dir) contains the port and auth info so clients can auto-discover the running instance.
- **PTY fan-out**: `PtyBroadcaster` uses tokio's `broadcast` channel so multiple WebSocket clients can subscribe to the same terminal's output independently.
- **Window model**: `GET /v1/state` returns `windows` (`ApiWindow[]`) — each open OS window with its `active` flag, per-window focus (project + terminal), fullscreen, visible projects, folder filter, OS bounds, and sidebar state. Built by `Okena::build_api_windows` (`src/app/extras.rs`). The flat `focused_project_id`/`fullscreen_terminal` are derived from the active window for backward compatibility. Headless serves a single synthetic main window.
- **Per-window action targeting**: `FocusTerminal`, `SetProjectShowInOverview`, and `SetFullscreen` accept an optional `window` field (`"main"` | extra UUID). The bridge parses it (`parse_window_id`) and the `FocusManagerResolver` (`Fn(&App, Option<WindowId>) -> Option<(WindowId, FocusManager)>`) routes the action to that window's `FocusManager`; `None` targets the focused/active window, a missing window yields "window not found". See `src/app/remote_commands.rs` + `src/app/extras.rs`.

## Clients

The `okena <subcommand>` CLI (`src/cli/`, see its CLAUDE.md) is the primary agent-facing client and talks this same API. Mobile/web clients use `crates/okena-remote-client/`.
