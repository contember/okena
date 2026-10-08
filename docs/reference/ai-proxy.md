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

Turning the proxy on affects **new** terminals only. A project terminal gets
`ANTHROPIC_BASE_URL=http://127.0.0.1:<port>/<token>/p/<project-id>` (a worktree
terminal names its main project); other terminals get the URL without
`/p/<project-id>`. Running terminals keep their environment; turning the proxy
off leaves the proxy running for them.

`claude` itself still needs its normal login (it refuses to start without one).
That login is used only when the pool has no enabled, logged-in account: then
the proxy forwards requests with the terminal's own credentials.

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

- `/v1/messages*` gets a pool account: the `authorization` header is replaced
  with that account's OAuth token and `metadata.user_id.account_uuid` is
  rewritten to the serving account. Everything else (OAuth, profile, telemetry)
  passes through untouched.
- Only accounts the terminal's policy does not forbid are used.
- A conversation (`x-claude-code-session-id`) stays on the account that served
  it last while that account is usable and permitted, so prompt caches keep
  working. A preferred account does not pull a conversation away from its
  account; a forbidden one moves it.
- Otherwise a preferred account goes before an allowed one, and among those the
  account with the lowest 5-hour utilization is chosen.
- **429**: the account cools down until the provider's reset time
  (`anthropic-ratelimit-unified-reset`), else `retry-after`, else one minute,
  and the request moves to the next permitted account. When all permitted
  accounts are limited, the provider's 429 is returned and `claude` waits and
  retries itself; a project that must use A waits for A.
- **No account at all:** with no enabled, logged-in account the proxy forwards
  requests with the terminal's own Claude login, unless the policy forbids an
  account (that login may be the forbidden one). Such a project, and any
  project whose permitted accounts are all disabled or logged out, gets a 400
  with the reason. (400, not 403: `claude` shows the message, where a 401/403
  can send the user to `/login`.)
- **401**: the access token is refreshed once and the request retried; if the
  refresh token is rejected the account is marked *Needs login*.
- A request is never retried after its response started streaming.
- An upstream that sends nothing for 15 minutes (no response headers, or no
  next chunk of a streaming body) is cut: before headers `claude` gets a 502,
  during a stream the connection ends. This is longer than `claude`'s own
  request timeout and stream watchdog, so it only ends requests `claude` has
  already given up on.

Utilization comes from the `anthropic-ratelimit-unified-5h/7d-*` response
headers and from polling the usage endpoint every five minutes.

While new terminals are routed through the proxy, the daemon reads the accounts
from it every 15 seconds and after each management command, and sends them to
clients with the workspace state. The Claude usage widget in the status bar then
shows the pool instead of the `claude` login's own usage: the 5-hour usage of
each enabled account in the bar, and each account's 5-hour and 7-day usage, plan
and state (disabled, needs login, rate limited) in the popover. A remote host's
accounts are listed under the host's name.

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
| `routing.json` | default and per-project policies, by account id (0600) |
| `proxy.lock` | instance lock |
| `proxy.log` | proxy log (`RUST_LOG=okena_ai=debug` logs every routed request) |

## Limits

- Hook and service terminals carry no project and use the default policy.
- The project in the URL routes requests; it does not isolate them. A process of
  the same user can put another project's id into its URL.
- A corrupt `routing.json` stops the proxy from starting rather than dropping
  the policies.

- Cooldown is per account, not per model: a weekly Opus limit also pauses Sonnet
  requests on that account until the reported reset.
- Accounts belong to the host that runs the daemon; a remote host has its own
  pool, managed through its own daemon.
- Only the desktop has management UI; the `ai_proxy` action is available to any
  client over the remote API.
