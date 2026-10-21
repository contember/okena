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

Turning the proxy on affects **new** terminals only. A project terminal,
including the project's hook and service terminals, gets
`ANTHROPIC_BASE_URL=http://127.0.0.1:<port>/<token>/p/<project-id>` (a worktree
terminal names its main project); other terminals get the URL without
`/p/<project-id>`. Running terminals keep their environment; turning the proxy
off leaves the proxy running for them.

`claude` itself still needs its normal login (it refuses to start without one).
That login is used only when the terminal's policy forbids no account and the
pool has no enabled, logged-in account: then the proxy forwards requests with
the terminal's own credentials.

## Project policies

A policy says which pool accounts a project's terminals may use. Each account
is **Prefer**, **Allow** or **Never**, and **New Accounts** (Allow or Never)
covers accounts added later.

| Need | Policy |
|---|---|
| Project X must use account A | A: Allow, others Never, New Accounts: Never |
| Project Y may use all, B first | B: Prefer, others Allow |
| Project Z must not use account C | C: Never, others Allow |

- **Settings → AI Proxy → Default Policy** applies to terminals of projects
  without their own policy, and to terminals without a project.
- To give a project its own policy, pick the project in the selector at the top
  of Settings, open **AI Proxy** and choose **Custom**. It starts as a copy of
  the default policy. **Default** removes it again.
- **Project Policies** on the User page lists the projects with their own
  policy; a deleted project's policy can be removed there.
- The proxy looks the policy up on every request: a change applies to running
  `claude` sessions with their next request.

## How requests are routed

- A `POST` to `/v1/messages` or `/v1/messages/count_tokens` (the endpoints
  Claude Code calls) gets a pool account; no other path does. The path is
  judged after `..` segments are resolved, and a path that still contains `%`
  after that gets a 400, so it cannot reach another endpoint with the
  account's token. The `authorization` header is replaced
  with that account's OAuth token and `metadata.user_id.account_uuid` is
  rewritten to the serving account. The minimal startup quota probe described
  below also has its model rewritten. Other endpoints (OAuth, profile, telemetry)
  pass through untouched.
- Claude Code's startup quota probe uses `claude-haiku-4-5-20251001` on
  `/v1/messages`. It must have integer `max_tokens: 1`, exactly one message
  containing only `role: "user"` and string `content: "quota"`, and streaming
  absent or `false`. Only `model`, `max_tokens`, `messages`, `metadata`, and
  `stream` fields are accepted for this rewrite; system prompts, tools, other
  generation options, and unknown fields leave the request unchanged. This
  also applies when the pool is empty and the caller's login is used.
  The workaround keeps the probe on Haiku when Claude Code selects the main
  model behind a custom base URL: the minimal probe can receive a bare 429
  on Opus even while regular inference succeeds. `quota_probe_model_rewrite`
  logs the original request context and target model at info level; subsequent
  attempt diagnostics show the rewritten model.
- Only accounts the terminal's policy does not forbid are used.
- An account with a known 5-hour or weekly utilization of 100% is skipped
  until that window resets or a usage update reports remaining quota. This
  does not require a preceding 429 response.
- A conversation (`x-claude-code-session-id`) stays on the account that served
  it last while that account is usable and permitted, so prompt caches keep
  working. A preferred account does not pull a conversation away from its
  account; a forbidden one moves it.
- Otherwise a preferred account goes before an allowed one. Among those, an
  account whose weekly quota would expire unused goes first: one that has more
  remaining than 20% per day until its weekly reset can consume. The account
  that would lose the most goes first. When no account would lose quota, the
  account with the lowest 5-hour utilization is chosen.
- **429**: the account cools down until the provider's reset time
  (`anthropic-ratelimit-unified-reset`), else `retry-after`, else one minute,
  and the request moves to the next permitted account. Each permitted account
  is tried once per request, plus once more after its token refresh. When all
  permitted accounts are limited, the provider's 429 is returned and `claude` waits and
  retries itself; a project that must use A waits for A.
  When the proxy rejects a request locally, a known exhausted quota window
  includes the unified status, representative claim and reset headers that
  let Claude Code display the usage limit and resume time. They describe the
  blocking window of the permitted account that becomes available first.
  A cooldown alone does not produce quota headers.
