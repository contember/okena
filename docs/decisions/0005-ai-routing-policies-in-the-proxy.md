---
id: 0005
title: Keep AI routing policies in the proxy, keyed by a project segment in the URL
status: accepted
date: 2026-10-08
---

# 0005 — Keep AI routing policies in the proxy, keyed by a project segment in the URL

## Context

The AI proxy ([ADR-0004](0004-ai-proxy-process.md)) spreads `claude` requests
over a pool of subscription accounts. Some projects must use one account, may
not use another, or should prefer one. The proxy needs two things for that:
which project a request comes from, and that project's policy.

All shells used to get the same `ANTHROPIC_BASE_URL`, so the proxy could not
tell projects apart. Per-project settings normally live in `workspace.json`,
owned by the daemon ([ADR-0001](0001-headless-two-process-daemon.md)). But the
proxy outlives the daemon: `claude` sessions in dtach/tmux keep sending requests
while the app is closed, and a "never use this account" policy must hold then
too.

## Decision

We will give each project terminal a base URL that names its project,
`http://127.0.0.1:<port>/<token>/p/<project-id>`. A worktree terminal names its
main project. The proxy looks the policy up on every request, so a policy change
applies to running sessions with their next request.

We will store the policies in the proxy (`<profile>/ai-proxy/routing.json`),
next to the accounts they refer to, and manage them through `AiProxyCommand`
like the accounts. A policy that forbids any account never falls back to the
terminal's own Claude login, because that login may be the forbidden account.

## Consequences

- Policies hold while the daemon is down, with no copy to keep in sync.
- Removing an account removes its rules in the same process that owns both.
- Policies are a second piece of state the daemon does not own, next to the
  provider credentials of ADR-0004.
- A deleted project leaves its policy in `routing.json` until it is removed in
  Settings → AI Proxy; the policy is inert because no new terminal carries that id.
- The project in the URL routes requests; it is not an access boundary. A process
  of the same user can put another project's id into its URL.
- Terminals started without a project segment (hook and service terminals, and
  shells opened before this change) use the default policy.

## Alternatives considered

- **Policy in `ProjectData`, pushed to the proxy** — keeps per-project data in
  `workspace.json` and cleans up with the project, but the proxy still needs a
  persisted copy for when the daemon is down, plus a sync on every change and
  every proxy start.
- **Project in `ANTHROPIC_CUSTOM_HEADERS`** — the same plumbing, but it replaces
  a value the user may set, and SDKs other than `claude` do not read it.
- **Infer the project from the request** (working directory in the prompt,
  session id mapped to a terminal) — fragile, and the session id is known only
  after the first request.
- **Policy encoded in the URL itself** — a policy change would not reach running
  sessions.
