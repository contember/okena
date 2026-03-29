# Headless Mode

Okena can run without a GUI, exposing its full workspace functionality via the HTTP/WebSocket API. This is useful for remote servers, containers, and SSH-only environments.

## Starting Headless Mode

```bash
# Explicit headless mode
okena --headless --listen 0.0.0.0

# Headless on localhost only
okena --headless --listen 127.0.0.1
```

On Linux, headless mode is **auto-detected** when `--listen` is provided and no display server is available (`DISPLAY` and `WAYLAND_DISPLAY` are both unset):

```bash
# Auto-detects headless on a displayless server
okena --listen 0.0.0.0
```

## What Runs in Headless Mode

All core functionality works without a GUI:

- **Workspace management** -- projects, layouts, worktrees
- **PTY manager** -- terminal sessions with session persistence (tmux/dtach)
- **Git status watcher** -- background git monitoring
- **Service manager** -- Docker Compose and Okena services
- **Remote server** -- HTTP/WebSocket API for external control
- **Authentication** -- token-based pairing for remote clients
- **PTY broadcaster** -- streams terminal output to connected clients

## Connecting to a Headless Instance

### From Okena Desktop

1. Start the headless server: `okena --headless --listen 0.0.0.0`
2. Generate a pairing code: `okena pair` (on the server)
3. In the desktop app, open the remote connection dialog
4. Enter the server address, port, and pairing code

### From the CLI

```bash
# On the server
okena --headless --listen 0.0.0.0

# From any terminal on the same machine
okena state          # View workspace state
okena services       # List services
okena health         # Check server health
```

### Via REST API

See [Remote Control API](remote.md) for the full API reference.

## Use Cases

- **Remote development** -- run Okena on a dev server, connect from your desktop
- **Docker containers** -- persistent terminal sessions inside containers
- **CI/CD** -- manage services and terminals programmatically
- **SSH servers** -- multiplexed terminal sessions accessible via the API
- **Tunneling** -- combine with Cloudflare Tunnel or SSH port forwarding for remote access

## Instance Locking

Only one Okena instance can run at a time per config directory. The lock file at `~/.config/okena/okena.lock` prevents duplicate instances.