- **No account at all:** the terminal's own Claude login is used only when
  both hold: the terminal's policy forbids no account (no account and not
  **New Accounts** is **Never**), and no account the policy permits is enabled
  and logged in. A policy that forbids at least one account never falls back
  to that login, because it may be the forbidden account: when none of its
  permitted accounts is enabled and logged in, the request gets a 400 with the
  reason. (400, not 403: `claude` shows the message, where a 401/403 can send
  the user to `/login`.) A request that a pool account already tried never
  falls back to the terminal's own login either; it gets a 400 instead.
- **401**: the access token is refreshed once and the request retried; if the
  token endpoint answers `invalid_grant` (or an error without a code) the
  account is marked *Needs login*. Other token endpoint errors are temporary.
  A refresh runs to completion and is saved even when the request that started
  it is cancelled, because the provider rotates the refresh token.
- A request is never retried after its response started streaming.
- An upstream that sends nothing for 15 minutes (no response headers, or no
  next chunk of a streaming body) is cut: before headers `claude` gets a 502,
  during a stream the connection ends. This is longer than `claude`'s own
  request timeout and stream watchdog, so it only ends requests `claude` has
  already given up on.

Utilization comes from the `anthropic-ratelimit-unified-5h/7d-*` response
headers and from polling the usage endpoint every five minutes.

The status bar shows a separate entry for each pool account, with both
**5h** and **7d** when available. Disabled accounts and accounts needing login
show their state alongside the name. A red **100%** conveys quota exhaustion
without an extra status label; the account's popover names the exhausted window
and its reset (**7d limit until …**), the activity view as **5h/7d limit
reached**.
**429 cooldown** means the provider returned 429 and the proxy is waiting before trying that
account again. A 429 alone does not establish that either quota is at 100%.
The activity view records these responses as HTTP 429 events separately from
the account's current quota state.

Pool usage refreshes in the proxy, first five seconds after startup and then
every five minutes, and also follows response headers. Clients never poll the
proxy or the providers for it; the daemon is their only source.

While new terminals are routed through the proxy, the daemon reads the accounts
from it every 15 seconds while a client is connected and after each management
command that changes the accounts (finishing a login, removing, enabling or
disabling an account), and publishes them as `pool` in the usage channel (see
below). The daemon then stops polling the `claude` login's own usage: those
terminals use the pool accounts. Without a pool (routing off, the proxy
unreachable or without accounts) it polls the login again. The Claude usage widget in the status bar
shows the pool instead of the `claude` login's own usage. The Claude extension
settings choose any combination of three items per host:

- **Per-account usage** (default): the 5-hour and 7-day usage of each account.
  Hovering an account opens its own popover with its usage, plan size and
  state (disabled, needs login, quota exhausted or cooling down, with the time
  it ends). Remote accounts include the host name in their popover.
- **Nonstop** (hard limit): how long nonstop work at the usual pace lasts
  before every account is blocked. A simulation serves the pace from one account at a time,
  the one whose weekly quota expires first, and returns accounts as their
  windows reset. The pace is the 75th percentile of the last week's active
  hours; the cost of a session point in weekly points comes from the same
  history. Both fall back to defaults until there is enough history.
- **Today** (soft limit): how much more work today leaves enough weekly quota
  for every working day until the next resets. Work beyond it is possible but
  runs the pool short later in the week. A simulation over the next seven local
  days draws each day's use from the quota that resets first and finds the
  most a working day may use without running short, at most the pool's
  long-run share (all weekly quota over the working days). Non-working days
  use what they used last week. The value is that maximum minus today's use,
  shown as work at the usual pace ("3h left", "45m over").

Today and Nonstop weigh accounts by plan size (Pro 1, Max 5x 5, Max 20x 20),
from the `rate_limit_tier` of the OAuth profile. The proxy stores
it at login; accounts logged in before learn it once per proxy process from the
usage poll.

