# okena-ai — per-profile Claude account proxy

GPUI-free. Runs as `okena --ai-proxy <profile-root>` / `okena-daemon --ai-proxy …`
(gated at the top of both `main`s via `proxy_args`). User-facing behaviour:
`docs/reference/ai-proxy.md`; why a separate process: ADR-0004.

| File | Purpose |
|---|---|
| `server.rs` | Process entry (`run`): instance lock, logging, bind, `/<token>[/p/<project>]/…` dispatch, management routes (`/_okena/health`, `/_okena/command`, `/_okena/shutdown`, bearer `management.key`), upstream silence timeout, usage poller. E2E tests against a fake provider live here. |
| `proxy.rs` | Relay: account selection per attempt, auth/`metadata.user_id` rewrite, 401 refresh-once, 429 failover, streaming with an in-flight guard. |
| `accounts.rs` | Pool state: `accounts.json` (credentials), `routing.json` (per-project policies), policy-aware selection, runtime quota/cooldown, conversation affinity, serialized token refresh, OAuth login bookkeeping. |
| `oauth.rs` | Claude OAuth (PKCE manual-code flow, refresh, profile plan, usage endpoint) — constants mirror the `claude` CLI. |
| `client.rs` | Daemon side: `ensure_running` (start detached / replace an idle older version) and management calls. |

The wire contract (endpoint file, `AiProxyCommand`, `AiProxyStatus`, `AiProxyState`)
is in `okena-core::ai_proxy`; the daemon glue is `okena-daemon-core/src/ai_proxy.rs`.

## Rules

- The proxy must outlive the daemon: spawn detached (`setsid` / `DETACHED_PROCESS`),
  never stop it from the daemon except via an idle `/_okena/shutdown` on version change.
- The endpoint (port + token) is stable; shells keep the URL they were started with.
- The shell token authorizes inference only. Management goes through the
  separate `management.key`; never put that key into a shell's environment.
- Upstream timeouts must stay longer than the CLI's own (request 10 min, stream
  idle 3 min), so the proxy never cuts a request the CLI would still wait for.
- `accounts.json` holds the only copy of refresh tokens: a corrupt file is an
  error, never an empty pool. Refresh tokens are single-use — refreshes go through
  the per-account lock in `Accounts::fresh_token`.
- Never retry a request after its response started streaming.
- A policy that forbids an account never falls back to the caller's own
  credentials (`Selection::NoPermittedAccount`). A corrupt `routing.json` is an
  error, never an open policy. Why policies live here: ADR-0005.
- Tests use loopback fakes and temp profiles; never read the developer's real
  Claude credentials.

```bash
cpu-lease run -n 2 -- cargo test -p okena-ai
cargo tree -p okena-ai -i gpui   # must be empty
```
