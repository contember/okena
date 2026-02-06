# Remote Control API

Okena includes a local HTTP/WebSocket server for remote control — useful for mobile companion apps or access via [Cloudflare Tunnel](https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/).

## Quick Start

1. Open Settings and enable **Remote Server** (under Appearance)
2. The status bar shows `REMOTE :19100 K7M2-9QFP` — click to copy the pairing code
3. Pair from another device:
   ```bash
   curl -X POST http://127.0.0.1:19100/v1/pair \
     -H 'Content-Type: application/json' \
     -d '{"code":"K7M2-9QFP"}'
   ```
4. Use the returned token for all subsequent requests

## Security

- Server **always** binds to `127.0.0.1` only — never exposed to the network
- For remote access, use a tunnel (e.g. Cloudflare Tunnel, SSH port forwarding)
- Pairing codes are 8-character base32, valid for 60 seconds, single-use
- Tokens are stored as HMAC-SHA256 digests (never plaintext) using a persistent app secret (`~/.config/okena/remote_secret`)
- Rate limiting: 5 attempts per IP per minute, 30 globally per minute
- 300ms delay on every failed pairing attempt

## Configuration

In `~/.config/okena/settings.json`:

```json
{
  "remote_server_enabled": true
}
```

When running, the server writes `~/.config/okena/remote.json`:

```json
{
  "port": 19100,
  "pid": 12345
}
```

This file is deleted on shutdown.

## API Reference

### `GET /health`

No auth required.

```json
{ "status": "ok", "version": "0.1.3", "uptime_secs": 120 }
```

### `POST /v1/pair`

No auth required (has its own rate limiting).

**Request:**
```json
{ "code": "K7M2-9QFP" }
```

**Response (200):**
```json
{ "token": "base64url-encoded-token", "expires_in": 86400 }
```

**Errors:**
- `401` — invalid or expired code
- `429` — rate limited

### `GET /v1/state`

Requires `Authorization: Bearer <token>`.

Returns the workspace state with a monotonic `state_version` counter. Clients can detect missed updates by comparing versions.

```json
{
  "state_version": 42,
  "projects": [
    {
      "id": "uuid",
      "name": "my-project",
      "path": "/home/user/my-project",
      "is_visible": true,
      "layout": {
        "type": "terminal",
        "terminal_id": "uuid",
        "minimized": false,
        "detached": false
      },
      "terminal_names": {}
    }
  ],
  "focused_project_id": "uuid",
  "fullscreen_terminal": null
}
```

Layout nodes are recursive:

| Type | Fields |
|------|--------|
| `terminal` | `terminal_id`, `minimized`, `detached` |
| `split` | `direction` (horizontal/vertical), `sizes`, `children` |
| `tabs` | `children`, `active_tab` |

### `POST /v1/actions`

Requires `Authorization: Bearer <token>`.

Tagged enum body — the `action` field selects the operation.

#### `send_text`

Write raw text to a terminal (no newline appended).

```json
{ "action": "send_text", "terminal_id": "uuid", "text": "ls -la" }
```

#### `run_command`

Write text + newline to a terminal.

```json
{ "action": "run_command", "terminal_id": "uuid", "command": "echo hello" }
```

#### `send_special_key`

Send a named key.

```json
{ "action": "send_special_key", "terminal_id": "uuid", "key": "CtrlC" }
```

Available keys: `Enter`, `Escape`, `CtrlC`, `CtrlD`, `CtrlZ`, `Tab`, `ArrowUp`, `ArrowDown`, `ArrowLeft`, `ArrowRight`, `Home`, `End`, `PageUp`, `PageDown`

#### `split_terminal`

Split a pane at a layout path.

```json
{
  "action": "split_terminal",
  "project_id": "uuid",
  "path": [0],
  "direction": "horizontal"
}
```

#### `close_terminal`

```json
{ "action": "close_terminal", "project_id": "uuid", "terminal_id": "uuid" }
```

#### `focus_terminal`

```json
{ "action": "focus_terminal", "project_id": "uuid", "terminal_id": "uuid" }
```

#### `read_content`

Get the visible terminal viewport as text.

```json
{ "action": "read_content", "terminal_id": "uuid" }
```

**Response:**
```json
{ "content": "user@host:~$ ls\nfile1  file2\nuser@host:~$ " }
```

### `WS /v1/stream`

Real-time PTY output and state change notifications.

**Authentication** — two modes:

1. **Query param:** `ws://127.0.0.1:19100/v1/stream?token=YOUR_TOKEN`
2. **First message:** connect without token, then send `{"type":"auth","token":"..."}` within 2 seconds

#### Inbound messages (client to server)

| Type | Fields | Description |
|------|--------|-------------|
| `subscribe` | `terminal_ids: string[]` | Start receiving PTY output |
| `unsubscribe` | `terminal_ids: string[]` | Stop receiving PTY output |
| `send_text` | `terminal_id`, `text` | Write text to terminal |
| `send_special_key` | `terminal_id`, `key` | Send named key |
| `ping` | — | Keepalive |

#### Outbound messages (server to client)

**JSON text frames:**

| Type | Fields | Description |
|------|--------|-------------|
| `auth_ok` | — | Authentication succeeded |
| `auth_failed` | `error` | Authentication failed |
| `subscribed` | `mappings: {terminal_id: stream_id}` | Subscription confirmed with numeric stream IDs |
| `state_changed` | `state_version: u64` | Workspace state changed — refetch via `GET /v1/state` |
| `dropped` | `count: u64` | Subscriber fell behind, N events were dropped |
| `pong` | — | Keepalive response |

**Binary frames (PTY output):**

```
[u8 proto_version=1] [u8 frame_type=1] [u32 stream_id (big-endian)] [raw PTY bytes...]
```

The `stream_id` maps to terminal UUIDs via the `subscribed` response, avoiding UUID overhead in every frame.

#### Backpressure

If a subscriber can't keep up, the server drops oldest events and sends a `dropped` message. The client should refetch state and/or resubscribe.

## Port Binding

The server tries ports 19100-19200 in order, falling back to an OS-assigned port if all are taken. The actual port is always reported in `remote.json` and the status bar.

## Architecture

```
[Tokio thread]                          [GPUI main thread]
   axum handler                            cx.spawn() loop
      |                                        |
  async_channel::Sender ──────────►  async_channel::Receiver
      |                                        |
  tokio::sync::oneshot::Receiver ◄── oneshot::Sender (reply)
```

All terminal and workspace operations go through a single bridge channel. The GPUI-side processor handles them sequentially, ensuring thread safety without locks on GPUI entities.
