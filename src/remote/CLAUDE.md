# remote/ — Remote Control Server

HTTP/WebSocket API for controlling the application from external clients (CLI, scripts, other tools).

## Architecture

```
Client → HTTP/WS request
  → axum router (tokio runtime)
    → async_channel → GPUI thread
      → execute command
      → oneshot reply → response
```

The remote server runs on a separate tokio runtime. Commands cross the thread boundary via `async_channel` to execute on the GPUI thread, with results returned via `oneshot` channels.

## Files

| File | Purpose |
|------|---------|
| `mod.rs` | Module re-exports. |
| `server.rs` | `RemoteServer` — starts tokio runtime, axum HTTP server. Port range 19100–19200 (auto-selects first available). Writes `remote.json` discovery file. |
| `auth.rs` | `AuthStore` — HMAC-SHA256 token auth, 6-digit pairing codes for initial setup, rate limiting on failed attempts. |
| `bridge.rs` | `RemoteCommand` enum, `BridgeMessage` — channel factory connecting axum handlers to the GPUI thread. |
| `pty_broadcaster.rs` | tokio broadcast channel for PTY output fan-out — streams terminal output to multiple connected WebSocket clients. |
| `types.rs` | API request/response types — serializable DTOs for all endpoints. |
| `routes/` | axum route handlers. |

### routes/

| File | Purpose |
|------|---------|
| `mod.rs` | Router construction, middleware setup. |
| `health.rs` | `GET /health` — server health check. |
| `pair.rs` | `POST /pair` — pairing flow with 6-digit code → auth token exchange. |
| `state.rs` | `GET /state` — full workspace state snapshot. |
| `actions.rs` | `POST /actions/*` — execute workspace actions (send text, run command, switch project, etc.). |
| `stream.rs` | `GET /stream` — WebSocket endpoint for real-time terminal output streaming. |

## Key Patterns

- **Thread boundary**: All mutable state access happens on the GPUI thread. The tokio server only serializes/deserializes and forwards via channels.
- **Discovery file**: `remote.json` (in config dir) contains the port and auth info so CLI clients can auto-discover the running instance.
- **PTY fan-out**: `PtyBroadcaster` uses tokio's `broadcast` channel so multiple WebSocket clients can subscribe to the same terminal's output independently.
