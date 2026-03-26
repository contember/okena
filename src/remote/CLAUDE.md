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