The host's daemon computes the pool, in its own time zone and with its working
days, and publishes it as `pool` in the Claude entry of the usage channel
(`GET /v1/usage`, `usage_changed`). It lists every account, with the reason
(`blocked`) for one that does not serve requests; only serving accounts count
in Today and Nonstop. The desktop and the mobile app only render it, for every
connected host whose daemon publishes it; a host with an older daemon shows
only the account items. The daemon recomputes the pool on each 15-second
accounts poll, after each proxy command that changes the accounts, and after
the routing setting or the working days change, and republishes it when it
changed. The pool follows each account's live
utilization, so while Claude is in use it can republish on most polls.
Hovering either item opens the pool popover with one section per
limit; an (i) next to Today and Nonstop explains each value in a tooltip. The
soft-limit section adds today's use, the safe daily use, and where the last 24
hours' pace runs short. One row per account follows, with a 5h and a 7d bar
that carry the same time marker and grid as the account's own popover. Clicking
a row shows that account's usage and history chart in the same popover.

Successful five-minute usage polls append quota samples to
`<profile>/ai-proxy/usage-history/claude.jsonl`, keyed by account ID. Collection
continues while the proxy runs, even with the desktop closed. Samples survive
proxy restarts; history begins with the first successful poll after upgrading.
While new shells route through the proxy, the daemon loads every in-service
account's last seven days through the authenticated `usage_history` management
command, at most once a minute per account, to compute the pool. History is
omitted from ordinary status and workspace snapshots and from the published
pool, which republishes often. Opening an account's detail reads its chart once
from the daemon's copy (`GET /v1/usage/pool/{account_id}`), reused for a minute
across desktop windows; the daemon does not ask the proxy for it again. The
sample types live in `okena-core::usage_history` because the proxy's wire
schema carries them; storage, pruning and chart data live in
`okena-agent-usage::history`, and `okena-usage` renders them without owning
proxy persistence.

## Activity

**AI Proxy Activity** (command palette; bind `ShowAiProxyActivity` in
`keybindings.json` for a shortcut) shows what the proxy did with `/v1/messages`
requests in the last hour. It updates every 5 seconds while open.

- **Accounts**: per pool account, its state, requests answered in the last
  minute and the last hour, error answers, 429s, and a per-minute graph. A row
  for the terminal's own login appears when requests went out with it.
- **Recent Events**: the last 50 failovers (429), rejected tokens, requests no
  account could take (all rate limited, no permitted account) and error
  answers with the provider's message, each with the terminal's project.
  A repeat of a listed event moves it to the top and raises its count.

The proxy keeps this in memory only: it starts empty after a proxy restart.
With *Route Claude Through the AI Proxy* off, the overlay does not start the
proxy and shows nothing.

## Lifecycle

- The daemon starts the proxy when the setting is on at startup, when the
  setting is turned on, and whenever the AI Proxy settings page sends a command.
  While the setting is on and a client (desktop, mobile, web) is connected, it
  asks the proxy for its accounts every 15 seconds and starts it again when it
  does not answer; after each failed start it waits twice as long, up to 10
  minutes. With no client connected it reads the accounts only after a
  management command or settings change, and at once when the next client
  connects. It still checks every five minutes, and whenever a terminal routed
  through the proxy starts (hooks and services included), that the proxy
  answers, and starts it when it does not. The terminal does not wait for that
  check.
- On Linux the proxy starts in its own systemd scope (`systemd-run --user
  --scope`), so stopping or OOM-killing the app's cgroup does not stop it;
  without `systemd-run`, or when it fails, the proxy starts in a new session
  only. On Windows it starts outside the app's job when the job allows that.
- The proxy holds `ai-proxy/proxy.lock`. A second instance waits up to 45
  seconds for the lock, longer than an exiting proxy needs to finish its token
  refreshes, and exits if it does not get it. The daemon waits that long for a
  proxy it started to answer, and stops waiting when the process exits.
- It is never stopped by the daemon, the GUI or an update. A daemon that finds a
  proxy of another version asks it to exit only while it has no request in
  flight, then starts its own.
- There is no restart button. **Refresh** in AI Proxy settings sends a status
  command through this version check; it replaces an idle proxy only when its
  version differs. A rebuild with the same version does not replace the proxy.
  A busy older proxy is checked again on a later management command.
- If the persisted port is taken by another program, the proxy moves to a free
  port and rewrites `endpoint.json`; only new terminals get the new URL. Each
  terminal reads `endpoint.json` when it starts.
