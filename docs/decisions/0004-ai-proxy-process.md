---
id: 0004
title: Run the AI account proxy as its own process
status: accepted
date: 2026-10-07
---

# 0004 — Run the AI account proxy as its own process

## Context

Okena can spread Claude Code requests over several subscription accounts by
pointing `ANTHROPIC_BASE_URL` of new terminals at a local proxy that swaps in the
credentials of a pool account. Terminals persisted by dtach/tmux outlive the
workspace daemon, and a `claude` process keeps the base URL it was started with.
A proxy inside the daemon would break every such session on each daemon restart
or update, and would cut responses that are being streamed at that moment.

[ADR-0001](0001-headless-two-process-daemon.md) makes the daemon the owner of all
authoritative state.

## Decision

We will run the proxy as a separate process, `okena --ai-proxy <profile-root>`
(the standalone `okena-daemon` binary accepts the same flag), one per profile.
The daemon starts it on demand and never stops it. Its loopback port and secret
path token are persisted in the profile, so a shell's URL stays valid across
proxy restarts. The proxy owns the account pool (credentials, refresh, quota
observations); clients reach it only through the daemon's `ai_proxy` action.

When the daemon finds a proxy of another version, it replaces it only while no
request is in flight; otherwise the old proxy keeps serving.

## Consequences

- Daemon restarts and app updates do not interrupt `claude` sessions or their
  in-flight responses.
- Provider credentials are the one piece of state the daemon does not own — an
  explicit exception to ADR-0001.
- No extra binary to ship: the proxy reuses the `okena`/`okena-daemon`
  executable, so installers and the updater are unchanged.
- A crash of the proxy breaks in-flight responses; the next daemon start or AI
  settings action starts a new one on the same URL.

## Alternatives considered

- **Proxy inside the daemon** — simplest, but every daemon restart breaks
  persisted `claude` sessions.
- **Separate `okena-ai` binary** — the same isolation, plus packaging, updater
  and CI changes for every platform.
- **Proxy under dtach/tmux** — ties a network service to a terminal persistence
  backend that is optional and platform-specific.
