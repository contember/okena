# AI proxy (Claude account pool)

Okena can route `claude` running in its terminals through a local proxy that
spreads requests over several Claude subscription accounts. Why it is a
separate process: [ADR-0004](../decisions/0004-ai-proxy-process.md). Code:
`crates/okena-ai` (see its `CLAUDE.md`).

## Use it

1. **Settings → AI Proxy**, turn on *Route Claude Through the AI Proxy*.
2. Under **Add Account**, enter a name and click **Log In**. Open the login
   page, sign in with the account to add, and paste the code it shows into the
   field, then click **Finish**. Repeat for more accounts.
3. Open a new terminal and run `claude` as usual.

Turning the proxy on affects **new** terminals only: they get
`ANTHROPIC_BASE_URL=http://127.0.0.1:<port>/<token>`. Running terminals keep
their environment; turning the proxy off leaves the proxy running for them.

`claude` itself still needs its normal login (it refuses to start without one).
That login is used only when the pool has no enabled, logged-in account: then
the proxy forwards requests with the terminal's own credentials.

## How requests are routed

- `/v1/messages*` gets a pool account: the `authorization` header is replaced
  with that account's OAuth token and `metadata.user_id.account_uuid` is
  rewritten to the serving account. Everything else (OAuth, profile, telemetry)
  passes through untouched.
- A conversation (`x-claude-code-session-id`) stays on the account that served
  it last while that account is usable, so prompt caches keep working.
  Otherwise the account with the lowest 5-hour utilization is chosen.
- **429**: the account cools down until the provider's reset time
  (`anthropic-ratelimit-unified-reset`), else `retry-after`, else one minute,
  and the request moves to the next account. When all accounts are limited, the
  provider's 429 is returned and `claude` waits and retries itself.
- **401**: the access token is refreshed once and the request retried; if the
  refresh token is rejected the account is marked *Needs login*.
- A request is never retried after its response started streaming.

Utilization comes from the `anthropic-ratelimit-unified-5h/7d-*` response
headers and from polling the usage endpoint every five minutes.

## Lifecycle

- The daemon starts the proxy when the setting is on (at startup and when the
  setting changes) and whenever the AI Proxy settings page sends a command.
- The proxy holds `ai-proxy/proxy.lock`; a second instance exits immediately.
- It is never stopped by the daemon, the GUI or an update. A daemon that finds a
  proxy of another version asks it to exit only while it has no request in
  flight, then starts its own.
- If the persisted port is taken by another program, the proxy moves to a free
  port and rewrites `endpoint.json`; only new terminals get the new URL.
- The terminal URL only sends requests through the proxy. Management
  (`/_okena/…`: health, commands, shutdown) needs `management.key` as a bearer
  token, and only the daemon sends it. Any process of the same OS user can still
  read the key file; it keeps management out of reach of anything that only
  has the URL from a shell's environment.

## Files (`<profile>/ai-proxy/`)

| File | Content |
|---|---|
| `endpoint.json` | port + path token (0600) |
| `management.key` | bearer token for the management routes (0600) |
| `accounts.json` | accounts with OAuth access/refresh tokens (0600) |
| `proxy.lock` | instance lock |
| `proxy.log` | proxy log (`RUST_LOG=okena_ai=debug` logs every routed request) |

## Limits

- Cooldown is per account, not per model: a weekly Opus limit also pauses Sonnet
  requests on that account until the reported reset.
- Accounts belong to the host that runs the daemon; a remote host has its own
  pool, managed through its own daemon.
- Only the desktop has management UI; the `ai_proxy` action is available to any
  client over the remote API.
