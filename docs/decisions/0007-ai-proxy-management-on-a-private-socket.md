---
id: 0007
title: Serve AI proxy management on a private socket, not the inference port
status: accepted
date: 2026-10-21
---

# 0007 — Serve AI proxy management on a private socket, not the inference port

## Context

The AI proxy ([ADR-0004](0004-ai-proxy-process.md)) listens on a persisted
loopback TCP port, so the `ANTHROPIC_BASE_URL` of running shells stays valid
across proxy restarts. Its management routes (health, commands, shutdown)
shared that port and required a bearer key from `ai-proxy/management.key`.

The daemon sends that key every 15 seconds. While the proxy is down (crash,
upgrade, not started yet), any local OS user can bind `127.0.0.1:<port>` and
receive the key, then manage the pool: log accounts out, change policies, or
read account state. A loopback port is not owned by a user; a file in an
owner-only directory is.

## Decision

We will serve the management routes only on a transport the OS restricts to
the current user:

- Unix: the socket `ai-proxy/management.sock` inside the owner-only (0700)
  proxy directory.
- Windows: a named pipe with a current-user-only access list. Pipe names are
  machine-wide, so the name is derived from the management key, and the first
  instance is created with `FILE_FLAG_FIRST_PIPE_INSTANCE`.

The bearer key stays as a second guard. The TCP port serves inference only.

## Consequences

- Another user who takes the port while the proxy is down gets no management
  key. Shells still send their path token and their own OAuth token to whatever
  listens on the persisted port; that exposure remains (see the reference's
  Limits).
- The socket path must fit the platform limit (about 104 bytes on macOS, 108
  on Linux). A longer profile path stops the proxy from starting.
- Daemon and proxy need a Unix-socket / named-pipe HTTP transport (reqwest's
  `unix_socket` / `windows_named_pipe`, axum's listener) next to TCP.

## Alternatives considered

- **Keep TCP and authenticate the server** (e.g. a challenge the proxy answers
  with the key) — the daemon would no longer leak the key, but every management
  call needs a handshake, and inference shells gain nothing.
- **Socket in `$XDG_RUNTIME_DIR` / `$TMPDIR`** like the daemon's own socket —
  short paths, but the runtime directory is removed at logout while the proxy
  outlives the session.
- **Pick a random port per proxy start** — shells would lose the stable URL
  that ADR-0004 relies on.
