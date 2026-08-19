# Okena Web Client

Remote web client for Okena terminal multiplexer. Connects to an Okena **daemon** via REST + WebSocket — the same protocol the desktop and mobile clients use.

## Stack

- **Runtime / Package manager:** Bun
- **Framework:** React 19
- **Language:** TypeScript (strict mode)
- **Build tool:** Vite
- **Styling:** Tailwind CSS 4 (utility classes, no CSS modules, no component library)
- **Terminal:** xterm.js (`@xterm/xterm`) with WebGL renderer + fit addon
- **State management:** React Reducer + Context API

## Commands

```bash
bun install        # Install dependencies
bun run dev        # Dev server with HMR (proxies API to localhost:19100)
bun run build      # TypeScript check + production build
```

## Development

Vite dev server proxies `/v1/*` and `/health` to `http://localhost:19100`. Before running the web client, start a daemon — either `cargo run` (the desktop spawns one) or `cargo run -p okena-daemon` standalone.

## Architecture

- **Auth:** 4-letter pairing code → JWT token in localStorage, auto-refresh at 75% TTL
- **REST API:** `/v1/pair`, `/v1/state`, `/v1/actions`, `/v1/refresh`
- **WebSocket** (`/v1/stream`): binary frame protocol for PTY I/O, JSON for control messages, auto-reconnect with exponential backoff
- **Layout:** recursive tree of splits/tabs/terminals, mirroring desktop's `LayoutNode`
