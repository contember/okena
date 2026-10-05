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
- Tokens are stored as HMAC-SHA256 digests (never plaintext) using a persistent app secret (`remote_secret` in your platform's config dir)
- Rate limiting: 5 attempts per IP per minute, 30 globally per minute
- 300ms delay on every failed pairing attempt

## Configuration

In `settings.json` in your platform's config dir (macOS: `~/Library/Application Support/okena/`, Linux: `~/.config/okena/`):

```json
{
  "remote_server_enabled": true
}
```

When running, the server writes `remote.json` to the same config dir:

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

The example below is a subset of the full project shape. One field worth calling
out: `terminal_agent_status` maps terminal id → `{ "lifecycle", "custom"?,
"labels"? }` for panes running an AI agent (see [Agent Status](agent-status.md)).
It is **runtime-only** — omitted entirely for terminals with no status, and
absent after a daemon restart — and a change to it bumps `state_version`.

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
      "terminal_names": {},
      "terminal_agent_status": {
        "uuid": { "lifecycle": "working", "custom": "running tests 3/5" }
      }
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

#### Work overview capability

`StateResponse.work_overview` is an optional, additive field. **Presence is the
capability signal**, including an empty object with empty collections; absence or
`null` means that the daemon does not advertise missions/shared attention.
Clients must not substitute local mission or acknowledgment writes for this
missing capability. Existing `terminal_agent_status` lifecycle meanings are
unchanged.

A supported daemon returns this shape (empty example):

```json
{
  "work_overview": {
    "missions": [],
    "attention": [],
    "lost_transitions": 0,
    "conversations": [],
    "conversation_history": [],
    "terminal_bindings": []
  }
}
```

| Field | Contents |
|---|---|
| `missions` | Ordered mission records: ID, title, goal, home project, creation time, lifecycle (`active`, `done`, `archived`), repository/worktree IDs, conversation identities and retained PR observations. |
| `attention` | Unread episodes, input-needed before completions, then oldest first. Each includes `id`, `revision`, `source`, `kind` (`input_needed` or `completion`), `summary`, `created_at`, `updated_at`, `available`, `terminal_available`, and `read`. |
| `lost_transitions` | Cumulative capture losses and anonymous-source retention evictions. This overview is not a complete event history. |
| `conversations` | Current `{project_id, terminal_id, conversation}` attachments, not the entire conversation-history registry. |
| `conversation_history` | Retained `{agent, session_id}` identities, including detached offline conversations that can be attached to a mission again. Defaults to empty for older daemons. |
| `terminal_bindings` | Effective `{project_id, terminal_id, mission_id}` membership, including worktree defaults and explicit exclusions already resolved by the daemon. |

A conversation is `{agent, session_id}`. An attention source also has
`project_id`, `terminal_id`, `attachment_id`, `generation` and optional
`conversation`. Work-overview DTOs do not include local transcript paths or
transcript contents. Reserved session labels are also stripped from
`terminal_agent_status.labels`; raw PTY output can still contain the original OSC
bytes. This projection is not a redaction of the terminal stream.

Attention `available` tracks confirmed source status. `terminal_available` tracks
whether the original terminal attachment can still be opened after `clear`.
Clients reading an older daemon without `terminal_available` may use `available`
for navigation; an unavailable episode alone does not prove its terminal survives.

Mission PR records use `{host, repository, number}` identity and contain `info`,
optional `ci`, `observed_at`, `available` and `source_project_ids`. Unavailable
observations retain last-known values; they are not evidence of a closed PR or
healthy CI.

Mission/attention mutations use the normal state-change notification and refetch
path. Reconnect rehydrates episodes rather than synthesizing new completions.
See [Missions](missions.md) and [shared attention](agent-status.md#shared-attention-inbox)
for lifecycle, retention and restart semantics.

**Connection separation:** all IDs in a daemon response are raw IDs scoped to
that daemon. A multi-daemon client must retain `(connection_id, raw_id)` for
missions, projects, terminals and episodes, including saved selection and delayed
acknowledgments. Desktop terminal keys use `remote:<connection>:<raw-id>` locally;
these prefixed keys are not daemon-persisted identities. Send actions to the
owning connection with its raw IDs. Equal IDs on different daemons do not imply
shared membership. A disconnected owner's retained overview is read-only and
unavailable; another connected daemon cannot acknowledge it.

### `POST /v1/actions`

Requires `Authorization: Bearer <token>`.

Tagged enum body — the `action` field selects the operation.

#### Mission and attention actions

Use these only when the owning daemon advertises `work_overview`.

```json
{
  "action": "mission",
  "command": {
    "operation": "create",
    "title": "Review the parser change",
    "goal": null,
    "home_project_id": null,
    "member": null
  }
}
```

The nested `command.operation` selects the mutation:

| Operation | Fields |
|---|---|
| `create` | `title`, optional `goal`, optional `home_project_id`, optional initial `member` |
| `edit` | `mission_id`, `title`, optional `goal`, optional `home_project_id` (replaces these details; omitted optional values become `null`) |
| `set_lifecycle` | `mission_id`, `lifecycle`: `active`, `done`, or `archived` |
| `attach`, `detach`, `move` | `mission_id`, `member` |

Members are tagged by `kind`: `repository` or `worktree` carries `project_id`;
`terminal` carries `project_id` and `terminal_id`; `conversation` carries
`conversation: {agent, session_id}`. References must exist on the owning daemon;
conversation attachment requires a known validated history identity. Conflicting
primary membership requires `move`, not `attach`. Successful mission actions
return `mission_id` in their result data.

`create_worktree` has an optional `mission_id` alongside `project_id`, `branch`
and `create_branch`. Missing or `null` preserves ordinary worktree creation.
When supplied, the daemon validates ownership before creation and attaches the
registered worktree to that mission.

To mark a completion read:

```json
{
  "action": "acknowledge_attention",
  "episode_id": "observed-episode-uuid",
  "revision": 1,
  "dismiss": false
}
```

Use the exact ID/revision from the observed snapshot. The result data contains
`changed`: `false` includes duplicate, stale and ineligible acknowledgments.
`dismiss` defaults to `false`. Setting it to `true` also permits dismissal of an
unavailable input-needed episode; it cannot clear a live input request. Read state
is daemon-shared and persisted. A new revision or replacement episode requires a
new acknowledgment. A plain `focus_terminal` request is not an acknowledgment.

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
[HTTP/WebSocket server]                 [Daemon command loop]
   axum handler                            sequential processor
      |                                        |
  async_channel::Sender ──────────►  async_channel::Receiver
      |                                        |
  tokio::sync::oneshot::Receiver ◄── oneshot::Sender (reply)
```

Terminal and workspace mutations go through the bridge to the daemon command
loop. The daemon owns authoritative workspace state, PTYs, missions and shared
attention; desktop GPUI views consume snapshots and submit actions. See
[ADR-0001](../decisions/0001-headless-two-process-daemon.md).