- The terminal URL only sends requests through the proxy. Management
  (`/_okena/…`: health, commands, shutdown) is not served on the TCP port. It
  listens on `ai-proxy/management.sock` (Unix), or on Windows on a named pipe
  that only the current user can open, named after a hash of `management.key`.
  It also needs `management.key` as a bearer token, and only the daemon sends
  it. Any process of the same OS user can still read the key file; the key
  keeps management out of reach of anything that only has the URL from a
  shell's environment. Why a socket: [ADR-0007](../decisions/0007-ai-proxy-management-on-a-private-socket.md).

## Files (`<profile>/ai-proxy/`, owner-only directory)

The directory is mode 0700 on Unix. On Windows its access list grants the
current user only, and the files inside inherit it.

| File | Content |
|---|---|
| `endpoint.json` | port + path token (0600) |
| `management.key` | bearer token for the management routes (0600) |
| `management.sock` | management socket (Unix) |
| `accounts.json` | accounts with OAuth access/refresh tokens (0600) |
| `routing.json` | default and per-project policies, by account id (0600) |
| `proxy.lock` | instance lock |
| `proxy.log` | proxy log (`RUST_LOG=okena_ai=debug` logs every routed request) |

Provider HTTP errors, including 401 and 429 attempts that are retried, are
logged at warning level with method, endpoint path, project/account IDs, status,
error type/message, request ID, and selected response headers (`retry-after`,
`anthropic-ratelimit-*`, request IDs, `cf-ray`, content type/encoding/length,
and server). Gzip, Brotli and deflate responses are decoded before logging or
relaying; the relay removes obsolete encoding and length headers.
Diagnostics accept nested error objects, string errors and top-level
`message`, `error_description` or `detail` fields. An unexpected JSON shape
lists up to 16 field names at the root and inside `error`, each limited to 64
characters, without dumping unknown values. Invalid JSON records the parse
error category and position. Request IDs fall back to response headers.
Messages are limited to 2,048 characters and header values to 256 characters;
control characters are escaped. Non-JSON error bodies are represented by
their size, headers and parse failure, not dumped into the log.

`upstream_error` identifies provider responses. `proxy_rejection` identifies
local rejections and includes the reason and earliest retry time when known.
Connection failures, unreadable error bodies, and interrupted response streams
have separate log entries with request context. Request bodies, query strings,
authorization headers, and cookies are not included. These diagnostics apply
to the relay; SSE error events inside an otherwise successful HTTP stream are
forwarded without parsing.

Inference diagnostics include a process-local `relay_id` (PID and sequence),
attempt number and elapsed times, model, requested `max_tokens`, streaming flag,
request byte size, a SHA-256 session hash prefix, the numeric SDK retry count,
and the current number of in-flight requests across the proxy. Retries within
one relay keep the same ID; client retries can be matched by the session hash.
These fields exclude message content and the raw session/user identity.
On provider failures, `recent_traffic` reports each active account's answers in
the last 60 seconds and hour, plus hourly errors and 429s. Successful streams
are counted when headers arrive; HTTP errors after reading their body, and
connection failures when detected. These counts do not establish that a stream
completed successfully. 429 attempts are counted separately.
`account_availability` accompanies 429s and local account
rejections with policy, enabled/login state, retry exclusions, quota windows,
and cooldown/block expiry for each account. `failover_recovered` logs a
successful response after another upstream attempt at info level.
These error diagnostics work at the default log level. Debug logging also
records each upstream response with the same request context and selected
headers. Response diagnostics additionally allow `x-ratelimit-*`,
`retry-after-ms`, `x-should-retry`, and `x-envoy-upstream-service-time`.

## Limits

- The project in the URL routes requests; it does not isolate them. A process of
  the same user can put another project's id into its URL.
- While the proxy is down, any local user can bind the persisted loopback port.
  Shells started with that URL then send their path token, the request (prompt
  included), and their own `claude` OAuth token to whatever listens there. The
  path token alone gives no management access.
- The management socket path must fit the platform limit (104 bytes on macOS,
  108 on Linux). With a longer profile path the proxy does not start, and
  `proxy.log` names the socket.
- A corrupt `routing.json` stops the proxy from starting rather than dropping
  the policies.

- Cooldown is per account, not per model: a weekly Opus limit also pauses Sonnet
  requests on that account until the reported reset.
- Accounts belong to the host that runs the daemon; a remote host has its own
  pool, managed through its own daemon.
- Only the desktop has management UI; the `ai_proxy` action is available to any
  client over the remote API.
