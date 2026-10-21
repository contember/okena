# okena-ai — per-profile Claude account proxy

GPUI-free. Runs as `okena --ai-proxy <profile-root>` / `okena-daemon --ai-proxy …`
(gated at the top of both `main`s via `proxy_args`). User-facing behaviour:
`docs/reference/ai-proxy.md`; why a separate process: ADR-0004.

| File | Purpose |
|---|---|
| `server.rs` | Process entry (`run`): instance lock (waits up to 45 s for an instance that is shutting down), logging, loopback TCP bind with `/<token>[/p/<project>]/…` dispatch, management routes on the management address (`/_okena/health`, `/_okena/command`, `/_okena/shutdown`, bearer `management.key`), upstream silence timeout, usage poller. E2E tests against a fake provider live here. |
| `management.rs` | Management address: `ai-proxy/management.sock` (Unix; a checked 0700 `okena-ai-<hash>/` under the runtime dir when that path is too long for a socket) or a current-user-only named pipe whose name derives from the management key (Windows); listener and the HTTP client for it. Proxy and daemon both resolve it through `Address::new`. |
| `proxy.rs` | Relay: account selection per attempt, auth/`metadata.user_id` rewrite, 401 refresh-once, 429 failover, streaming with an in-flight guard, activity recording (error answers are read whole for their message). |
| `diagnostics.rs` | Bounded provider error details and request context for relay logs; only structured error fields and selected response headers are logged. |
| `activity.rs` | In-memory last hour of `/v1/messages` outcomes per account and the recent failover/error events, sent with every status. |
| `accounts.rs` | Pool state: `accounts.json` (credentials), `routing.json` (per-project policies), policy-aware selection, runtime quota/cooldown, conversation affinity, serialized token refresh, OAuth login bookkeeping. |
| `oauth.rs` | Claude OAuth (PKCE manual-code flow, refresh, profile plan, usage endpoint) — constants mirror the `claude` CLI. |
| `client.rs` | Daemon side: `ensure_running` (start detached / replace an idle older version, wait for the new one) and management calls. |

The wire contract (endpoint file, `AiProxyCommand`, `AiProxyStatus`, `AiProxyState`)
is in `okena-core::ai_proxy`; the daemon glue is `okena-daemon-core/src/ai_proxy.rs`.

## Rules

- The proxy must outlive the daemon: spawn detached (Linux: `systemd-run --user
  --scope` when it works, plus `setsid`; Windows: `DETACHED_PROCESS` and
  `CREATE_BREAKAWAY_FROM_JOB` when the job allows it), never stop it from the
  daemon except via an idle `/_okena/shutdown` on version change.
- The endpoint (port + token) is stable; shells keep the URL they were started with.
- The shell token authorizes inference only. Management is never served on the
  TCP port, only on the management address, and also needs the separate
  `management.key`; never put that key into a shell's environment.
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
